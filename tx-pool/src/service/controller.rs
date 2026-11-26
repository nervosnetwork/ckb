//! Direct transaction-pool controller backed by `Weak<Pool>`, `RequestGate`,
//! and the sole ordered `ChainControl` lane.

use crate::authority::service::{
    Batch, Error, LocalFastOutcome, LocalStep, LocalTask, PauseScope, Pool, PushErrorKind,
    VerificationControl,
};
use crate::block_assembler::BoundedCandidateUncle;
use crate::error::{handle_recv_error, handle_send_cmd_error};
use crate::service::gate::RequestPermit;
use crate::service::message::SyncRequest;
use crate::service::{
    AdministrationGate, AdmissionLane, AdmittedAdministration, BlockTemplateResult,
    BoundedProposalIds, BoundedTransaction, BoundedTransactionError, BoundedTransactionHashes,
    ChainControl, ChainReorgArgs, ChainReorgPayloadLimit, FeeEstimatesResult,
    FetchTxsWithCyclesResult, GetTransactionWithStatusResult, GetTxStatusResult, NotifyTxBatch,
    RemoteTxBatchOutcome, RemoteTxSubmissionBatch, Request, RequestGate, SubmitTxResult,
    TestAcceptTxResult,
};
use crate::util::block_offload;
use ckb_async_runtime::Handle;
use ckb_channel::oneshot;
use ckb_error::{AnyError, OtherError};
use ckb_logger::info;
use ckb_network::PeerIndex;
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{
        BlockView, Cycle, EstimateMode, TransactionView, UncleBlockView, Version,
        cell::CellStatus,
        tx_pool::{PoolTxDetailInfo, TxPoolEntryInfo, TxPoolIds, TxPoolInfo},
    },
    packed::{Byte32, OutPoint, ProposalShortId},
};
use futures_util::FutureExt;
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Weak};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[cfg(feature = "internal")]
use crate::{PlugTarget, component::entry::TxEntry};

/// Cloneable handle to one tx-pool service generation.
///
/// Holds `Weak<Pool>` so external handles never extend generation lifetime
/// past runtime shutdown. Pure reads and mutations execute directly against
/// `Pool` under `RequestGate` backpressure and drain control; only causal
/// chain/administration commands cross the bounded ordered `ChainControl` lane.
#[derive(Clone)]
pub struct TxPoolController {
    pub(crate) pool: Weak<Pool>,
    pub(crate) gate: Arc<RequestGate>,
    pub(crate) chain_control_sender: mpsc::Sender<ChainControl>,
    pub(crate) verification_command: VerificationControl,
    pub(crate) handle: Handle,
    pub(crate) administration_gate: AdministrationGate,
    pub(crate) chain_reorg_payload_limit: ChainReorgPayloadLimit,
    pub(crate) candidate_uncle_payload_limit: usize,
    pub(crate) signal: CancellationToken,
}

macro_rules! send_admitted_chain_control {
    ($self:ident, $command:ident, $args:expr) => {{
        let admission = $self
            .administration_gate
            .try_acquire()
            .ok_or_else(|| -> AnyError {
                OtherError::new("another tx-pool administration is already admitted".to_owned())
                    .into()
            })?;
        $self.send_chain_control($args, |request| {
            ChainControl::$command(AdmittedAdministration::new(admission, request))
        })
    }};
}

macro_rules! reject_callback_mutation {
    ($operation:literal) => {
        if crate::callback::in_callback() {
            return Err(OtherError::new(format!(
                "tx-pool callback cannot synchronously invoke mutating controller operation {}",
                $operation
            ))
            .into());
        }
    };
}

fn ingress_allocation_error() -> AnyError {
    OtherError::new("tx-pool transaction ingress allocation unavailable".to_owned()).into()
}

fn bounded_direct_transaction(
    transaction: TransactionView,
) -> Result<Result<BoundedTransaction, ckb_types::core::tx_pool::Reject>, AnyError> {
    match BoundedTransaction::try_new(transaction) {
        Ok(transaction) => Ok(Ok(transaction)),
        Err(BoundedTransactionError::TooLarge { actual, maximum }) => Ok(Err(
            ckb_types::core::tx_pool::Reject::ExceededTransactionSizeLimit(actual, maximum),
        )),
        Err(BoundedTransactionError::Allocation) => Err(ingress_allocation_error()),
    }
}

fn escalate_fault(pool: &Pool, gate: &RequestGate, signal: &CancellationToken) {
    pool.fault();
    gate.close();
    signal.cancel();
}

