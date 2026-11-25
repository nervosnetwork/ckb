//! Direct and queued submission, removal and internal fixture admission.

use super::super::model::{Premises, Resolved};
use super::{membership::AdmissionOptions, *};

/// Commit membership and effects, or validate the same decision without mutation.
#[derive(Clone, Copy)]
enum AdmissionMode {
    Commit,
    Preview,
}

// Verification/admission capacity is an ordinary local refusal. Errors from
// publication keep their service-failure meaning and do not pass this boundary.
fn capacity_rejection(error: Error) -> Error {
    match error {
        Error::Full(reason) => Error::Rejected(Reject::Full(reason.to_string())),
        error => error,
    }
}

struct LocalContext {
    candidate: Arc<Entry>,
    before: Option<Arc<Entry>>,
    view: u64,
    snapshot: Arc<Snapshot>,
}

type LocalResolution = Result<Arc<Resolved>, (Reject, Premises)>;

enum LocalPrepared {
    Verified(Result<Verified, Error>, Arc<Resolved>),
    NeedsAsyncVerify(Arc<Resolved>),
    ResolveRejected(Reject, Premises),
}

const LOCAL_FAST_STALE_RETRIES: usize = 4;

/// Result of running a direct local submission on a dedicated compute worker.
pub(crate) enum LocalFastOutcome {
    /// Finished commit/rejection and any required outbox publication.
    Done(Result<Result<(), Reject>, Error>),
    /// Committed and activated `batch` in `Outbox`; caller awaits publication.
    WaitPublish {
        result: Result<(), Reject>,
        batch: Arc<Batch>,
    },
    /// Compute or active-job capacity was momentarily contended; requeue in
    /// `ComputePool::local` and wait for capacity instead of cascading into
    /// async fallback.
    RetryCapacity(BoundedTransaction),
    /// Needs full async retry/wait (verification paused, async VM fallback,
    /// outbox temporarily full, or repeated view churn).
    FallbackAsync(BoundedTransaction),
}

async fn consume_batch_coop(count: usize) {
    for _ in 0..count {
        tokio::task::coop::consume_budget().await;
    }
}

fn prepare_local_with(
    store: &Store,
    config: &TxPoolConfig,
    transaction: &Arc<TransactionView>,
    arrival: u64,
) -> Result<(LocalContext, LocalResolution), Error> {
    let hash = transaction.hash_ref();
    let points = jobs::prefetch_points(transaction, store.budget.limits.per_job.edges);
    let mut premises = Premises::default();
    let obs = store.observe_resolution(Some(hash), &points, &mut premises)?;
    if obs.before.as_ref().is_some_and(|e| e.accepted().is_some()) {
        return Err(Error::Rejected(Reject::Duplicated(hash.clone())));
    }
    let candidate = Entry::try_new(
        Arc::clone(transaction),
        obs.before.as_ref().map_or(arrival, |entry| entry.arrival),
        Source::Local,
        Phase::Resolve,
    )?;
    let resolved = match non_contextual_verify(obs.snapshot.consensus(), transaction) {
        Err(reject) => {
            let mut reject_premises = Premises::default();
            reject_premises.observe_owner(hash, obs.before.as_ref())?;
            Err((reject, reject_premises))
        }
        Ok(()) => match jobs::resolve_prefetched(
            store,
            &candidate,
            config,
            obs.view,
            &obs.snapshot,
            premises,
            obs.prefetched,
        )? {
            Resolution::Ready(resolved) => {
                resolved.premises.check_owner(hash, obs.before.as_ref())?;
                Ok(resolved)
            }
            Resolution::Waiting(keys, mut premises) => {
                premises.observe_owner(hash, obs.before.as_ref())?;
                let point = keys
                    .into_iter()
                    .next()
                    .ok_or(Error::Fault("missing cell condition"))?;
                Err((Reject::Resolve(OutPointError::Unknown(point)), premises))
            }
            Resolution::Rejected(reject, mut premises) => {
                premises.observe_owner(hash, obs.before.as_ref())?;
                Err((reject, premises))
            }
        },
    };
    Ok((
        LocalContext {
            candidate,
            before: obs.before,
            view: obs.view,
            snapshot: obs.snapshot,
        },
        resolved,
    ))
}

