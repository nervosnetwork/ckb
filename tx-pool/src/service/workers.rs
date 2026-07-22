//! Background worker spawning for the tx-pool pipeline.
//!
//! Every long-running pipeline task is spawned here: the pre-check worker
//! pool, the verify-manager and ordered-resolver monitors (with
//! panic-respawn backoff), the reorg handler, and the deferred-task worker.
//! The service builder (`service::builder`) keeps only assembly, startup
//! and shutdown orchestration; worker lifecycle lives in this module.

use crate::component::pipeline_queue::PipelineQueue;
use crate::service::{ChainReorgArgs, DeferredTask, Notify, TxPoolService};
use crate::verify_mgr::VerifyMgr;
use ckb_async_runtime::Handle;
use ckb_logger::{error, info};
use ckb_script::ChunkCommand;
use ckb_verification::cache::TxVerificationCache;
use futures_util::FutureExt;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::util::panic_payload_to_string;

/// Backoff between respawns of a crashed worker monitor.
///
/// Respawning immediately is right for a worker that died after a long
/// healthy run (a rare panic), while a persistent start-time failure (a
/// panic that fires immediately on every run) must not become a hot spin
/// with log spam. The delay therefore doubles per consecutive failure
/// (100ms → 25.6s, capped at 30s) and resets to the base after any run
/// that stayed up for at least `HEALTHY_RUN`.
struct RespawnBackoff {
    failures: u32,
}

impl RespawnBackoff {
    /// First retry delay after a failure.
    const BASE: Duration = Duration::from_millis(100);
    /// Maximum delay between respawns.
    const MAX: Duration = Duration::from_secs(30);
    /// A run lasting at least this long counts as healthy and resets the
    /// backoff to `BASE`.
    const HEALTHY_RUN: Duration = Duration::from_secs(60);

    fn new() -> Self {
        Self { failures: 0 }
    }

    /// Delay before the next respawn, given how long the previous run
    /// lasted.
    fn delay_for(&mut self, ran_for: Duration) -> Duration {
        if ran_for >= Self::HEALTHY_RUN {
            self.failures = 0;
        }
        let delay = Self::BASE.saturating_mul(2u32.saturating_pow(self.failures.min(10)));
        self.failures = self.failures.saturating_add(1);
        delay.min(Self::MAX)
    }
}

/// The pre-check worker body: pop → classify → finish, with a per-job
/// panic guard. Shared by the production worker pool and the test/bench
/// harnesses so the `finish` contract (and the per-job panic semantics)
/// lives in exactly one place.
pub(crate) async fn run_pre_check_worker_loop(service: TxPoolService) {
    while let Some(job) = service.pipeline.queues.pre_check_queue.pop().await {
        let id = job.tx.proposal_short_id();
        if service.is_recently_banned(job.source) {
            // The peer was banned after this job was popped: its in-flight
            // jobs must not keep flowing into the pool.
            service.terminal_internal(job.tx, job.source).await;
            service.pipeline.queues.pre_check_queue.finish(&id);
            continue;
        }
        let tx = job.tx.clone();
        let source = job.source;
        // Guard each job individually: one bad job must neither kill the
        // worker nor strand this job's active marker. The outer respawn
        // (production) stays as the backstop for panics outside the loop.
        let outcome = crate::worker::catch_job_panic(async {
            if let Err(reject) = service.classify_and_enqueue_tx(job.tx, job.source).await {
                // Non-`Full` rejects were already routed terminally inside
                // classify. `Full` is transient backpressure and
                // deliberately skips after_process there (nothing is
                // recorded); still close the loop with the relayer so the
                // peer's filter entry does not wait forever.
                if matches!(reject, crate::error::Reject::Full(_)) && source.peer().is_some() {
                    service.send_result_to_relayer(crate::service::TxVerificationResult::Reject {
                        tx_hash: tx.hash(),
                    });
                }
            }
        })
        .await;
        if let Err(message) = outcome {
            error!(
                "tx-pool pre-check worker panicked on job {}: {}",
                id, message
            );
            // Close the loop with the relayer: nothing is recorded (this
            // was an internal failure, not the transaction's fault), but
            // the peer's filter entry must not wait forever.
            service.terminal_internal(tx, source).await;
        }
        // Terminal state reached (forwarded, re-enqueued elsewhere,
        // rejected, or panicked): clear the active marker set at pop time.
        service.pipeline.queues.pre_check_queue.finish(&id);
    }
}

