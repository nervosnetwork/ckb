//! Direct and queued submission, removal and internal fixture admission.

use super::super::model::Resolved;
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
                    match ingress::prepare(&self.store, Arc::clone(&transaction), source) {
                        Ok(plan) => Ok(plan),
                        Err(Error::Full(reason)) => ingress::capacity_refusal(
                            &self.store,
                            &transaction.hash(),
                            source,
                            reason,
                        ),
                        Err(error) => Err(error),
                    }
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
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "Completed count advances once per transaction in one protocol-bounded batch."
    )]
    pub(crate) async fn submit_remote_batch(
        &self,
        peer: PeerIndex,
        submissions: Vec<(BoundedTransaction, Cycle)>,
    ) -> (usize, Option<Error>) {
        let mut completed = 0;
        for (transaction, cycles) in submissions {
            if let Err(error) = self.submit_remote(transaction, cycles, peer).await {
                return (completed, Some(error));
            }
            completed += 1;
            // Fresh ingress can commit without polling any Tokio resource.
            tokio::task::coop::consume_budget().await;
        }
        (completed, None)
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
    pub(super) async fn reject_local(
        &self,
        view: u64,
        hash: &Byte32,
        reject: Reject,
        reads: ReadSet,
        dry_run: bool,
    ) -> Result<Reject, Error> {
        if dry_run {
            let check = Plan::new(view, Class::Trusted, reads);
            self.store.apply(check.dry_run())?;
        } else {
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
        Ok(reject)
    }
    pub(crate) async fn submit_local(
        &self,
        transaction: BoundedTransaction,
        dry_run: bool,
    ) -> Result<Result<EntryCompleted, Reject>, Error> {
        let transaction = transaction.into_transaction();
        let arrival = self.store.next_arrival()?;
        loop {
            self.open()?;
            // A dry-run may be invoked by a publication callback. It must not
            // wait for active jobs whose terminal publication needs that callback.
            let (cpu, _memory) = match self.direct_capacity(!dry_run).await {
                Ok(capacity) => capacity,
                Err(Error::Full(reason)) => return Ok(Err(Reject::Full(reason.to_string()))),
                Err(error) => return Err(error),
            };
            let LocalPreparation {
                candidate,
                before,
                view,
                reads,
                resolved,
            } = match self.run_compute(&cpu, || self.prepare_local(&transaction, arrival)) {
                Ok(prepared) => prepared,
                Err(Error::Stale) => continue,
                Err(Error::Rejected(reject)) => return Ok(Err(reject)),
                Err(Error::Full(reason)) => return Ok(Err(Reject::Full(reason.to_string()))),
                Err(error) => return Err(error),
            };
            let resolved = match resolved {
                Ok(resolved) => resolved,
                Err(reject) => {
                    drop(cpu);
                    match self
                        .reject_local(view, &transaction.hash(), reject, reads, dry_run)
                        .await
                    {
                        Err(Error::Stale) => continue,
                        result => return result.map(Err),
                    }
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
                Err(Error::Stale) => continue,
                Err(Error::Rejected(reject)) => {
                    match self
                        .reject_local(resolved.view, &transaction.hash(), reject, reads, dry_run)
                        .await
                    {
                        Err(Error::Stale) => continue,
                        result => return result.map(Err),
                    }
                }
                Err(Error::Full(reason)) => return Ok(Err(Reject::Full(reason.to_string()))),
                Err(error) => return Err(error),
            };
            let mut retain_history = !dry_run;
            let mut attempt = || {
                self.open()?;
                let (plan, reject) = membership::admission(
                    &self.store,
                    &candidate,
                    before.clone(),
                    &verified,
                    &self.config,
                    retain_history,
                )?;
                let applied = if dry_run {
                    self.store.apply(plan.dry_run())
                } else {
                    self.store.apply_admission(plan, &mut retain_history)
                };
                applied.map(|batch| (batch, reject))
            };
            let applied = if dry_run {
                attempt()
            } else {
                self.commit_attempt(attempt).await
            };
            match applied {
                Ok((batch, rejection)) => {
                    self.published(batch).await?;
                    return Ok(rejection.map_or_else(
                        || {
                            Ok(EntryCompleted {
                                cycles: verified.cycles(),
                                fee: verified.resolved().fee,
                            })
                        },
                        Err,
                    ));
                }
                Err(Error::Stale) => continue,
                Err(Error::Full(reason)) => return Ok(Err(Reject::Full(reason.to_string()))),
                Err(error) => return Err(error),
            }
        }
    }

    pub(crate) async fn submit_local_test(
        &self,
        transaction: BoundedTransaction,
    ) -> Result<Result<(), Reject>, Error> {
        let transaction = transaction.into_transaction();
        let arrival = self.store.next_arrival()?;
        loop {
            let (cpu, _memory) = self.direct_capacity(true).await?;
            let prepared = self.run_compute(&cpu, || self.prepare_local(&transaction, arrival));
            drop(cpu);
            let LocalPreparation {
                candidate,
                before,
                view,
                reads,
                resolved,
            } = match prepared {
                Ok(prepared) => prepared,
                Err(Error::Stale) => continue,
                Err(Error::Rejected(reject)) => return Ok(Err(reject)),
                Err(Error::Full(reason)) => return Ok(Err(Reject::Full(reason.to_string()))),
                Err(error) => return Err(error),
            };
            if before.is_some() {
                return Ok(Err(Reject::Duplicated(transaction.hash())));
            }
            let resolved = match resolved {
                Ok(resolved) => resolved,
                Err(reject) => match self
                    .reject_local(view, &transaction.hash(), reject, reads, false)
                    .await
                {
                    Err(Error::Stale) => continue,
                    result => return result.map(Err),
                },
            };
            let result = self
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
                .await;
            match result {
                Ok(batch) => {
                    self.published(batch).await?;
                    return Ok(Ok(()));
                }
                Err(Error::Stale) => continue,
                Err(Error::Full(reason)) => return Ok(Err(Reject::Full(reason.to_string()))),
                Err(error) => return Err(error),
            }
        }
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
    ) -> Result<(), Reject> {
        use super::super::model::Status;
        let status = match target {
            crate::PlugTarget::Pending => Status::Pending,
            crate::PlugTarget::Proposed => Status::Proposed,
        };
        for entry in entries {
            let transaction = BoundedTransaction::try_new(entry.transaction().clone())
                .map_err(|_| Reject::Full("internal transaction materialization".into()))?
                .into_transaction();
            loop {
                let (_, _memory) = self.direct_capacity(true).await.map_err(as_reject)?;
                let (view, _) = self.store.snapshot();
                let mut reads = ReadSet::default();
                let before = self
                    .store
                    .get(&transaction.hash(), &mut reads)
                    .map_err(as_reject)?;
                if before
                    .as_ref()
                    .is_some_and(|entry| entry.accepted().is_some())
                {
                    break;
                }
                if before.is_some() {
                    return Err(Reject::Full(
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
                        .get(&point.tx_hash(), &mut reads)
                        .map_err(as_reject)?
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
                        .map_or_else(|| self.store.next_arrival(), |entry| Ok(entry.arrival))
                        .map_err(as_reject)?,
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
                        self.published(batch).await.map_err(as_reject)?;
                        break;
                    }
                    Err(Error::Stale) => continue,
                    Err(error) => return Err(as_reject(error)),
                }
            }
        }
        Ok(())
    }
}

#[cfg(feature = "internal")]
fn as_reject(error: Error) -> Reject {
    match error {
        Error::Rejected(reject) => reject,
        Error::Full(reason) => Reject::Full(reason.to_string()),
        error => Reject::Internal(error.to_string()),
    }
}
