//! Shared construction for tx-pool service test harnesses.
//!
//! One builder replaces the six near-identical `TxPoolService`
//! constructions that used to be copy-pasted across this test tree
//! (`service_with_pipeline*`, `service_with_rbf*`, the secp variants and
//! the retry-cap harness). `chunk.rs` keeps its own harness on purpose: it
//! relies on a minimal mock store with only one column family, which is
//! load-bearing for the chunk-verification tests.
//!
//! Existing helpers are kept as thin wrappers over [`harness`], so no test
//! call sites change.

use crate::callback::Callbacks;
use crate::component::pipeline_queue::PipelineQueue;
use crate::component::pipeline_queues::PipelineQueues;
use crate::component::verify_queue::VerifyQueue;
use crate::component::waiting_room::WaitingRoom;
use crate::pool::TxPool;
use crate::resolve_mgr::{OrderedResolver, ResolveExit};
use crate::service::{TxPoolService, TxVerificationResult};
use crate::verify_mgr::VerifyMgr;
use ckb_fee_estimator::FeeEstimator;
use ckb_script::ChunkCommand;
use ckb_stop_handler::CancellationToken;
use ckb_test_chain_utils::MockStore;
use ckb_types::{
    core::FeeRate,
    packed::{CellDep, OutPoint},
};
use ckb_verification::cache::init_cache;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc, watch};

use super::pipeline::secp_test_consensus;
use super::pipeline::{snapshot_with_genesis, test_consensus, tx_pool_config};
use ckb_snapshot::Snapshot;

/// Which background workers the harness spawns.
pub(crate) enum WorkerSet {
    /// Nothing but the deferred-task drain loop: the test drives every
    /// queue manually (e.g. parking entries in the verify queue).
    None,
    /// The full pipeline: pre-check workers, verify manager and the ordered
    /// resolver (with a panic watcher), plus the deferred drain.
    All,
}

/// Everything a test needs from a constructed service, in one struct so
/// that adding a field stops changing every wrapper's return type.
pub(crate) struct Harness {
    pub(crate) service: TxPoolService,
    pub(crate) relay_rx: ckb_channel::Receiver<TxVerificationResult>,
    pub(crate) block_assembler_rx: mpsc::Receiver<crate::service::BlockAssemblerMessage>,
    pub(crate) cancel: CancellationToken,
    #[allow(dead_code)]
    pub(crate) store: MockStore,
    pub(crate) out_points: Vec<OutPoint>,
    #[allow(dead_code)]
    pub(crate) cell_deps: Option<Vec<CellDep>>,
    pub(crate) chunk_tx: Option<watch::Sender<ChunkCommand>>,
    #[allow(dead_code)]
    pub(crate) queues: Arc<PipelineQueues>,
}

pub(crate) struct HarnessBuilder {
    issue_outputs: usize,
    secp: bool,
    rbf: bool,
    max_tx_pool_size: Option<usize>,
    max_workers: Option<usize>,
    workers: WorkerSet,
    with_chunk_sender: bool,
    snapshot: Option<(MockStore, Arc<Snapshot>)>,
}

/// Start building a harness with `issue_outputs` always-success funding
/// cells in genesis.
pub(crate) fn harness(issue_outputs: usize) -> HarnessBuilder {
    HarnessBuilder {
        issue_outputs,
        secp: false,
        rbf: false,
        max_tx_pool_size: None,
        max_workers: None,
        workers: WorkerSet::All,
        with_chunk_sender: false,
        snapshot: None,
    }
}

impl HarnessBuilder {
    /// Use the secp256k1 test consensus (real signatures) instead of
    /// always-success.
    pub(crate) fn secp(mut self, enabled: bool) -> Self {
        self.secp = enabled;
        self
    }

    /// Enable RBF by setting `min_rbf_rate` above `min_fee_rate`.
    pub(crate) fn rbf(mut self, enabled: bool) -> Self {
        self.rbf = enabled;
        self
    }

    pub(crate) fn max_tx_pool_size(mut self, size: usize) -> Self {
        self.max_tx_pool_size = Some(size);
        self
    }

    pub(crate) fn max_workers(mut self, n: usize) -> Self {
        self.max_workers = Some(n);
        self
    }

    pub(crate) fn workers(mut self, set: WorkerSet) -> Self {
        self.workers = set;
        self
    }

    /// Return the `watch::Sender<ChunkCommand>` so the test can send
    /// Suspend/Resume signals to the verify manager.
    pub(crate) fn with_chunk_sender(mut self, enabled: bool) -> Self {
        self.with_chunk_sender = enabled;
        self
    }

    /// Use a custom (store, snapshot) pair instead of the default
    /// full-genesis snapshot.
    #[allow(dead_code)]
    pub(crate) fn snapshot(mut self, store: MockStore, snap: Arc<Snapshot>) -> Self {
        self.snapshot = Some((store, snap));
        self
    }