/// Run local resolution and synchronous VM/proof-cache verification in one
/// synchronous frame on a dedicated compute worker.
fn prepare_and_verify_local_with(
    store: &Store,
    config: &TxPoolConfig,
    cache: &RwLock<TxVerificationCache>,
    transaction: &Arc<TransactionView>,
    arrival: u64,
) -> Result<(LocalContext, LocalPrepared), Error> {
    let (ctx, resolved) = prepare_local_with(store, config, transaction, arrival)?;
    let outcome = match resolved {
        Err((reject, premises)) => LocalPrepared::ResolveRejected(reject, premises),
        Ok(resolved) => match jobs::try_verify_local_sync(
            #[cfg(any(test, feature = "internal"))]
            store,
            &ctx.snapshot,
            &ctx.candidate,
            Arc::clone(&resolved),
            cache,
        ) {
            Some(verified) => LocalPrepared::Verified(verified, resolved),
            None => LocalPrepared::NeedsAsyncVerify(resolved),
        },
    };
    Ok((ctx, outcome))
}

impl Pool {
    fn settle_publish_fast(
        &self,
        batch: Option<Arc<Batch>>,
        result: Result<(), Reject>,
    ) -> LocalFastOutcome {
        // When `commit_inline` was selected (idle local queue),
        // `Batch::activate_nonblocking` already published non-blocking
        // in-memory batches inline while keeping callbacks, `recent_reject` DB
        // writes, bans, and block updates isolated on `TxPoolPublisher`. Under
        // concurrent local backlog, `TxPoolPublisher` drains batches in
        // parallel off the compute threads.
        match batch {
            Some(batch) if !batch.is_published() => LocalFastOutcome::WaitPublish { result, batch },
            _ => LocalFastOutcome::Done(Ok(result)),
        }
    }

