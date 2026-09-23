//! Direct and queued submission, removal and internal fixture admission.

use super::super::model::Resolved;
use super::admission::{Admission, AdmissionMode, capacity_rejection};
use super::*;

/// Local entry points share resolution and its original observations. Their
/// completion boundary differs: ordinary submission verifies before replying;
/// the integration-test entry point transfers resolved work to the queue.
struct LocalPreparation {
    candidate: Arc<Entry>,
    before: Option<Arc<Entry>>,
    view: u64,
    reads: ReadSet,
    resolved: Result<Arc<Resolved>, Reject>,
}

impl Pool {
    fn prepare_local(
        &self,
        transaction: &Arc<TransactionView>,
        arrival: u64,
    ) -> Result<LocalPreparation, Error> {
        let (view, snapshot) = self.store.snapshot();
        let mut reads = ReadSet::default();
        let before = self.store.get(&transaction.hash(), &mut reads)?;
        if before
            .as_ref()
            .is_some_and(|entry| entry.accepted().is_some())
        {
            return Err(Error::Rejected(Reject::Duplicated(transaction.hash())));
        }
        let candidate = Arc::new(Entry {
            transaction: Arc::clone(transaction),
            arrival: before.as_ref().map_or(arrival, |entry| entry.arrival),
            source: Source::Local,
            phase: Phase::Resolve,
        });
        let resolved = match non_contextual_verify(snapshot.consensus(), transaction) {
            Err(reject) => Err(reject),
            Ok(()) => match jobs::resolve(&self.store, &candidate, &self.config)? {
                Resolution::Ready(resolved) => {
                    reads.merge(&resolved.reads)?;
                    Ok(resolved)
                }
                Resolution::Waiting(keys, observed) => {
                    reads.merge(&observed)?;
                    let point = keys
                        .into_iter()
                        .find_map(|key| match key {
                            DependencyKey::Cell(point) => Some(point),
                            DependencyKey::Header(_) => None,
                        })
                        .ok_or(Error::Fault("missing cell condition"))?;
                    Err(Reject::Resolve(OutPointError::Unknown(point)))
                }
                Resolution::Rejected(reject, observed) => {
                    reads.merge(&observed)?;
                    Err(reject)
                }
            },
        };
        Ok(LocalPreparation {
            candidate,
            before,
            view,
            reads,
            resolved,
        })
    }