fn fault_on_unwind(pool: &Pool, gate: &RequestGate, signal: &CancellationToken) -> AnyError {
    crate::metrics::record_failure(crate::metrics::FailureBoundary::HandlerUnwind);
    escalate_fault(pool, gate, signal);
    Error::Fault("tx-pool request handler panicked").into()
}

fn settle_error<T, E: Into<AnyError>>(
    pool: &Pool,
    gate: &RequestGate,
    signal: &CancellationToken,
    result: Result<T, E>,
) -> Result<T, AnyError> {
    result.map_err(|error| {
        let error: AnyError = error.into();
        if matches!(error.downcast_ref::<Error>(), Some(Error::Fault(_))) {
            escalate_fault(pool, gate, signal);
        }
        error
    })
}

fn run_sync_guarded<T, E: Into<AnyError>>(
    pool: &Pool,
    gate: &RequestGate,
    signal: &CancellationToken,
    operation: impl FnOnce() -> Result<T, E>,
) -> Result<T, AnyError> {
    match std::panic::catch_unwind(AssertUnwindSafe(operation)) {
        Ok(result) => settle_error(pool, gate, signal, result),
        Err(_) => Err(fault_on_unwind(pool, gate, signal)),
    }
}

async fn run_async_guarded<T, E: Into<AnyError>>(
    pool: &Pool,
    gate: &RequestGate,
    signal: &CancellationToken,
    operation: impl Future<Output = Result<T, E>>,
) -> Result<T, AnyError> {
    match AssertUnwindSafe(operation).catch_unwind().await {
        Ok(result) => settle_error(pool, gate, signal, result),
        Err(_) => Err(fault_on_unwind(pool, gate, signal)),
    }
}

fn upgrade_pool(pool: &Weak<Pool>) -> Result<Arc<Pool>, AnyError> {
    pool.upgrade().ok_or_else(|| Error::Closed.into())
}

/// Owned direct-execution context moved into spawned or `'static` request futures.
struct DirectContext {
    pool: Weak<Pool>,
    gate: Arc<RequestGate>,
    signal: CancellationToken,
}

impl DirectContext {
    fn upgrade(&self) -> Result<Arc<Pool>, AnyError> {
        upgrade_pool(&self.pool)
    }

    fn run_sync<T, E: Into<AnyError>>(
        &self,
        pool: &Pool,
        operation: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, AnyError> {
        run_sync_guarded(pool, &self.gate, &self.signal, operation)
    }

    async fn run_async<T, E: Into<AnyError>>(
        &self,
        pool: &Pool,
        operation: impl Future<Output = Result<T, E>>,
    ) -> Result<T, AnyError> {
        run_async_guarded(pool, &self.gate, &self.signal, operation).await
    }
}

impl TxPoolController {
    fn direct_context(&self) -> DirectContext {
        DirectContext {
            pool: Weak::clone(&self.pool),
            gate: Arc::clone(&self.gate),
            signal: self.signal.clone(),
        }
    }

    fn wait_ready_blocking(&self, allow_prestart: bool) -> Result<(), AnyError> {
        if allow_prestart || self.gate.is_open() {
            if self.gate.is_closed() {
                return Err(crate::service::gate::closed_gate_error());
            }
            return Ok(());
        }
        block_offload(|| self.handle.block_on(self.gate.wait_ready(false)))
    }

    fn run_read_sync_nested<T, E: Into<AnyError>>(
        &self,
        operation: impl FnOnce(&Pool) -> Result<T, E>,
    ) -> Result<Result<T, AnyError>, AnyError> {
        let callback = crate::callback::in_callback();
        let _permit = self.gate.try_admit(AdmissionLane::for_read(callback))?;
        self.wait_ready_blocking(callback)?;
        let pool = upgrade_pool(&self.pool)?;
        Ok(run_sync_guarded(&pool, &self.gate, &self.signal, || {
            operation(&pool)
        }))
    }

    fn run_read_sync<T, E: Into<AnyError>>(
        &self,
        operation: impl FnOnce(&Pool) -> Result<T, E>,
    ) -> Result<T, AnyError> {
        self.run_read_sync_nested(operation)?
    }

    async fn run_read_async<T, E: Into<AnyError>>(
        &self,
        operation: impl FnOnce(&Pool) -> Result<T, E>,
    ) -> Result<T, AnyError> {
        let callback = crate::callback::in_callback();
        let _permit = self.gate.try_admit(AdmissionLane::for_read(callback))?;
        self.gate.wait_ready(callback).await?;
        let pool = upgrade_pool(&self.pool)?;
        run_sync_guarded(&pool, &self.gate, &self.signal, || operation(&pool))
    }