/// Spawn a pool of pre-check workers that pop jobs from the queue and
/// classify them into the pipeline.  Returns the spawned task handles so
/// the shutdown path can quiesce them before persisting.
pub(crate) fn spawn_pre_check_workers(
    handle: &Handle,
    service: TxPoolService,
    pre_check_cancel: CancellationToken,
    count: usize,
) -> Vec<tokio::task::JoinHandle<()>> {
    let mut handles = Vec::with_capacity(count);
    for _ in 0..count {
        let svc = service.clone();
        let cancel = pre_check_cancel.child_token();
        let handle = handle.spawn(async move {
            let mut backoff = RespawnBackoff::new();
            loop {
                let svc = svc.clone();
                let started = std::time::Instant::now();
                let worker = run_pre_check_worker_loop(svc);
                let exit = match AssertUnwindSafe(worker).catch_unwind().await {
                    Ok(()) => crate::resolve_mgr::ResolveExit::Stopped,
                    Err(payload) => crate::resolve_mgr::ResolveExit::Panicked {
                        message: crate::util::panic_payload_to_string(payload.as_ref()),
                    },
                };
                if cancel.is_cancelled() {
                    break;
                }
                match exit {
                    crate::resolve_mgr::ResolveExit::Stopped => {
                        // Normal exit because the queue was cancelled.
                        break;
                    }
                    crate::resolve_mgr::ResolveExit::Panicked { message } => {
                        error!("tx-pool pre-check worker panicked: {}; respawning", message);
                        tokio::select! {
                            _ = tokio::time::sleep(backoff.delay_for(started.elapsed())) => {}
                            _ = cancel.cancelled() => break,
                        }
                    }
                }
            }
        });
        handles.push(handle);
    }
    handles
}