    /// Execute a direct local submission end-to-end on a dedicated compute OS
    /// thread without `GlobalRt` task spawns or `block_in_place`. Falls back
    /// losslessly to the async path if capacity is contended, verification is
    /// paused, the proof cache is write-locked, or the outbox is full.
    pub(crate) fn try_execute_local_fast(&self, tx: BoundedTransaction) -> LocalFastOutcome {
        if let Err(error) = self.open() {
            return LocalFastOutcome::Done(Err(error));
        }
        let arrival = match self.store.next_arrival() {
            Ok(arrival) => arrival,
            Err(error) => return LocalFastOutcome::Done(Err(error)),
        };
        for _ in 0..=LOCAL_FAST_STALE_RETRIES {
            let (cpu, memory) = match self.try_fast_direct_capacity() {
                Ok(capacity) => capacity,
                Err(FastCapacityError::Contended) => return LocalFastOutcome::RetryCapacity(tx),
                Err(FastCapacityError::Suspended) => return LocalFastOutcome::FallbackAsync(tx),
                Err(FastCapacityError::Other(error)) => return LocalFastOutcome::Done(Err(error)),
            };
            let transaction = tx.as_transaction();
            let prepared = prepare_and_verify_local_with(
                &self.store,
                &self.config,
                &self.cache,
                transaction,
                arrival,
            );
            let (ctx, outcome) = match prepared {
                Ok(pair) => pair,
                Err(Error::Stale) => {
                    drop((cpu, memory));
                    continue;
                }
                Err(Error::Rejected(reject)) => {
                    drop((cpu, memory));
                    return LocalFastOutcome::Done(Ok(Err(reject)));
                }
                Err(error) => {
                    drop((cpu, memory));
                    return match capacity_rejection(error) {
                        Error::Rejected(reject) => LocalFastOutcome::Done(Ok(Err(reject))),
                        error => LocalFastOutcome::Done(Err(error)),
                    };
                }
            };
            drop(cpu);
            let (resolved, verified) = match outcome {
                LocalPrepared::NeedsAsyncVerify(_) => {
                    drop((ctx, memory));
                    return LocalFastOutcome::FallbackAsync(tx);
                }
                LocalPrepared::ResolveRejected(reject, premises) => {
                    let hash = transaction.hash();
                    let committed = ingress::commit_rejection(
                        &self.store,
                        ctx.view,
                        &premises,
                        None,
                        &hash,
                        Source::Local,
                        reject.clone(),
                    );
                    drop((ctx, memory));
                    return match committed {
                        Ok(batch) => self.settle_publish_fast(batch, Err(reject)),
                        Err(Error::Stale) => continue,
                        Err(Error::Full(FullReason::NoticeOutbox)) => {
                            LocalFastOutcome::FallbackAsync(tx)
                        }
                        Err(error) => LocalFastOutcome::Done(Err(error)),
                    };
                }
                LocalPrepared::Verified(verified, resolved) => (resolved, verified),
            };
            if let Err(error) = self.open() {
                drop((ctx, resolved, verified, memory));
                return LocalFastOutcome::Done(Err(error));
            }
            let verified = match verified {
                Ok(verified) => verified,
                Err(Error::Stale) => {
                    drop((ctx, resolved, memory));
                    continue;
                }
                Err(Error::Rejected(reject)) => {
                    let hash = transaction.hash_ref();
                    let mut premises = resolved.premises.clone();
                    if let Err(error) = premises.observe_owner(hash, ctx.before.as_ref()) {
                        drop((ctx, resolved, memory));
                        if matches!(error, Error::Stale) {
                            continue;
                        }
                        return LocalFastOutcome::Done(Err(error));
                    }
                    let committed = ingress::commit_rejection(
                        &self.store,
                        resolved.view,
                        &premises,
                        None,
                        hash,
                        Source::Local,
                        reject.clone(),
                    );
                    drop((ctx, resolved, memory));
                    return match committed {
                        Ok(batch) => self.settle_publish_fast(batch, Err(reject)),
                        Err(Error::Stale) => continue,
                        Err(Error::Full(FullReason::NoticeOutbox)) => {
                            LocalFastOutcome::FallbackAsync(tx)
                        }
                        Err(error) => LocalFastOutcome::Done(Err(error)),
                    };
                }
                Err(error) => {
                    drop((ctx, resolved, memory));
                    return match capacity_rejection(error) {
                        Error::Rejected(reject) => LocalFastOutcome::Done(Ok(Err(reject))),
                        error => LocalFastOutcome::Done(Err(error)),
                    };
                }
            };
            let options = if self.compute.is_local_idle() {
                AdmissionOptions::commit_inline()
            } else {
                AdmissionOptions::commit()
            };
            let committed = (|| {
                self.open()?;
                membership::commit_admission(
                    &self.store,
                    &ctx.candidate,
                    ctx.before.clone(),
                    &verified,
                    &self.config,
                    options,
                )
            })();
            drop((ctx, resolved, verified, memory));
            return match committed {
                Ok((batch, rejection)) => {
                    let result = match rejection {
                        Some(reject) => Err(reject),
                        None => Ok(()),
                    };
                    self.settle_publish_fast(batch, result)
                }
                Err(Error::Stale) => continue,
                Err(Error::Full(FullReason::NoticeOutbox)) => LocalFastOutcome::FallbackAsync(tx),
                Err(error) => match capacity_rejection(error) {
                    Error::Rejected(reject) => LocalFastOutcome::Done(Ok(Err(reject))),
                    error => LocalFastOutcome::Done(Err(error)),
                },
            };
        }
        LocalFastOutcome::FallbackAsync(tx)
    }