    fn run_blocking_async<T, E, F, Fut>(
        &self,
        lane: AdmissionLane,
        allow_prestart: bool,
        operation: F,
    ) -> Result<T, AnyError>
    where
        E: Into<AnyError>,
        F: FnOnce(Arc<Pool>) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let _permit = self.gate.try_admit(lane)?;
        let direct = self.direct_context();
        block_offload(|| {
            self.handle.block_on(async move {
                direct.gate.wait_ready(allow_prestart).await?;
                let pool = direct.upgrade()?;
                direct.run_async(&pool, operation(Arc::clone(&pool))).await
            })
        })
    }

    fn send_chain_control<A>(
        &self,
        arguments: A,
        command: impl FnOnce(SyncRequest<A, ()>) -> ChainControl,
    ) -> Result<(), AnyError> {
        let (responder, response) = oneshot::channel();
        let command = command(Request::call(arguments, responder));
        // Chain completion must remain available while transaction operations
        // await Resume. Backpressure preserves every ordered control, even
        // when the bounded lane is occupied by an earlier transition.
        block_offload(|| {
            self.handle
                .block_on(self.chain_control_sender.send(command))
        })
        .map_err(|error| OtherError::new(format!("send ordered chain control fails: {error}")))?;
        block_offload(|| response.recv())
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }

    /// Return whether tx-pool service is started
    pub fn service_started(&self) -> bool {
        self.gate.is_open()
    }

    /// Whether chain publication still has an owner, including before startup.
    /// Chain-only import/replay drops the pool builder and closes this lane.
    pub fn accepts_chain_updates(&self) -> bool {
        !self.chain_control_sender.is_closed()
    }

    /// Return reference of tokio runtime handle
    pub fn handle(&self) -> &Handle {
        &self.handle
    }

    /// Request graceful shutdown of the service and its background workers.
    /// Returning only confirms cancellation was signalled; workers may still be joining.
    pub fn stop(&self) {
        self.signal.cancel();
    }

    /// Return a block template within one deadline, including startup/refresh waits.
    pub fn get_block_template(
        &self,
        _bytes_limit: Option<u64>,
        _proposals_limit: Option<u64>,
        _max_version: Option<Version>,
    ) -> Result<BlockTemplateResult, AnyError> {
        let deadline = tokio::time::Instant::now()
            .checked_add(crate::constants::BLOCK_TEMPLATE_TIMEOUT)
            .ok_or_else(|| OtherError::new("block template deadline overflow".to_owned()))?;
        self.get_block_template_at(deadline)
    }