/// Spawn the verification manager monitor with panic-respawn protection,
/// mirroring [`spawn_resolver_monitor`]. The manager supervises its verify
/// workers internally, but nothing watched the manager task itself:
/// without this loop, a manager-level exit (panic or unexpected stop)
/// would silently stall the whole verification stage — the verify queue
/// would fill up and every new transaction would eventually be rejected as
/// `Reject::Full`, with no log at all. Returns the spawned task handle so
/// the shutdown path can quiesce it before persisting.
pub(crate) fn spawn_verify_mgr_monitor(
    handle: &Handle,
    service: TxPoolService,
    chunk_rx: watch::Receiver<ChunkCommand>,
    signal: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    handle.spawn(async move {
        let mut backoff = RespawnBackoff::new();
        loop {
            let mut verify_mgr = VerifyMgr::new(service.clone(), chunk_rx.clone(), signal.clone());
            let started = std::time::Instant::now();
            let outcome = AssertUnwindSafe(verify_mgr.run()).catch_unwind().await;
            match outcome {
                Ok(()) => {
                    if signal.is_cancelled() {
                        break;
                    }
                    error!("tx-pool verify manager stopped unexpectedly, respawning");
                }
                Err(payload) => {
                    error!(
                        "tx-pool verify manager panicked: {}; respawning",
                        crate::util::panic_payload_to_string(payload.as_ref())
                    );
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(backoff.delay_for(started.elapsed())) => {}
                _ = signal.cancelled() => break,
            }
        }
        info!("TxPool verify manager monitor exited");
    })
}

/// Spawn the ordered resolver monitor with panic-respawn protection.
/// Returns the spawned task handle so the shutdown path can quiesce it
/// before persisting.
pub(crate) fn spawn_resolver_monitor(
    handle: &Handle,
    service: TxPoolService,
    chunk_rx: watch::Receiver<ChunkCommand>,
    resolver_exit_signal: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    handle.spawn(async move {
        let mut backoff = RespawnBackoff::new();
        loop {
            let resolver = crate::resolve_mgr::OrderedResolver::new(
                service.clone(),
                chunk_rx.clone(),
                resolver_exit_signal.clone(),
            );
            let (exit_tx, mut exit_rx) = tokio::sync::mpsc::unbounded_channel();
            let handle = resolver.start(exit_tx);
            let started = std::time::Instant::now();

            tokio::select! {
                _ = resolver_exit_signal.cancelled() => {
                    let _ = handle.await;
                    break;
                }
                Some((_worker_id, exit)) = exit_rx.recv() => {
                    let _ = handle.await;
                    match exit {
                        crate::resolve_mgr::ResolveExit::Stopped => {
                            if resolver_exit_signal.is_cancelled() {
                                break;
                            }
                            error!("tx-pool ordered resolver stopped unexpectedly, respawning");
                        }
                        crate::resolve_mgr::ResolveExit::Panicked { message } => {
                            error!("tx-pool ordered resolver panicked: {}; respawning", message);
                        }
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(backoff.delay_for(started.elapsed())) => {}
                        _ = resolver_exit_signal.cancelled() => break,
                    }
                }
            }
        }
        info!("TxPool ordered resolver monitor exited");
    })
}

/// Spawn the reorg handler with panic-respawn protection.
pub(crate) fn spawn_reorg_handler(
    handle: &Handle,
    service: TxPoolService,
    mut reorg_receiver: mpsc::Receiver<Notify<ChainReorgArgs>>,
    signal_receiver: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    handle.spawn(async move {
        loop {
            let service = service.clone();
            let keep_running = tokio::select! {
                Some(message) = reorg_receiver.recv() => {
                    let Notify {
                        arguments: (detached_blocks, attached_blocks, detached_proposal_id, snapshot),
                    } = message;

                    // One bounded retry: every step of reorg processing is
                    // idempotent — the write-lock section (snapshot swap,
                    // committed/detached-proposal removal, status
                    // migration, expiry, size limit) is a no-op when
                    // repeated, and the lock-free retain recovery re-adds
                    // each tx independently (duplicates return Ok(false)).
                    // Without the retry a single panic would silently drop
                    // the whole reorg: detached transactions would never
                    // be re-added and the pool would stay half-updated
                    // against the new tip.
                    let outcome = crate::worker::run_with_one_retry(|| {
                        let service = service.clone();
                        let detached_blocks = detached_blocks.clone();
                        let attached_blocks = attached_blocks.clone();
                        let detached_proposal_id = detached_proposal_id.clone();
                        let snapshot = Arc::clone(&snapshot);
                        async move {
                            service.update_block_assembler_before_tx_pool_reorg(
                                detached_blocks.clone(),
                                Arc::clone(&snapshot),
                            ).await;

                            service
                                .update_tx_pool_for_reorg(
                                    detached_blocks,
                                    attached_blocks,
                                    detached_proposal_id,
                                    snapshot,
                                )
                                .await;

                            service.update_block_assembler_after_tx_pool_reorg().await;
                        }
                    })
                    .await;
                    if let Err(message) = outcome {
                        error!(
                            "tx-pool reorg handler panicked twice; dropping reorg message: {}",
                            message
                        );
                    }
                    true
                },
                _ = signal_receiver.cancelled() => {
                    info!("TxPool reorg process service received exit signal, exit now");
                    false
                },
                else => false,
            };
            if !keep_running {
                break;
            }
        }
    })
}

/// Spawn the deferred task worker with panic-respawn protection.
///
/// Recovery tx re-enqueue and verify cache updates run sequentially in a
/// single background task. The worker only needs the ordered resolve queue
/// and the verify cache; it must NOT keep a clone of `TxPoolService`
/// (which holds `deferred_sender`), because the receiver task itself
/// holding a sender would keep the channel open forever.
pub(crate) fn spawn_deferred_worker(
    handle: &Handle,
    queues: Arc<crate::component::pipeline_queues::PipelineQueues>,
    txs_verify_cache: Arc<RwLock<TxVerificationCache>>,
    deferred_receiver: mpsc::Receiver<DeferredTask>,
    cancel: CancellationToken,
    relay: crate::service::RelayState,
) -> tokio::task::JoinHandle<()> {
    handle.spawn(async move {
        let mut deferred_rx = deferred_receiver;
        loop {
            // recv() is outside catch_unwind: the mpsc receiver is not
            // poisoned by panics in the message handler below.
            let task = tokio::select! {
                Some(task) = deferred_rx.recv() => task,
                _ = cancel.cancelled() => {
                    info!("deferred task worker received exit signal, draining remaining tasks");
                    // Best-effort drain: recovered txs lose their only
                    // handle when the channel is dropped, so enqueue each
                    // with a single attempt (no retries) before exiting.
                    while let Ok(task) = deferred_rx.try_recv() {
                        if let DeferredTask::RecoverTxs(txs) = task {
                            let mut queue = queues.ordered_resolve_queue.write().await;
                            for (tx, source) in txs {
                                let _ = queue
                                    .add_tx(crate::resolved_tx::ResolveJob::new(tx, source));
                            }
                        }
                    }
                    break;
                }
                else => break,
            };
            // Coalesce back-to-back recovery tasks: under RBF churn they
            // arrive in bursts, and each used to occupy its own bounded
            // retry window — merging keeps the deferred channel (and the
            // verify workers blocked on `send`) from backing up.
            let task = match task {
                DeferredTask::RecoverTxs(mut txs) => {
                    while let Ok(next) = deferred_rx.try_recv() {
                        match next {
                            DeferredTask::RecoverTxs(mut more) => txs.append(&mut more),
                            DeferredTask::CacheUpdate { wtx_hash, verified } => {
                                let mut guard = txs_verify_cache.write().await;
                                guard.put(wtx_hash, verified);
                            }
                        }
                    }
                    DeferredTask::RecoverTxs(txs)
                }
                other => other,
            };
            let queues = Arc::clone(&queues);
            let queues_retry = Arc::clone(&queues);
            let txs_verify_cache = Arc::clone(&txs_verify_cache);
            let recover_txs_for_retry = match &task {
                DeferredTask::RecoverTxs(txs) => Some(txs.clone()),
                DeferredTask::CacheUpdate { .. } => None,
            };
            let cancel_handler = cancel.clone();
            let relay_handler = relay.clone();
            let handler = async move {
                match task {
                    DeferredTask::RecoverTxs(txs) => {
                        crate::process::recover::enqueue_recover_txs(
                            queues,
                            txs,
                            &cancel_handler,
                            &relay_handler,
                        )
                        .await;
                    }
                    DeferredTask::CacheUpdate { wtx_hash, verified } => {
                        let mut guard = txs_verify_cache.write().await;
                        guard.put(wtx_hash, verified);
                    }
                }
            };
            match AssertUnwindSafe(handler).catch_unwind().await {
                Ok(()) => {}
                Err(payload) => {
                    let message = panic_payload_to_string(payload.as_ref());
                    error!("deferred task worker panicked: {}; continuing", message);
                    // If a RecoverTxs task panicked, retry it directly
                    // without going back through the channel. The retry is
                    // guarded too: a deterministic panic (same input, same
                    // failure) must not kill the worker — its death would
                    // close the deferred channel and silently drop every
                    // later recovery task.
                    if let Some(txs) = recover_txs_for_retry {
                        let retry = crate::worker::catch_job_panic(
                            crate::process::recover::enqueue_recover_txs(
                                queues_retry,
                                txs,
                                &cancel,
                                &relay,
                            ),
                        )
                        .await;
                        if let Err(message) = retry {
                            error!(
                                "deferred task worker retry panicked again: {}; dropping task",
                                message
                            );
                        }
                    }
                }
            }
        }
        info!("deferred task worker exited (channel closed)");
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn respawn_backoff_progresses_caps_and_resets() {
        let mut backoff = RespawnBackoff::new();
        let crash = Duration::from_millis(5);

        // Consecutive crashes: 100ms, 200ms, 400ms, ...
        assert_eq!(backoff.delay_for(crash), Duration::from_millis(100));
        assert_eq!(backoff.delay_for(crash), Duration::from_millis(200));
        assert_eq!(backoff.delay_for(crash), Duration::from_millis(400));

        // Capped at 30s under a persistent failure.
        for _ in 0..20 {
            backoff.delay_for(crash);
        }
        assert_eq!(backoff.delay_for(crash), Duration::from_secs(30));

        // A healthy run (>= HEALTHY_RUN) resets the backoff to the base.
        assert_eq!(
            backoff.delay_for(Duration::from_secs(120)),
            Duration::from_millis(100)
        );
    }

    /// Cancel during backoff sleep must exit immediately (bug #40):
    /// the `select!` wrapping the sleep must observe cancellation
    /// instead of waiting out the full backoff duration.
    #[tokio::test]
    async fn cancel_during_backoff_exits_immediately() {
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        // Spawn a task that simulates a worker in backoff sleep.
        let handle = tokio::spawn(async move {
            let mut backoff = RespawnBackoff::new();
            // Simulate a crash to get a non-trivial backoff delay.
            let _ = backoff.delay_for(Duration::from_millis(1));
            let _ = backoff.delay_for(Duration::from_millis(1));
            // Now the next delay would be 400ms.
            let delay = backoff.delay_for(Duration::from_millis(1));
            assert!(delay >= Duration::from_millis(400));

            let started = std::time::Instant::now();
            tokio::select! {
                _ = tokio::time::sleep(delay) => {
                    // Should not reach here if cancel fires first.
                    started.elapsed()
                }
                _ = cancel_clone.cancelled() => {
                    // Cancel fired: exit immediately.
                    started.elapsed()
                }
            }
        });

        // Give the task time to enter the select!.
        tokio::time::sleep(Duration::from_millis(50)).await;
        // Cancel: the task should exit well before the 400ms backoff.
        cancel.cancel();

        let elapsed = tokio::time::timeout(Duration::from_millis(200), handle)
            .await
            .expect("task must exit within 200ms of cancel, not wait out the backoff")
            .expect("task joins");

        assert!(
            elapsed < Duration::from_millis(150),
            "cancel must interrupt backoff sleep immediately, took {:?}",
            elapsed
        );
    }

    /// Cancel must drain pending `RecoverTxs` from the deferred channel
    /// before exiting (bug #41): recovered transactions lose their only
    /// handle when the channel is dropped, so they must be enqueued.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_drains_deferred_recover_txs() {
        use crate::component::pipeline_queue::PipelineQueue;
        use crate::service::DeferredTask;
        use ckb_types::core::TransactionBuilder;

        let queues = Arc::new(crate::component::pipeline_queues::PipelineQueues {
            ordered_resolve_queue: RwLock::new(
                crate::component::ordered_resolve_queue::OrderedResolveQueue::new(),
            ),
            verify_queue: RwLock::new(crate::component::verify_queue::VerifyQueue::new(
                70_000_000,
                ckb_app_config::VerifyOrdering::ArrivalTime,
                usize::MAX,
            )),
            pre_check_queue: crate::component::pre_check_queue::PreCheckQueue::new(
                CancellationToken::new(),
            ),
            rbf_candidates: RwLock::new(crate::component::rbf_candidates::RbfCandidates::new()),
        });
        let txs_verify_cache = Arc::new(RwLock::new(ckb_verification::cache::init_cache()));
        let (deferred_tx, deferred_rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();
        let (tx_relay_sender, _relay_rx) = ckb_channel::bounded(16);
        let (block_assembler_sender, _ba_rx) = tokio::sync::mpsc::channel(1);
        let relay = crate::service::RelayState {
            network: Arc::new(crate::network::DummyTxPoolNetwork),
            tx_relay_sender,
            block_assembler_sender,
            callbacks: Arc::new(crate::callback::Callbacks::new()),
            banned_peers: Default::default(),
        };

        // Enqueue two RecoverTxs tasks before cancelling.
        let tx1 = TransactionBuilder::default().build();
        let tx2 = TransactionBuilder::default()
            .input(ckb_types::packed::CellInput::new(
                ckb_types::packed::OutPoint::new(tx1.hash(), 0),
                0,
            ))
            .build();
        let id1 = tx1.proposal_short_id();
        let id2 = tx2.proposal_short_id();
        deferred_tx
            .send(DeferredTask::RecoverTxs(vec![
                (tx1, crate::tx_source::TxSource::Local),
                (tx2, crate::tx_source::TxSource::Local),
            ]))
            .await
            .unwrap();

        // Spawn the deferred worker, then cancel immediately.
        let ckb_handle = ckb_async_runtime::Handle::new(tokio::runtime::Handle::current(), None);
        let handle = spawn_deferred_worker(
            &ckb_handle,
            Arc::clone(&queues),
            txs_verify_cache,
            deferred_rx,
            cancel.clone(),
            relay,
        );
        // Give the worker a moment to start and block on recv.
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();

        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("deferred worker must exit after cancel")
            .expect("task joins");

        // The RecoverTxs must have been drained into the ordered queue.
        let queue = queues.ordered_resolve_queue.read().await;
        assert!(
            queue.contains_key(&id1) || queue.contains_key(&id2),
            "at least one recovered tx must be drained into the ordered queue on cancel"
        );
    }
}