    pub(super) async fn ingest(
        &self,
        transaction: Arc<TransactionView>,
        source: Source,
    ) -> Result<(), Error> {
        loop {
            self.open()?;
            let result = self
                .commit(|| {
                    self.open()?;
                    ingress::prepare(&self.store, Arc::clone(&transaction), source)
                })
                .await;
            match result {
                Ok(batch) => return self.published(batch).await,
                Err(Error::Stale) => tokio::task::yield_now().await,
                Err(Error::Full(reason)) => {
                    // Admission pressure still owes the relayer its release.
                    // This plan is owner-free and fits one fixed notice slot.
                    let batch = self
                        .commit(|| {
                            self.open()?;
                            ingress::capacity_refusal(
                                &self.store,
                                &transaction.hash(),
                                source,
                                reason,
                            )
                        })
                        .await;
                    match batch {
                        Ok(batch) => return self.published(batch).await,
                        Err(Error::Stale) => continue,
                        Err(error) => return Err(error),
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }
    pub(crate) async fn submit_remote(
        &self,
        transaction: BoundedTransaction,
        cycles: Cycle,
        peer: PeerIndex,
    ) -> Result<(), Error> {
        self.ingest(
            transaction.into_transaction(),
            ingress::remote_source(peer, cycles)?,
        )
        .await
    }
    pub(crate) async fn submit_remote_batch(
        &self,
        peer: PeerIndex,
        submissions: Vec<(BoundedTransaction, Cycle)>,
    ) -> Result<(), (usize, Error)> {
        for (completed, (transaction, cycles)) in submissions.into_iter().enumerate() {
            if let Err(error) = self.submit_remote(transaction, cycles, peer).await {
                return Err((completed, error));
            }
            // Fresh ingress can commit without polling any Tokio resource.
            tokio::task::coop::consume_budget().await;
        }
        Ok(())
    }
    pub(crate) async fn submit_proposal_batch(
        &self,
        transactions: Vec<BoundedTransaction>,
    ) -> Result<(), Error> {
        for transaction in transactions {
            self.ingest(
                transaction.into_transaction(),
                Source::Proposal { remote: None },
            )
            .await?;
            tokio::task::coop::consume_budget().await;
        }
        Ok(())
    }
    async fn reject_local(
        &self,
        view: u64,
        hash: &Byte32,
        reject: Reject,
        reads: ReadSet,
        action: AdmissionMode,
    ) -> Result<Reject, Error> {
        match action {
            AdmissionMode::Preview => {
                let check = Plan::new(view, Class::Trusted, reads);
                self.store.apply(check.dry_run())?;
            }
            AdmissionMode::Commit => {
                let batch = self
                    .commit(|| {
                        ingress::rejection(
                            &self.store,
                            Plan::new(view, ingress::class(Source::Local), reads.clone()),
                            None,
                            hash,
                            Source::Local,
                            reject.clone(),
                        )
                    })
                    .await?;
                self.published(batch).await?;
            }
        }
        Ok(reject)
    }
    /// Reply after verification, admission and publication complete.
    pub(crate) async fn submit_local(
        &self,
        transaction: BoundedTransaction,
    ) -> Result<Result<EntryCompleted, Reject>, Error> {
        self.process_local(transaction, AdmissionMode::Commit).await
    }

    /// Verify admission without changing membership or waiting for capacity.
    pub(crate) async fn test_accept(
        &self,
        transaction: BoundedTransaction,
    ) -> Result<Result<EntryCompleted, Reject>, Error> {
        self.process_local(transaction, AdmissionMode::Preview)
            .await
    }

    async fn process_local(
        &self,
        transaction: BoundedTransaction,
        action: AdmissionMode,
    ) -> Result<Result<EntryCompleted, Reject>, Error> {
        let transaction = transaction.into_transaction();
        let arrival = self.store.next_arrival()?;
        loop {
            match self.attempt_local(&transaction, arrival, action).await {
                Err(Error::Stale) => continue,
                Err(Error::Rejected(reject)) => return Ok(Err(reject)),
                result => return result.map(Ok),
            }
        }
    }

    /// A stale attempt releases its work before retrying with the same arrival.
    /// Active memory remains owned through admission and publication.
    async fn attempt_local(
        &self,
        transaction: &Arc<TransactionView>,
        arrival: u64,
        action: AdmissionMode,
    ) -> Result<EntryCompleted, Error> {
        self.open()?;
        let capacity = match action {
            AdmissionMode::Commit => self.direct_capacity().await,
            // A preview may run inside a publication callback. Waiting for
            // jobs whose publication needs that callback would deadlock.
            AdmissionMode::Preview => self.try_direct_capacity(),
        };
        let (cpu, _memory) = capacity.map_err(capacity_rejection)?;
        let LocalPreparation {
            candidate,
            before,
            view,
            reads,
            resolved,
        } = self
            .run_compute(&cpu, || self.prepare_local(transaction, arrival))
            .map_err(capacity_rejection)?;
        let resolved = match resolved {
            Ok(resolved) => resolved,
            Err(reject) => {
                drop(cpu);
                let reject = self
                    .reject_local(view, &transaction.hash(), reject, reads, action)
                    .await?;
                return Err(Error::Rejected(reject));
            }
        };
        let verified = jobs::verify(
            &self.store,
            &candidate,
            Arc::clone(&resolved),
            &self.config,
            &self.cache,
            &mut self.commands.clone(),
            self.mode,
        )
        .await;
        drop(cpu);
        self.open()?;
        let verified = match verified {
            Ok(verified) => verified,
            Err(Error::Rejected(reject)) => {
                let reject = self
                    .reject_local(resolved.view, &transaction.hash(), reject, reads, action)
                    .await?;
                return Err(Error::Rejected(reject));
            }
            Err(error) => return Err(capacity_rejection(error)),
        };
        Admission::new(self, &candidate, before.as_ref(), &verified, action)
            .complete()
            .await
    }

    /// Acknowledge resolved queue ownership; script verification finishes later.
    pub(crate) async fn enqueue_local_test(
        &self,
        transaction: BoundedTransaction,
    ) -> Result<Result<(), Reject>, Error> {
        let transaction = transaction.into_transaction();
        let arrival = self.store.next_arrival()?;
        loop {
            match self.attempt_local_enqueue(&transaction, arrival).await {
                Err(Error::Stale) => continue,
                Err(Error::Rejected(reject)) => return Ok(Err(reject)),
                result => return result.map(Ok),
            }
        }
    }

    async fn attempt_local_enqueue(
        &self,
        transaction: &Arc<TransactionView>,
        arrival: u64,
    ) -> Result<(), Error> {
        let (cpu, _memory) = self.direct_capacity().await.map_err(capacity_rejection)?;
        let prepared = self.run_compute(&cpu, || self.prepare_local(transaction, arrival));
        drop(cpu);
        let LocalPreparation {
            candidate,
            before,
            view,
            reads,
            resolved,
        } = prepared.map_err(capacity_rejection)?;
        if before.is_some() {
            return Err(Error::Rejected(Reject::Duplicated(transaction.hash())));
        }
        let resolved = match resolved {
            Ok(resolved) => resolved,
            Err(reject) => {
                let reject = self
                    .reject_local(
                        view,
                        &transaction.hash(),
                        reject,
                        reads,
                        AdmissionMode::Commit,
                    )
                    .await?;
                return Err(Error::Rejected(reject));
            }
        };
        let batch = self
            .commit(|| {
                self.open()?;
                let mut plan = Plan::new(resolved.view, Class::Trusted, reads.clone());
                plan.edit(
                    None,
                    Some(candidate.with_phase(Phase::Verify(Arc::clone(&resolved)))),
                    None,
                )?;
                Ok(plan)
            })
            .await
            .map_err(capacity_rejection)?;
        self.published(batch).await
    }
    pub(crate) async fn remove_local(
        &self,
        hash: &Byte32,
    ) -> Result<Result<bool, LocalRemovalCompetingProgress>, Error> {
        self.open()?;
        let (_, root) = self.store.point(hash);
        let Some(root) = root else {
            return Ok(Ok(false));
        };
        match self
            .commit(|| membership::removal(&self.store, &root, &self.config, None))
            .await
        {
            Ok(batch) => {
                self.published(batch).await?;
                Ok(Ok(true))
            }
            Err(Error::Stale) => Ok(Err(LocalRemovalCompetingProgress)),
            Err(error) => Err(error),
        }
    }
    #[cfg(feature = "internal")]
    pub(crate) async fn plug(
        &self,
        entries: Vec<crate::TxEntry>,
        target: crate::PlugTarget,
    ) -> Result<(), Error> {
        use super::super::model::Status;
        let status = match target {
            crate::PlugTarget::Pending => Status::Pending,
            crate::PlugTarget::Proposed => Status::Proposed,
        };
        for entry in entries {
            let transaction = BoundedTransaction::try_new(entry.transaction().clone())
                .map_err(|_| Error::Full("internal transaction materialization".into()))?
                .into_transaction();
            loop {
                let (_, _memory) = self.direct_capacity().await?;
                let (view, _) = self.store.snapshot();
                let mut reads = ReadSet::default();
                let before = self.store.get(&transaction.hash(), &mut reads)?;
                if before
                    .as_ref()
                    .is_some_and(|entry| entry.accepted().is_some())
                {
                    break;
                }
                if before.is_some() {
                    return Err(Error::Full(
                        "internal insertion would displace an owner".into(),
                    ));
                }
                let mut pool_cells = BTreeSet::new();
                for point in entry
                    .rtx
                    .resolved_inputs
                    .iter()
                    .chain(&entry.rtx.resolved_cell_deps)
                    .chain(&entry.rtx.resolved_dep_groups)
                    .map(|cell| &cell.out_point)
                {
                    if self
                        .store
                        .get(&point.tx_hash(), &mut reads)?
                        .is_some_and(|owner| owner.accepted().is_some())
                    {
                        pool_cells.insert(point.clone());
                    }
                }
                let verified = jobs::fixture(
                    view,
                    &entry,
                    Arc::clone(&transaction),
                    reads.into_verification_reads(),
                    pool_cells,
                    status,
                );
                let candidate = Arc::new(Entry {
                    transaction: Arc::clone(&transaction),
                    arrival: before
                        .as_ref()
                        .map_or_else(|| self.store.next_arrival(), |entry| Ok(entry.arrival))?,
                    source: Source::Local,
                    phase: Phase::Resolve,
                });
                let result = self
                    .commit(|| {
                        let (mut plan, reject) = membership::admission(
                            &self.store,
                            &candidate,
                            before.clone(),
                            &verified,
                            &self.config,
                            false,
                        )?;
                        if let Some(reject) = reject {
                            return Err(Error::Rejected(reject));
                        }
                        if plan.edits().values().any(|edit| edit.before.is_some()) {
                            return Err(Error::Rejected(Reject::Full(
                                "internal insertion would displace an owner".into(),
                            )));
                        }
                        plan.silence_fixture();
                        Ok(plan)
                    })
                    .await;
                match result {
                    Ok(batch) => {
                        self.published(batch).await?;
                        break;
                    }
                    Err(Error::Stale) => continue,
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(())
    }
}