    pub(crate) fn get_block_template_at(
        &self,
        deadline: tokio::time::Instant,
    ) -> Result<BlockTemplateResult, AnyError> {
        let callback = crate::callback::in_callback();
        let permit = self.gate.try_admit(AdmissionLane::for_ordinary(callback))?;
        if tokio::time::Instant::now() >= deadline {
            drop(permit);
            return if !callback && !self.gate.is_open() {
                Err(
                    OtherError::new("block template response unavailable: timed out".to_owned())
                        .into(),
                )
            } else {
                Ok(Err(Error::Full("template refresh timeout".into()).into()))
            };
        }
        let direct = self.direct_context();
        let (reply_tx, reply_rx) =
            tokio::sync::oneshot::channel::<Result<BlockTemplateResult, AnyError>>();
        // Run validation, refresh and response construction inside one bounded
        // task that retains `permit` until work finishes. The caller awaits
        // `reply_rx` independently against the same absolute `deadline`; if the
        // caller exits early on timeout, `permit` stays held through completion
        // (preserving shutdown drain and lane accounting) and the late response
        // is discarded when `reply_tx.send` sees the receiver closed.
        self.handle.spawn(async move {
            let permit = permit;
            match tokio::time::timeout_at(deadline, direct.gate.wait_ready(callback)).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    drop(permit);
                    let _ = reply_tx.send(Err(error));
                    return;
                }
                Err(_) => {
                    drop(permit);
                    let _ = reply_tx.send(Err(OtherError::new(
                        "block template response unavailable: timed out".to_owned(),
                    )
                    .into()));
                    return;
                }
            }
            let pool = match direct.upgrade() {
                Ok(pool) => pool,
                Err(error) => {
                    drop(permit);
                    let _ = reply_tx.send(Err(error));
                    return;
                }
            };
            let result = direct.run_async(&pool, pool.block_template(deadline)).await;
            let settled = match result {
                Ok(_template) if tokio::time::Instant::now() >= deadline => {
                    Ok(Err(Error::Full("template refresh timeout".into()).into()))
                }
                Ok(template) => Ok(Ok(template)),
                Err(error) => Ok(Err(error)),
            };
            // Release admission/drain accounting before signalling the caller
            // so callers observing a normal completion see `inflight` reclaimed.
            drop(permit);
            let _ = reply_tx.send(settled);
        });
        block_offload(|| {
            self.handle.block_on(async move {
                match tokio::time::timeout_at(deadline, reply_rx).await {
                    Ok(Ok(Ok(Ok(_)))) if tokio::time::Instant::now() >= deadline => {
                        Ok(Err(Error::Full("template refresh timeout".into()).into()))
                    }
                    Ok(Ok(response)) => response,
                    Ok(Err(_)) => Err(handle_recv_error(oneshot::RecvError).into()),
                    Err(_) => {
                        if !callback && !self.gate.is_open() {
                            Err(OtherError::new(
                                "block template response unavailable: timed out".to_owned(),
                            )
                            .into())
                        } else {
                            Ok(Err(Error::Full("template refresh timeout".into()).into()))
                        }
                    }
                }
            })
        })
    }

    /// Notify new uncle
    pub fn notify_new_uncle(&self, uncle: UncleBlockView) -> Result<(), AnyError> {
        let uncle = BoundedCandidateUncle::try_new(uncle, self.candidate_uncle_payload_limit)
            .map_err(|error| {
                OtherError::new(format!(
                    "tx-pool candidate-uncle ingress rejected: {error:?}"
                ))
            })?;
        let permit = self.gate.try_admit(AdmissionLane::Ordinary)?;
        if self.gate.is_open() {
            let pool = upgrade_pool(&self.pool)?;
            run_sync_guarded(&pool, &self.gate, &self.signal, || {
                pool.uncle(uncle);
                Ok::<(), Error>(())
            })?;
            drop(permit);
            return Ok(());
        }
        let direct = self.direct_context();
        self.handle.spawn(async move {
            let _permit = permit;
            if direct.gate.wait_ready(false).await.is_ok()
                && let Some(pool) = direct.pool.upgrade()
            {
                let _ = direct.run_sync(&pool, || {
                    pool.uncle(uncle);
                    Ok::<(), Error>(())
                });
            }
        });
        Ok(())
    }

    /// Make tx-pool consistent after a reorg, by re-adding or recursively erasing
    /// detached block transactions from the tx-pool, and also removing any
    /// other transactions from the tx-pool that are no longer valid given the new
    /// tip/height.
    pub fn update_tx_pool_for_reorg(
        &self,
        detached_blocks: VecDeque<BlockView>,
        attached_blocks: VecDeque<BlockView>,
        snapshot: Arc<Snapshot>,
    ) -> Result<(), AnyError> {
        reject_callback_mutation!("update_tx_pool_for_reorg");
        self.send_chain_control(
            ChainReorgArgs::bounded(
                detached_blocks,
                attached_blocks,
                snapshot,
                self.chain_reorg_payload_limit,
            ),
            ChainControl::Reconcile,
        )
    }

    /// Submit local tx to tx-pool
    pub fn submit_local_tx(&self, tx: TransactionView) -> Result<SubmitTxResult, AnyError> {
        let mut submission = std::pin::pin!(self.submit_local_tx_async(tx));
        // Admission refusals need no blocking runtime boundary. Only an
        // admitted operation whose response is still pending needs to wait.
        match submission.as_mut().now_or_never() {
            Some(result) => result,
            None => block_offload(|| self.handle.block_on(submission)),
        }
    }

    /// Submit a local transaction and await verification, admission and publication.
    ///
    /// Dropping this future after admission abandons only its reply, not the
    /// admitted operation (matching the HTTP 408 timeout contract).
    pub async fn submit_local_tx_async(
        &self,
        tx: TransactionView,
    ) -> Result<SubmitTxResult, AnyError> {
        enum LocalFastReply {
            Done(Result<SubmitTxResult, AnyError>),
            WaitPublish {
                result: SubmitTxResult,
                batch: Arc<Batch>,
                permit: RequestPermit,
            },
        }
        reject_callback_mutation!("submit_local_tx");
        let tx = match bounded_direct_transaction(tx)? {
            Ok(tx) => tx,
            Err(reason) => return Ok(Err(reason)),
        };
        let permit = self.gate.try_admit(AdmissionLane::Ordinary)?;
        let direct = self.direct_context();
        if self.gate.is_open()
            && let Some(pool) = self.pool.upgrade()
        {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel::<LocalFastReply>();
            let worker_pool = Arc::clone(&pool);
            let handle = self.handle.clone();
            let mut pending = Some((tx, permit, reply_tx, direct, handle));
            let job: LocalTask = Box::new(move || {
                let Some((tx, permit, reply_tx, direct, handle)) = pending.take() else {
                    return LocalStep::Finished;
                };
                match std::panic::catch_unwind(AssertUnwindSafe(|| {
                    worker_pool.try_execute_local_fast(tx)
                })) {
                    Ok(LocalFastOutcome::Done(result)) => {
                        let settled =
                            settle_error(&worker_pool, &direct.gate, &direct.signal, result);
                        drop(permit);
                        let _ = reply_tx.send(LocalFastReply::Done(settled));
                        LocalStep::Finished
                    }
                    Ok(LocalFastOutcome::WaitPublish { result, batch }) => {
                        // Transfer `permit` back to the awaiting caller. If the
                        // caller already dropped `reply_rx` (HTTP 408), `permit`
                        // drops immediately while `batch` is already committed
                        // in `Outbox` and guaranteed to publish before shutdown.
                        let _ = reply_tx.send(LocalFastReply::WaitPublish {
                            result,
                            batch,
                            permit,
                        });
                        LocalStep::Finished
                    }
                    Ok(LocalFastOutcome::RetryCapacity(tx)) => {
                        pending = Some((tx, permit, reply_tx, direct, handle));
                        LocalStep::RetryOnCapacity
                    }
                    Ok(LocalFastOutcome::FallbackAsync(tx)) => {
                        let fallback_pool = Arc::clone(&worker_pool);
                        handle.spawn(async move {
                            let _permit = permit;
                            let res = direct
                                .run_async(&fallback_pool, async {
                                    fallback_pool
                                        .submit_local_request(tx)
                                        .await
                                        .map(|result| result.map(|_| ()))
                                })
                                .await;
                            let _ = reply_tx.send(LocalFastReply::Done(res));
                        });
                        LocalStep::Finished
                    }
                    Err(_) => {
                        let error = fault_on_unwind(&worker_pool, &direct.gate, &direct.signal);
                        drop(permit);
                        let _ = reply_tx.send(LocalFastReply::Done(Err(error)));
                        LocalStep::Finished
                    }
                }
            });
            return match pool.push_local_fast(job) {
                Ok(()) => match reply_rx.await {
                    Ok(LocalFastReply::Done(result)) => result,
                    Ok(LocalFastReply::WaitPublish {
                        result,
                        batch,
                        permit,
                    }) => {
                        let _permit = permit;
                        settle_error(
                            &pool,
                            &self.gate,
                            &self.signal,
                            pool.wait_batch(batch).await,
                        )?;
                        Ok(result)
                    }
                    Err(_) => Err(AnyError::from(handle_recv_error(oneshot::RecvError))),
                },
                Err((PushErrorKind::Closed, job)) => {
                    drop(job);
                    Err(crate::service::gate::closed_gate_error())
                }
                Err((PushErrorKind::WorkerSpawnFailed, job)) => {
                    drop(job);
                    Err(Error::Full("compute worker unavailable".into()).into())
                }
            };
        }
        let task = self.handle.spawn(async move {
            let _permit = permit;
            direct.gate.wait_ready(false).await?;
            let pool = direct.upgrade()?;
            direct
                .run_async(&pool, async {
                    pool.submit_local_request(tx)
                        .await
                        .map(|result| result.map(|_| ()))
                })
                .await
        });
        task.await
            .map_err(|_| AnyError::from(handle_recv_error(oneshot::RecvError)))?
    }

    /// Check admission without inserting the transaction or relaying it.
    pub fn test_accept_tx(&self, tx: TransactionView) -> Result<TestAcceptTxResult, AnyError> {
        let tx = match bounded_direct_transaction(tx)? {
            Ok(tx) => tx,
            Err(reason) => return Ok(Err(reason)),
        };
        let callback = crate::callback::in_callback();
        self.run_blocking_async(
            AdmissionLane::for_ordinary(callback),
            callback,
            move |pool| async move { pool.test_accept(tx).await },
        )
    }

    /// Remove tx from tx-pool
    pub fn remove_local_tx(&self, tx_hash: Byte32) -> Result<bool, AnyError> {
        reject_callback_mutation!("remove_local_tx");
        self.run_blocking_async(AdmissionLane::Ordinary, false, move |pool| async move {
            pool.remove_local(&tx_hash)
                .await
                .map_err(AnyError::from)
                .and_then(|result| result.map_err(AnyError::from))
        })
    }

    /// Submit remote tx with declared cycles and origin to tx-pool
    pub async fn submit_remote_tx(
        &self,
        tx: TransactionView,
        declared_cycles: Cycle,
        peer: PeerIndex,
    ) -> Result<(), AnyError> {
        reject_callback_mutation!("submit_remote_tx");
        let transaction = bounded_direct_transaction(tx)?.map_err(AnyError::from)?;
        let permit = self.gate.try_admit(AdmissionLane::Ordinary)?;
        let direct = self.direct_context();
        let task = self.handle.spawn(async move {
            let _permit = permit;
            direct.gate.wait_ready(false).await?;
            let pool = direct.upgrade()?;
            direct
                .run_async(
                    &pool,
                    pool.submit_remote(transaction, declared_cycles, peer),
                )
                .await
        });
        task.await
            .map_err(|_| AnyError::from(handle_recv_error(oneshot::RecvError)))?
    }

    /// Submit one already-bounded network relay batch through one bounded
    /// admission slot.
    ///
    /// The service handles a canonical input prefix, including rejection notices.
    /// Completion does not mean the transactions passed verification or entered
    /// the accepted pool. Validation and non-blocking admission finish before
    /// this method returns; admitted work is owned by the service runtime so
    /// holding or dropping the returned response future never pins permits or
    /// cancels admitted work.
    pub fn submit_remote_txs(
        &self,
        submissions: Vec<(TransactionView, Cycle)>,
        peer: PeerIndex,
    ) -> Result<
        impl Future<Output = Result<RemoteTxBatchOutcome, AnyError>> + Send + 'static,
        AnyError,
    > {
        reject_callback_mutation!("submit_remote_txs");
        #[cfg(feature = "profiling")]
        let _batch_span = tracing::trace_span!(
            target: "ckb_tx_pool_profile",
            "tx_pool.ingress.remote_batch"
        )
        .entered();
        let task = if submissions.is_empty() {
            None
        } else {
            let batch = RemoteTxSubmissionBatch::try_new(submissions, peer)?;
            let permit = self.gate.try_admit(AdmissionLane::Ordinary)?;
            let direct = self.direct_context();
            Some(self.handle.spawn(async move {
                let _permit = permit;
                direct.gate.wait_ready(false).await?;
                let pool = direct.upgrade()?;
                let (peer, submissions) = batch.into_parts();
                let offered = submissions.len();
                match AssertUnwindSafe(pool.submit_remote_batch(peer, submissions))
                    .catch_unwind()
                    .await
                {
                    Ok(Ok(())) => Ok(RemoteTxBatchOutcome::complete(offered)),
                    Ok(Err((completed, error))) => {
                        if matches!(error, Error::Fault(_)) {
                            escalate_fault(&pool, &direct.gate, &direct.signal);
                        }
                        Ok(RemoteTxBatchOutcome::failed(
                            offered,
                            completed,
                            error.into(),
                        ))
                    }
                    Err(_) => {
                        let error = fault_on_unwind(&pool, &direct.gate, &direct.signal);
                        Ok(RemoteTxBatchOutcome::failed(offered, 0, error))
                    }
                }
            }))
        };
        Ok(async move {
            match task {
                Some(task) => task
                    .await
                    .map_err(|_| AnyError::from(handle_recv_error(oneshot::RecvError)))?,
                None => Ok(RemoteTxBatchOutcome::complete(0)),
            }
        })
    }

    /// Admit bounded transaction notifications for asynchronous proposal ingress.
    /// Success reports admission under `RequestGate`, not completion of verification.
    pub fn notify_txs(&self, txs: Vec<TransactionView>) -> Result<(), AnyError> {
        let batch = NotifyTxBatch::try_new(txs)?;
        let permit = self.gate.try_admit(AdmissionLane::Ordinary)?;
        let direct = self.direct_context();
        self.handle.spawn(async move {
            let _permit = permit;
            if direct.gate.wait_ready(false).await.is_ok()
                && let Some(pool) = direct.pool.upgrade()
            {
                let _ = direct
                    .run_async(&pool, pool.submit_proposal_batch(batch.into_transactions()))
                    .await;
            }
        });
        Ok(())
    }

    /// Admit bounded transaction notifications for asynchronous proposal ingress.
    /// Success reports admission under `RequestGate`, not completion of verification.
    pub async fn notify_txs_async(&self, txs: Vec<TransactionView>) -> Result<(), AnyError> {
        self.notify_txs(txs)
    }

    /// Return tx-pool information
    pub fn get_tx_pool_info(&self) -> Result<TxPoolInfo, AnyError> {
        self.run_read_sync(|pool| pool.pool_info_sync())
    }

    /// Return cell status from the pool and chain snapshot.
    pub fn get_live_cell(
        &self,
        out_point: OutPoint,
        with_data: bool,
    ) -> Result<CellStatus, AnyError> {
        self.run_read_sync(|pool| pool.live_cell(&out_point, with_data))
    }

    /// Return fresh proposals
    pub async fn fresh_proposals_filter(
        &self,
        proposals: Vec<ProposalShortId>,
    ) -> Result<Vec<ProposalShortId>, AnyError> {
        let bounded = BoundedProposalIds::try_from_vec(proposals)?;
        self.run_read_async(|pool| pool.fresh_proposals(bounded.into_vec()))
            .await
    }

    /// Return tx_status for rpc (get_transaction verbosity = 1)
    pub fn get_tx_status(&self, hash: Byte32) -> Result<GetTxStatusResult, AnyError> {
        self.run_read_sync_nested(|pool| pool.transaction_status(&hash))
    }

    /// Return transaction_with_status for rpc (get_transaction verbosity = 2)
    pub fn get_transaction_with_status(
        &self,
        hash: Byte32,
    ) -> Result<GetTransactionWithStatusResult, AnyError> {
        self.run_read_sync_nested(|pool| pool.transaction_blocking(&self.handle, &hash))
    }

    /// Mainly used for compact block reconstruction and block proposal pre-broadcasting
    /// Orphan/conflicted/etc transactions that are returned for compact block reconstruction.
    pub async fn fetch_txs(
        &self,
        short_ids: HashSet<ProposalShortId>,
    ) -> Result<HashMap<ProposalShortId, TransactionView>, AnyError> {
        let bounded = BoundedProposalIds::try_from_set(short_ids)?;
        self.run_read_async(|pool| pool.compact_transactions(&bounded.into_vec()))
            .await
    }

    /// Return accepted transactions with cycles by complete raw transaction
    /// hash. This identity is distinct from compact-block proposal IDs.
    pub async fn fetch_txs_with_cycles(
        &self,
        tx_hashes: HashSet<Byte32>,
    ) -> Result<FetchTxsWithCyclesResult, AnyError> {
        let bounded = BoundedTransactionHashes::try_from_set(tx_hashes)?;
        self.run_read_async(|pool| pool.accepted_with_cycles(&bounded.into_vec()))
            .await
    }

    /// Clears the tx-pool, removing all txs, update snapshot.
    pub fn clear_pool(&self, new_snapshot: Arc<Snapshot>) -> Result<(), AnyError> {
        reject_callback_mutation!("clear_pool");
        send_admitted_chain_control!(self, ClearPool, new_snapshot)
    }

    /// Clears every preaccepted transaction and retained replacement history without
    /// touching the already-accepted pool. The method name is retained for
    /// controller API compatibility.
    pub fn clear_verify_queue(&self) -> Result<(), AnyError> {
        reject_callback_mutation!("clear_verify_queue");
        send_admitted_chain_control!(self, ClearPipeline, ())
    }

    /// Returns information about all transactions in the pool.
    pub fn get_all_entry_info(&self) -> Result<TxPoolEntryInfo, AnyError> {
        self.run_read_sync(|pool| pool.entry_info_blocking(&self.handle))
    }

    /// Returns the IDs of all transactions in the pool.
    pub fn get_all_ids(&self) -> Result<TxPoolIds, AnyError> {
        self.run_read_sync(|pool| pool.pool_ids_blocking(&self.handle))
    }

    /// Capture accepted inputs and their chain tip for a rebuildable read view.
    /// Unchanged membership reuses the same immutable snapshot.
    pub fn input_snapshot(&self) -> Result<crate::TxPoolInputSnapshot, AnyError> {
        self.run_read_sync(|pool| pool.input_snapshot_blocking(&self.handle))
    }

    /// query the details of a transaction in the pool
    pub fn get_tx_detail(&self, tx_hash: Byte32) -> Result<PoolTxDetailInfo, AnyError> {
        self.run_read_sync(|pool| pool.detail_blocking(&self.handle, &tx_hash))
    }

    /// Saves tx pool into disk.
    pub fn save_pool(&self) -> Result<(), AnyError> {
        reject_callback_mutation!("save_pool");
        info!("Please be patient, tx-pool are saving data into disk ...");
        self.run_blocking_async(AdmissionLane::Ordinary, false, |pool| async move {
            pool.save().await
        })
    }

    /// Updates IBD state.
    pub fn update_ibd_state(&self, in_ibd: bool) -> Result<(), AnyError> {
        reject_callback_mutation!("update_ibd_state");
        self.send_chain_control(in_ibd, ChainControl::UpdateIBDState)
    }

    /// Estimates fee rate.
    pub fn estimate_fee_rate(
        &self,
        estimate_mode: EstimateMode,
        enable_fallback: bool,
    ) -> Result<FeeEstimatesResult, AnyError> {
        self.run_read_sync_nested(|pool| {
            pool.estimate_fee_blocking(&self.handle, estimate_mode, enable_fallback)
        })
    }

    fn set_verification_paused(&self, scope: PauseScope, paused: bool) -> Result<(), AnyError> {
        self.verification_command
            .set_paused(scope, paused)
            .map_err(handle_send_cmd_error)
            .map_err(Into::into)
    }

    /// Pause new pool computation and request suspension of running pool VMs.
    /// Does not wait for in-flight computation to become idle.
    pub fn suspend_chunk_process(&self) -> Result<(), AnyError> {
        self.set_verification_paused(PauseScope::External, true)
    }

    /// Resume pool computation unless this service generation has stopped.
    pub fn continue_chunk_process(&self) -> Result<(), AnyError> {
        self.set_verification_paused(PauseScope::External, false)
    }

    /// Pause background pool queue verification across a queued chain backlog
    /// without stalling local RPC submissions during single-threaded chain work.
    pub fn suspend_background_verification(&self) -> Result<(), AnyError> {
        self.set_verification_paused(PauseScope::Background, true)
    }

    /// Release the background pool queue pause when chain backlog drains.
    pub fn resume_background_verification(&self) -> Result<(), AnyError> {
        self.set_verification_paused(PauseScope::Background, false)
    }

    /// Pause both background and local pool VM execution while parallel block
    /// script verification runs.
    pub fn suspend_block_script_verification(&self) -> Result<(), AnyError> {
        self.set_verification_paused(PauseScope::BlockScripts, true)
    }

    /// Release the block-script pool VM pause after contextual block
    /// verification returns.
    pub fn resume_block_script_verification(&self) -> Result<(), AnyError> {
        self.set_verification_paused(PauseScope::BlockScripts, false)
    }

    /// Plug tx-pool entry to tx-pool, skip verification. only for test
    #[cfg(feature = "internal")]
    pub fn plug_entry(&self, entries: Vec<TxEntry>, target: PlugTarget) -> Result<(), AnyError> {
        reject_callback_mutation!("plug_entry");
        self.run_blocking_async(AdmissionLane::Ordinary, false, move |pool| async move {
            pool.plug(entries, target).await
        })
    }

    /// Package txs with specified bytes_limit. for test
    #[cfg(feature = "internal")]
    pub fn package_txs(&self, bytes_limit: Option<u64>) -> Result<Vec<TxEntry>, AnyError> {
        let callback = crate::callback::in_callback();
        self.run_blocking_async(
            AdmissionLane::for_ordinary(callback),
            callback,
            move |pool| async move { pool.package_transactions(bytes_limit).await },
        )
    }

    /// Resolve a transaction through the integration-test RPC and enqueue its
    /// script verification. Success acknowledges the queue, not final validity.
    pub fn submit_local_test_tx(&self, tx: TransactionView) -> Result<SubmitTxResult, AnyError> {
        reject_callback_mutation!("submit_local_test_tx");
        let tx = match bounded_direct_transaction(tx)? {
            Ok(tx) => tx,
            Err(reason) => return Ok(Err(reason)),
        };
        self.run_blocking_async(AdmissionLane::Ordinary, false, move |pool| async move {
            pool.enqueue_local_test(tx).await
        })
    }

    /// Install or clear a verification observer hook for cross-crate ordering tests.
    #[cfg(feature = "internal")]
    pub fn set_verify_observer(
        &self,
        hook: Option<crate::authority::service::VerifyObserverHook>,
    ) -> Result<(), AnyError> {
        let pool = upgrade_pool(&self.pool)?;
        pool.set_verify_observer(hook);
        Ok(())
    }

    /// Observe current in-flight ordinary requests admitted through `RequestGate`.
    #[cfg(feature = "internal")]
    pub fn inflight_requests(&self) -> usize {
        self.gate.inflight()
    }

    /// get total recent reject num
    pub fn get_total_recent_reject_num(&self) -> Result<Option<u64>, AnyError> {
        self.run_read_sync(|pool| {
            pool.open()?;
            Ok::<_, Error>(pool.recent_count())
        })
    }
}

#[cfg(test)]
#[path = "tests/controller.rs"]
mod tests;