    pub(super) async fn ingest(
        &self,
        transaction: Arc<TransactionView>,
        source: Source,
    ) -> Result<(), Error> {
        loop {
            self.open()?;
            match self
                .commit_attempt(|| {
                    self.open()?;
                    ingress::commit_ingress(&self.store, Arc::clone(&transaction), source)
                })
                .await
            {
                Ok(batch) => return self.published(batch).await,
                Err(Error::Stale) => tokio::task::yield_now().await,
                Err(Error::Full(reason)) => {
                    match self
                        .commit_attempt(|| {
                            self.open()?;
                            ingress::commit_capacity_refusal(
                                &self.store,
                                &transaction.hash(),
                                source,
                                reason,
                            )
                        })
                        .await
                    {
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

    async fn ingest_chunked_batch(
        &self,
        items: &[(Arc<TransactionView>, Source)],
    ) -> Result<(), (usize, Error)> {
        let mut completed = 0usize;
        while let Some(rest) = items.get(completed..).filter(|s| !s.is_empty()) {
            if let Err(error) = self.open() {
                return Err((completed, error));
            }
            let chunk = rest
                .get(..rest.len().min(ingress::INGRESS_BATCH_CHUNK))
                .unwrap_or_default();
            match ingress::commit_ingress_chunk(&self.store, chunk) {
                Ok(n) if n > 0 => {
                    completed = completed.saturating_add(n);
                    consume_batch_coop(n).await;
                }
                Ok(_) => {
                    if let Some((tx, source)) = rest.first() {
                        if let Err(error) = self.ingest(Arc::clone(tx), *source).await {
                            return Err((completed, error));
                        }
                        completed = completed.saturating_add(1);
                    }
                    tokio::task::coop::consume_budget().await;
                }
                Err(error) => return Err((completed, error)),
            }
        }
        Ok(())
    }

    pub(crate) async fn submit_remote_batch(
        &self,
        peer: PeerIndex,
        submissions: Vec<(BoundedTransaction, Cycle)>,
    ) -> Result<(), (usize, Error)> {
        let mut items = Vec::with_capacity(submissions.len());
        for (completed, (transaction, cycles)) in submissions.into_iter().enumerate() {
            let source = ingress::remote_source(peer, cycles).map_err(|e| (completed, e))?;
            items.push((transaction.into_transaction(), source));
        }
        self.ingest_chunked_batch(&items).await
    }

    pub(crate) async fn submit_proposal_batch(
        &self,
        transactions: Vec<BoundedTransaction>,
    ) -> Result<(), Error> {
        let items: Vec<_> = transactions
            .into_iter()
            .map(|tx| (tx.into_transaction(), Source::Proposal { remote: None }))
            .collect();
        self.ingest_chunked_batch(&items)
            .await
            .map_err(|(_, error)| error)
    }

    async fn reject_local(
        &self,
        view: u64,
        hash: &Byte32,
        reject: Reject,
        premises: Premises,
        action: AdmissionMode,
        assist_publish: bool,
    ) -> Result<Reject, Error> {
        match action {
            AdmissionMode::Preview => self.store.validate_preview_premises(view, &premises)?,
            AdmissionMode::Commit => {
                let batch = self
                    .commit_attempt(|| {
                        ingress::commit_rejection(
                            &self.store,
                            view,
                            &premises,
                            None,
                            hash,
                            Source::Local,
                            reject.clone(),
                        )
                    })
                    .await?;
                self.settle_publish(batch, assist_publish).await?;
            }
        }
        Ok(reject)
    }

    /// Reply after verification, admission and publication complete.
    pub(crate) async fn submit_local(
        &self,
        transaction: BoundedTransaction,
    ) -> Result<Result<EntryCompleted, Reject>, Error> {
        self.process_local(transaction, AdmissionMode::Commit, false)
            .await
    }

    /// An owned request task may help publish once its commit has appended an
    /// outbox batch, without contending on `outbox` while queued for capacity.
    pub(crate) async fn submit_local_request(
        &self,
        transaction: BoundedTransaction,
    ) -> Result<Result<EntryCompleted, Reject>, Error> {
        self.process_local(transaction, AdmissionMode::Commit, true)
            .await
    }

    /// Verify admission without changing membership or waiting for capacity.
    pub(crate) async fn test_accept(
        &self,
        transaction: BoundedTransaction,
    ) -> Result<Result<EntryCompleted, Reject>, Error> {
        self.process_local(transaction, AdmissionMode::Preview, false)
            .await
    }

    async fn process_local(
        &self,
        transaction: BoundedTransaction,
        action: AdmissionMode,
        assist_publish: bool,
    ) -> Result<Result<EntryCompleted, Reject>, Error> {
        let transaction = transaction.into_transaction();
        let arrival = self.store.next_arrival()?;
        // `_direct` outlives `_memory` inside `attempt_local`, guaranteeing the
        // next FIFO waiter wakes strictly after this submission's `ActivePermit`
        // has been released.
        let _direct = match action {
            AdmissionMode::Commit => {
                match self.acquire_direct_slot().await.map_err(capacity_rejection) {
                    Ok(slot) => Some(slot),
                    Err(Error::Rejected(reject)) => return Ok(Err(reject)),
                    Err(error) => return Err(error),
                }
            }
            AdmissionMode::Preview => None,
        };
        loop {
            match self
                .attempt_local(&transaction, arrival, action, assist_publish)
                .await
            {
                Err(Error::Stale) => continue,
                Err(Error::Rejected(reject)) => return Ok(Err(reject)),
                result => return result.map(Ok),
            }
        }
    }

    async fn attempt_local(
        &self,
        transaction: &Arc<TransactionView>,
        arrival: u64,
        action: AdmissionMode,
        assist_publish: bool,
    ) -> Result<EntryCompleted, Error> {
        self.open()?;
        let capacity = match action {
            AdmissionMode::Commit => self.direct_capacity().await,
            AdmissionMode::Preview => self.try_direct_capacity(),
        };
        let (cpu, memory) = capacity.map_err(capacity_rejection)?;
        let (ctx, outcome, cpu) = {
            let store = Arc::clone(&self.store);
            let config = Arc::clone(&self.config);
            let cache = Arc::clone(&self.cache);
            let tx = Arc::clone(transaction);
            self.compute
                .run_permitted(move || {
                    match prepare_and_verify_local_with(&store, &config, &cache, &tx, arrival) {
                        Ok((ctx, outcome @ LocalPrepared::NeedsAsyncVerify(_))) => {
                            Ok((ctx, outcome, Some(cpu)))
                        }
                        Ok((ctx, outcome)) => {
                            // Release CPU capacity on the compute worker before waking
                            // the async task so the next job starts without a hop.
                            drop(cpu);
                            Ok((ctx, outcome, None))
                        }
                        Err(error) => {
                            drop(cpu);
                            Err(error)
                        }
                    }
                })
                .await
                .map_err(capacity_rejection)?
                .map_err(capacity_rejection)?
        };
        let (resolved, verified) = match outcome {
            LocalPrepared::ResolveRejected(reject, premises) => {
                drop(cpu);
                drop(memory);
                let reject = self
                    .reject_local(
                        ctx.view,
                        &transaction.hash(),
                        reject,
                        premises,
                        action,
                        assist_publish,
                    )
                    .await?;
                return Err(Error::Rejected(reject));
            }
            LocalPrepared::Verified(verified, resolved) => {
                drop(cpu);
                (resolved, verified)
            }
            LocalPrepared::NeedsAsyncVerify(resolved) => {
                let cpu = cpu.ok_or(Error::Fault("missing compute permit for async verify"))?;
                let verified = jobs::verify_with_snapshot(
                    #[cfg(any(test, feature = "internal"))]
                    &self.store,
                    Arc::clone(&ctx.snapshot),
                    &ctx.candidate,
                    Arc::clone(&resolved),
                    &self.config,
                    &self.cache,
                    &mut self.local_commands.clone(),
                    &cpu,
                    Some(&self.compute),
                )
                .await;
                drop(cpu);
                (resolved, verified)
            }
        };
        self.open()?;
        let verified = match verified {
            Ok(verified) => verified,
            Err(Error::Rejected(reject)) => {
                let hash = transaction.hash_ref();
                let view = resolved.view;
                let mut premises = resolved.premises.clone();
                premises.observe_owner(hash, ctx.before.as_ref())?;
                drop((ctx, resolved, memory));
                let reject = self
                    .reject_local(view, hash, reject, premises, action, assist_publish)
                    .await?;
                return Err(Error::Rejected(reject));
            }
            Err(error) => return Err(capacity_rejection(error)),
        };
        let completed = EntryCompleted {
            cycles: verified.cycles(),
            fee: verified.resolved().fee,
        };
        let rejection = match action {
            AdmissionMode::Preview => {
                self.open()?;
                let rejection = membership::commit_admission(
                    &self.store,
                    &ctx.candidate,
                    ctx.before.clone(),
                    &verified,
                    &self.config,
                    AdmissionOptions::preview(),
                )
                .map_err(capacity_rejection)?
                .1;
                drop((ctx, resolved, verified, memory));
                rejection
            }
            AdmissionMode::Commit => {
                let (batch, rejection) = self
                    .commit_attempt(|| {
                        self.open()?;
                        membership::commit_admission(
                            &self.store,
                            &ctx.candidate,
                            ctx.before.clone(),
                            &verified,
                            &self.config,
                            AdmissionOptions::commit(),
                        )
                    })
                    .await
                    .map_err(capacity_rejection)?;
                drop((ctx, resolved, verified, memory));
                self.settle_publish(batch, assist_publish).await?;
                rejection
            }
        };
        match rejection {
            Some(reject) => Err(Error::Rejected(reject)),
            None => Ok(completed),
        }
    }

    /// Acknowledge resolved queue ownership; script verification finishes later.
    pub(crate) async fn enqueue_local_test(
        &self,
        transaction: BoundedTransaction,
    ) -> Result<Result<(), Reject>, Error> {
        let transaction = transaction.into_transaction();
        let arrival = self.store.next_arrival()?;
        let _direct = match self.acquire_direct_slot().await.map_err(capacity_rejection) {
            Ok(slot) => slot,
            Err(Error::Rejected(reject)) => return Ok(Err(reject)),
            Err(error) => return Err(error),
        };
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
        let store = Arc::clone(&self.store);
        let config = Arc::clone(&self.config);
        let tx = Arc::clone(transaction);
        let prepared = self
            .compute
            .run_permitted(move || {
                let prepared = prepare_local_with(&store, &config, &tx, arrival);
                drop(cpu);
                prepared
            })
            .await
            .map_err(capacity_rejection)?;
        let (ctx, resolved) = prepared.map_err(capacity_rejection)?;
        if ctx.before.is_some() {
            return Err(Error::Rejected(Reject::Duplicated(transaction.hash())));
        }
        let resolved = match resolved {
            Ok(resolved) => resolved,
            Err((reject, premises)) => {
                let reject = self
                    .reject_local(
                        ctx.view,
                        &transaction.hash(),
                        reject,
                        premises,
                        AdmissionMode::Commit,
                        false,
                    )
                    .await?;
                return Err(Error::Rejected(reject));
            }
        };
        let hash = ctx.candidate.hash_bytes();
        let batch = self
            .commit_attempt(|| {
                self.open()?;
                let (_, batch) = self.store.commit_with(|tx| {
                    tx.state
                        .validate_premises(resolved.view, &resolved.premises)?;
                    if tx.state.owner(&hash).is_some() {
                        return Err(Error::Stale);
                    }
                    let after = ctx
                        .candidate
                        .try_with_phase(Phase::Verify(Arc::clone(&resolved)))?;
                    tx.commit_single(hash, None, Some(after), Vec::new(), Class::Trusted)
                })?;
                Ok(batch)
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
        let Some(root) = self.store.point(hash).1 else {
            return Ok(Ok(false));
        };
        match self
            .commit_attempt(|| membership::commit_removal(&self.store, &root, &self.config, None))
            .await
        {
            Ok(batch) => {
                self.publish_assisted(batch).await?;
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
        use super::super::model::{OutPointKey, Status};
        let status = match target {
            crate::PlugTarget::Pending => Status::Pending,
            crate::PlugTarget::Proposed => Status::Proposed,
        };
        for entry in entries {
            let transaction = BoundedTransaction::try_new(entry.transaction().clone())
                .map_err(|_| Error::Full("internal transaction materialization".into()))?
                .into_transaction();
            let cell_points: Vec<_> = entry
                .rtx
                .resolved_inputs
                .iter()
                .chain(&entry.rtx.resolved_cell_deps)
                .chain(&entry.rtx.resolved_dep_groups)
                .map(|cell| cell.out_point.clone())
                .collect();
            let point_keys: Vec<_> = cell_points
                .iter()
                .map(OutPointKey::from_out_point)
                .collect();
            loop {
                let (_, _memory) = self.direct_capacity().await?;
                let mut premises = Premises::default();
                let obs = self.store.observe_resolution(
                    Some(transaction.hash_ref()),
                    &point_keys,
                    &mut premises,
                )?;
                if obs.before.as_ref().is_some_and(|e| e.accepted().is_some()) {
                    break;
                }
                if obs.before.is_some() {
                    return Err(Error::Full(
                        "internal insertion would displace an owner".into(),
                    ));
                }
                let pool_cells: BTreeSet<_> = cell_points
                    .iter()
                    .zip(&point_keys)
                    .filter(|(_, key)| obs.prefetched.get(key).is_some_and(Option::is_some))
                    .map(|(point, _)| point.clone())
                    .collect();
                let verified = jobs::fixture(
                    obs.view,
                    &entry,
                    Arc::clone(&transaction),
                    premises,
                    pool_cells,
                    status,
                );
                let candidate = Entry::try_new(
                    Arc::clone(&transaction),
                    self.store.next_arrival()?,
                    Source::Local,
                    Phase::Resolve,
                )?;
                let options = membership::AdmissionOptions {
                    retain_history: false,
                    dry_run: false,
                    inline_publish: false,
                    silence_effects: true,
                    forbid_displacement: true,
                };
                match self
                    .commit_attempt(|| {
                        let (batch, reject) = membership::commit_admission(
                            &self.store,
                            &candidate,
                            None,
                            &verified,
                            &self.config,
                            options,
                        )?;
                        reject.map_or(Ok(batch), |r| Err(Error::Rejected(r)))
                    })
                    .await
                {
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