    pub(crate) fn build(self) -> Harness {
        let (consensus, out_points, cell_deps) = if self.secp {
            let (consensus, out_points, cell_deps) = secp_test_consensus(self.issue_outputs);
            (consensus, out_points, Some(cell_deps))
        } else {
            let (consensus, out_points) = test_consensus(self.issue_outputs);
            (consensus, out_points, None)
        };
        let consensus = Arc::new(consensus);
        let (store, snap) = self
            .snapshot
            .unwrap_or_else(|| snapshot_with_genesis(Arc::clone(&consensus)));

        let mut config = tx_pool_config();
        if self.rbf {
            config.min_rbf_rate = FeeRate::from_u64(1000);
        }
        if let Some(size) = self.max_tx_pool_size {
            config.max_tx_pool_size = size;
        }
        if let Some(n) = self.max_workers {
            config.max_tx_verify_workers = n;
        }

        let (tx_relay_sender, relay_rx) = ckb_channel::bounded(1024);
        let (block_assembler_sender, block_assembler_rx) = mpsc::channel(1);
        let signal = CancellationToken::new();
        let pre_check_cancel = signal.child_token();
        let queues = Arc::new(PipelineQueues {
            ordered_resolve_queue: RwLock::new(
                crate::component::ordered_resolve_queue::OrderedResolveQueue::new(),
            ),
            verify_queue: RwLock::new(VerifyQueue::new(
                config.max_tx_verify_cycles,
                config.verify_ordering,
                config.verify_queue_tx_size_budget(),
            )),
            pre_check_queue: crate::component::pre_check_queue::PreCheckQueue::new(
                pre_check_cancel,
            ),
            rbf_candidates: RwLock::new(crate::component::rbf_candidates::RbfCandidates::new()),
        });
        let (deferred_sender, mut deferred_receiver) = mpsc::channel(1024);
        // Two command channels, mirroring the previous inline harnesses: the
        // service keeps its own receiver (for direct per-tx verification),
        // while the verify manager and ordered resolver share a second one
        // whose sender may be handed to the test.
        let (service_chunk_tx, service_chunk_rx) = watch::channel(ChunkCommand::Resume);
        let (chunk_tx, verify_chunk_rx) = watch::channel(ChunkCommand::Resume);

        let service = TxPoolService {
            pool: crate::service::PoolCore {
                tx_pool: Arc::new(RwLock::new(TxPool::new(config.clone(), snap))),
                consensus: Arc::clone(&consensus),
                tx_pool_config: Arc::new(config),
            },
            pipeline: crate::service::PipelineState {
                epoch: Arc::new(crate::service::PipelineEpoch::default()),
                queues: Arc::clone(&queues),
                waiting_room: Arc::new(RwLock::new(WaitingRoom::new())),
                chunk_rx: service_chunk_rx,
                deferred_sender,
            },
            relay: crate::service::RelayState {
                network: super::chunk::dummy_network(),
                tx_relay_sender,
                block_assembler_sender,
                block_assembler_dirty: Arc::new(std::sync::atomic::AtomicU8::new(0)),
                callbacks: Arc::new(Callbacks::new()),
                banned_peers: Default::default(),
            },
            aux: crate::service::AuxServices {
                txs_verify_cache: Arc::new(RwLock::new(init_cache())),
                recent_reject: None,
                fee_estimator: FeeEstimator::new_dummy(),
            },
            block_assembler: None,
            recovery_lock: Arc::new(tokio::sync::Mutex::new(())),
        };

        // Drain deferred tasks (RBF recovery + verify cache updates) for tests.
        {
            let queues = Arc::clone(&queues);
            let txs_verify_cache = Arc::clone(&service.aux.txs_verify_cache);
            let epoch = Arc::clone(&service.pipeline.epoch);
            tokio::spawn(async move {
                while let Some(task) = deferred_receiver.recv().await {
                    match task {
                        crate::service::DeferredTask::RecoverTxs(txs) => {
                            let mut queue = queues.ordered_resolve_queue.write().await;
                            for job in txs {
                                if epoch.is_current(job.epoch) {
                                    let _ = queue.add_tx(job);
                                }
                            }
                        }
                        crate::service::DeferredTask::CacheUpdate { wtx_hash, verified } => {
                            let mut guard = txs_verify_cache.write().await;
                            guard.put(wtx_hash, verified);
                        }
                    }
                }
            });
        }

        match self.workers {
            WorkerSet::None => {}
            WorkerSet::All => {
                let max_workers = service.pool.tx_pool_config.max_tx_verify_workers.max(1);
                let pre_check_workers =
                    max_workers.min(std::thread::available_parallelism().map_or(4, |n| n.get()));
                for _ in 0..pre_check_workers {
                    let svc = service.clone();
                    tokio::spawn(crate::service::workers::run_pre_check_worker_loop(svc));
                }
                let mut verify_mgr =
                    VerifyMgr::new(service.clone(), verify_chunk_rx, signal.child_token());
                tokio::spawn(async move { verify_mgr.run().await });

                let ordered_resolver = OrderedResolver::new(
                    service.clone(),
                    chunk_tx.subscribe(),
                    signal.child_token(),
                );
                let (resolve_exit_tx, mut resolve_exit_rx) = tokio::sync::mpsc::unbounded_channel();
                let resolver_handle = ordered_resolver.start(resolve_exit_tx);
                tokio::spawn(async move {
                    if let Some((_, ResolveExit::Panicked { message })) =
                        resolve_exit_rx.recv().await
                    {
                        panic!("tx-pool ordered resolver panicked: {message}");
                    }
                    let _ = resolver_handle.await;
                });
            }
        }

        // Keep the command senders alive until the harness is cancelled: a
        // dropped sender closes the watch channel, and the pipeline workers
        // treat a closed command channel as a clean stop (channel drop =
        // shutdown). Holding them only in the `Harness` struct is not
        // enough — most tests destructure it and would drop the keep-alive
        // fields immediately.
        {
            let keepalive_cancel = signal.clone();
            let keep_service_tx = service_chunk_tx;
            let keep_verify_tx = chunk_tx.clone();
            tokio::spawn(async move {
                let _keep = (keep_service_tx, keep_verify_tx);
                keepalive_cancel.cancelled().await;
            });
        }

        Harness {
            service,
            relay_rx,
            block_assembler_rx,
            cancel: signal,
            store,
            out_points,
            cell_deps,
            chunk_tx: self.with_chunk_sender.then_some(chunk_tx),
            queues,
        }
    }
}
