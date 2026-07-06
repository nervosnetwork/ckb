//! Tx-pool background service

use crate::block_assembler::{self, BlockAssembler};
use crate::callback::{Callbacks, PendingCallback, ProposedCallback, RejectCallback};
use crate::component::orphan::OrphanPool;
use crate::component::pipeline_queue::PipelineQueue;
use crate::component::pool_map::Status;
#[cfg(feature = "pipeline")]
use crate::component::rbf_candidates::RbfCandidates;
use crate::component::recent_reject::RecentReject;
use crate::component::verify_queue::VerifyQueue;
use crate::constants::SECONDS_PER_DAY;
use crate::error::{handle_recv_error, handle_send_cmd_error, handle_try_send_error};
use crate::network::{TxPoolNetwork, TxPoolNetworkHandle};
use crate::pool::TxPool;
use crate::verify_mgr::VerifyMgr;
use ckb_app_config::{BlockAssemblerConfig, TxPoolConfig};
use ckb_async_runtime::Handle;
use ckb_chain_spec::consensus::Consensus;
use ckb_channel::oneshot;
use ckb_error::AnyError;
use ckb_fee_estimator::FeeEstimator;
use ckb_jsonrpc_types::BlockTemplate;
use ckb_logger::{debug, error, info, warn};
use ckb_network::PeerIndex;
use ckb_script::ChunkCommand;
use ckb_snapshot::Snapshot;
use ckb_stop_handler::new_tokio_exit_rx;
use ckb_store::ChainStore;
use ckb_types::{
    core::{
        BlockView, Capacity, Cycle, EstimateMode, FeeRate, TransactionView, UncleBlockView,
        Version,
        cell::{CellProvider, CellStatus, OverlayCellProvider},
        tx_pool::{
            EntryCompleted, PoolTxDetailInfo, Reject, TRANSACTION_SIZE_LIMIT,
            TransactionWithStatus, TxPoolEntryInfo, TxPoolIds, TxPoolInfo, TxStatus,
        },
    },
    packed::{Byte32, OutPoint, ProposalShortId},
};
use ckb_util::LinkedHashSet;
use ckb_verification::cache::TxVerificationCache;
use futures_util::FutureExt;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::panic::AssertUnwindSafe;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::sync::watch;
use tokio::sync::{RwLock, Semaphore, mpsc};
use tokio::task::block_in_place;
use tokio_util::sync::CancellationToken;

use crate::pool_cell::PoolCell;
use crate::util::panic_payload_to_string;
#[cfg(feature = "internal")]
use crate::{component::entry::TxEntry, process::PlugTarget};

pub(crate) const DEFAULT_CHANNEL_SIZE: usize = 512;
pub(crate) const BLOCK_ASSEMBLER_CHANNEL_SIZE: usize = 100;

fn respond<R: fmt::Debug>(responder: oneshot::Sender<R>, value: R, message: &'static str) {
    if let Err(e) = responder.send(value) {
        error!("Responder sending {} failed {:?}", message, e);
    }
}

fn respond_async<R: fmt::Debug>(
    responder: tokio::sync::oneshot::Sender<R>,
    value: R,
    message: &'static str,
) {
    if let Err(e) = responder.send(value) {
        error!("Responder sending {} failed {:?}", message, e);
    }
}

async fn enqueue_recover_txs(
    ordered_queue: Arc<RwLock<crate::component::ordered_resolve_queue::OrderedResolveQueue>>,
    txs: Vec<TransactionView>,
) {
    let mut queue = ordered_queue.write().await;
    for tx in txs {
        debug!("recover back: {:?}", tx.proposal_short_id());
        if let Err(reject) = queue.add_tx(crate::resolved_tx::ResolveJob {
            tx,
            remote: None,
            is_proposal_tx: false,
            attempts: 0,
        }) {
            warn!(
                "failed to recover tx back to ordered resolve queue: {}",
                reject
            );
        }
    }
}

pub(crate) struct Request<R, A> {
    pub responder: R,
    pub arguments: A,
}

impl<R, A> Request<R, A> {
    pub(crate) fn call(arguments: A, responder: R) -> Request<R, A> {
        Request {
            responder,
            arguments,
        }
    }
}

/// Synchronous request using the `ckb_channel` oneshot responder.
pub(crate) type SyncRequest<A, T> = Request<oneshot::Sender<T>, A>;
/// Asynchronous request using the `tokio` oneshot responder.
pub(crate) type AsyncRequest<A, T> = Request<tokio::sync::oneshot::Sender<T>, A>;

pub(crate) struct Notify<A> {
    pub arguments: A,
}

impl<A> Notify<A> {
    pub(crate) fn new(arguments: A) -> Notify<A> {
        Notify { arguments }
    }
}

pub(crate) type BlockTemplateResult = Result<BlockTemplate, AnyError>;
type BlockTemplateArgs = (Option<u64>, Option<u64>, Option<Version>);

pub(crate) type SubmitTxResult = Result<(), Reject>;

pub(crate) type TestAcceptTxResult = Result<EntryCompleted, Reject>;

type GetTxStatusResult = Result<(TxStatus, Option<Cycle>), AnyError>;
type GetTransactionWithStatusResult = Result<TransactionWithStatus, AnyError>;
type FetchTxsWithCyclesResult = Vec<(ProposalShortId, (TransactionView, Cycle))>;

pub(crate) type ChainReorgArgs = (
    VecDeque<BlockView>,
    VecDeque<BlockView>,
    HashSet<ProposalShortId>,
    Arc<Snapshot>,
);

pub(crate) type FeeEstimatesResult = Result<FeeRate, AnyError>;

pub(crate) enum Message {
    BlockTemplate(SyncRequest<BlockTemplateArgs, BlockTemplateResult>),
    SubmitLocalTx(SyncRequest<TransactionView, SubmitTxResult>),
    RemoveLocalTx(SyncRequest<Byte32, bool>),
    TestAcceptTx(SyncRequest<TransactionView, TestAcceptTxResult>),
    SubmitRemoteTx(SyncRequest<(TransactionView, Cycle, PeerIndex), ()>),
    NotifyTxs(Notify<Vec<TransactionView>>),
    FreshProposalsFilter(AsyncRequest<Vec<ProposalShortId>, Vec<ProposalShortId>>),
    FetchTxs(AsyncRequest<HashSet<ProposalShortId>, HashMap<ProposalShortId, TransactionView>>),
    FetchTxsWithCycles(AsyncRequest<HashSet<ProposalShortId>, FetchTxsWithCyclesResult>),
    GetTxPoolInfo(SyncRequest<(), TxPoolInfo>),
    GetLiveCell(SyncRequest<(OutPoint, bool), CellStatus>),
    GetTxStatus(SyncRequest<Byte32, GetTxStatusResult>),
    GetTransactionWithStatus(SyncRequest<Byte32, GetTransactionWithStatusResult>),
    NewUncle(Notify<UncleBlockView>),
    /// Replace the tx-pool snapshot, clear **all** in-pool entries, and drain
    /// all pipeline queues (ordered resolve, verification, orphan and pre-check).
    ClearPool(SyncRequest<Arc<Snapshot>, ()>),
    /// Clear only the pipeline queues (ordered resolve, verification, orphan
    /// and pre-check) without touching the already-accepted pool.
    ClearPipeline(SyncRequest<(), ()>),
    GetAllEntryInfo(SyncRequest<(), TxPoolEntryInfo>),
    GetAllIds(SyncRequest<(), TxPoolIds>),
    SavePool(SyncRequest<(), ()>),
    GetPoolTxDetails(SyncRequest<Byte32, PoolTxDetailInfo>),
    GetTotalRecentRejectNum(SyncRequest<(), Option<u64>>),

    UpdateIBDState(SyncRequest<bool, ()>),
    EstimateFeeRate(SyncRequest<(EstimateMode, bool), FeeEstimatesResult>),

    // test
    #[cfg(feature = "internal")]
    PlugEntry(SyncRequest<(Vec<TxEntry>, PlugTarget), ()>),
    #[cfg(feature = "internal")]
    PackageTxs(SyncRequest<Option<u64>, Vec<TxEntry>>),
    SubmitLocalTestTx(SyncRequest<TransactionView, SubmitTxResult>),
}

#[derive(Debug, Hash, Eq, PartialEq)]
pub(crate) enum BlockAssemblerMessage {
    Pending,
    Proposed,
    Uncle,
    Reset(Arc<Snapshot>),
}

/// Controller to the tx-pool service.
///
/// The Controller is internally reference-counted and can be freely cloned. A Controller can be obtained when tx-pool service construct.
#[derive(Clone)]
pub struct TxPoolController {
    sender: mpsc::Sender<Message>,
    reorg_sender: mpsc::Sender<Notify<ChainReorgArgs>>,
    chunk_tx: Arc<watch::Sender<ChunkCommand>>,
    handle: Handle,
    started: Arc<AtomicBool>,
    signal: CancellationToken,
}

macro_rules! send_message {
    ($self:ident, $msg_type:ident, $args:expr) => {{
        let (responder, response) = oneshot::channel();
        let request = Request::call($args, responder);
        $self
            .sender
            .try_send(Message::$msg_type(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        block_in_place(|| response.recv())
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }};
}

macro_rules! send_notify {
    ($self:ident, $msg_type:ident, $args:expr) => {{
        let notify = Notify::new($args);
        $self
            .sender
            .try_send(Message::$msg_type(notify))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e.into()
            })
    }};
}

impl TxPoolController {
    /// Return whether tx-pool service is started
    pub fn service_started(&self) -> bool {
        self.started.load(Ordering::Acquire)
    }

    /// Set tx-pool service started, should only used for test
    #[cfg(feature = "internal")]
    pub fn set_service_started(&self, v: bool) {
        self.started.store(v, Ordering::Release);
    }

    /// Return reference of tokio runtime handle
    pub fn handle(&self) -> &Handle {
        &self.handle
    }

    /// Send a graceful stop signal to the tx-pool service and background workers.
    pub fn stop(&self) {
        self.signal.cancel();
    }

    /// Generate and return block_template
    pub fn get_block_template(
        &self,
        bytes_limit: Option<u64>,
        proposals_limit: Option<u64>,
        max_version: Option<Version>,
    ) -> Result<BlockTemplateResult, AnyError> {
        send_message!(
            self,
            BlockTemplate,
            (bytes_limit, proposals_limit, max_version)
        )
    }

    /// Notify new uncle
    pub fn notify_new_uncle(&self, uncle: UncleBlockView) -> Result<(), AnyError> {
        send_notify!(self, NewUncle, uncle)
    }

    /// Make tx-pool consistent after a reorg, by re-adding or recursively erasing
    /// detached block transactions from the tx-pool, and also removing any
    /// other transactions from the tx-pool that are no longer valid given the new
    /// tip/height.
    pub fn update_tx_pool_for_reorg(
        &self,
        detached_blocks: VecDeque<BlockView>,
        attached_blocks: VecDeque<BlockView>,
        detached_proposal_id: HashSet<ProposalShortId>,
        snapshot: Arc<Snapshot>,
    ) -> Result<(), AnyError> {
        let notify = Notify::new((
            detached_blocks,
            attached_blocks,
            detached_proposal_id,
            snapshot,
        ));
        self.reorg_sender.try_send(notify).map_err(|e| {
            let (_m, e) = handle_try_send_error(e);
            e.into()
        })
    }

    /// Submit local tx to tx-pool
    pub fn submit_local_tx(&self, tx: TransactionView) -> Result<SubmitTxResult, AnyError> {
        send_message!(self, SubmitLocalTx, tx)
    }

    /// test if a tx can be accepted by tx-pool
    /// Won't be broadcasted to network
    /// won't be insert to tx-pool
    pub fn test_accept_tx(&self, tx: TransactionView) -> Result<TestAcceptTxResult, AnyError> {
        send_message!(self, TestAcceptTx, tx)
    }

    /// Remove tx from tx-pool
    pub fn remove_local_tx(&self, tx_hash: Byte32) -> Result<bool, AnyError> {
        send_message!(self, RemoveLocalTx, tx_hash)
    }

    /// Submit remote tx with declared cycles and origin to tx-pool
    pub async fn submit_remote_tx(
        &self,
        tx: TransactionView,
        declared_cycles: Cycle,
        peer: PeerIndex,
    ) -> Result<(), AnyError> {
        send_message!(self, SubmitRemoteTx, (tx, declared_cycles, peer))
    }

    /// Receive txs from network, try to add txs to tx-pool
    pub fn notify_txs(&self, txs: Vec<TransactionView>) -> Result<(), AnyError> {
        send_notify!(self, NotifyTxs, txs)
    }

    /// Receive txs from network, try to add txs to tx-pool
    pub async fn notify_txs_async(&self, txs: Vec<TransactionView>) -> Result<(), AnyError> {
        let notify = Notify::new(txs);
        self.sender
            .try_send(Message::NotifyTxs(notify))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e.into()
            })
    }

    /// Return tx-pool information
    pub fn get_tx_pool_info(&self) -> Result<TxPoolInfo, AnyError> {
        send_message!(self, GetTxPoolInfo, ())
    }

    /// Return tx-pool information
    pub fn get_live_cell(
        &self,
        out_point: OutPoint,
        with_data: bool,
    ) -> Result<CellStatus, AnyError> {
        send_message!(self, GetLiveCell, (out_point, with_data))
    }

    /// Return fresh proposals
    pub async fn fresh_proposals_filter(
        &self,
        proposals: Vec<ProposalShortId>,
    ) -> Result<Vec<ProposalShortId>, AnyError> {
        let (responder, response) = tokio::sync::oneshot::channel();
        let request = AsyncRequest::call(proposals, responder);
        self.sender
            .try_send(Message::FreshProposalsFilter(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        response.await.map_err(Into::into)
    }

    /// Return tx_status for rpc (get_transaction verbosity = 1)
    pub fn get_tx_status(&self, hash: Byte32) -> Result<GetTxStatusResult, AnyError> {
        send_message!(self, GetTxStatus, hash)
    }

    /// Return transaction_with_status for rpc (get_transaction verbosity = 2)
    pub fn get_transaction_with_status(
        &self,
        hash: Byte32,
    ) -> Result<GetTransactionWithStatusResult, AnyError> {
        send_message!(self, GetTransactionWithStatus, hash)
    }

    /// Mainly used for compact block reconstruction and block proposal pre-broadcasting
    /// Orphan/conflicted/etc transactions that are returned for compact block reconstruction.
    pub async fn fetch_txs(
        &self,
        short_ids: HashSet<ProposalShortId>,
    ) -> Result<HashMap<ProposalShortId, TransactionView>, AnyError> {
        let (responder, response) = tokio::sync::oneshot::channel();
        let request = AsyncRequest::call(short_ids, responder);
        self.sender
            .try_send(Message::FetchTxs(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        response.await.map_err(Into::into)
    }

    /// Return txs with cycles
    /// Mainly for relay transactions
    pub async fn fetch_txs_with_cycles(
        &self,
        short_ids: HashSet<ProposalShortId>,
    ) -> Result<FetchTxsWithCyclesResult, AnyError> {
        let (responder, response) = tokio::sync::oneshot::channel();
        let request = AsyncRequest::call(short_ids, responder);
        self.sender
            .try_send(Message::FetchTxsWithCycles(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        response.await.map_err(Into::into)
    }

    /// Clears the tx-pool, removing all txs, update snapshot.
    pub fn clear_pool(&self, new_snapshot: Arc<Snapshot>) -> Result<(), AnyError> {
        send_message!(self, ClearPool, new_snapshot)
    }

    /// Clears the pipeline queues (ordered resolve, verify, orphan and
    /// pre-check) without touching the already-accepted pool.
    pub fn clear_verify_queue(&self) -> Result<(), AnyError> {
        send_message!(self, ClearPipeline, ())
    }

    /// Returns information about all transactions in the pool.
    pub fn get_all_entry_info(&self) -> Result<TxPoolEntryInfo, AnyError> {
        send_message!(self, GetAllEntryInfo, ())
    }

    /// Returns the IDs of all transactions in the pool.
    pub fn get_all_ids(&self) -> Result<TxPoolIds, AnyError> {
        send_message!(self, GetAllIds, ())
    }

    /// query the details of a transaction in the pool
    pub fn get_tx_detail(&self, tx_hash: Byte32) -> Result<PoolTxDetailInfo, AnyError> {
        send_message!(self, GetPoolTxDetails, tx_hash)
    }

    /// Saves tx pool into disk.
    pub fn save_pool(&self) -> Result<(), AnyError> {
        info!("Please be patient, tx-pool are saving data into disk ...");
        send_message!(self, SavePool, ())
    }

    /// Updates IBD state.
    pub fn update_ibd_state(&self, in_ibd: bool) -> Result<(), AnyError> {
        send_message!(self, UpdateIBDState, in_ibd)
    }

    /// Estimates fee rate.
    pub fn estimate_fee_rate(
        &self,
        estimate_mode: EstimateMode,
        enable_fallback: bool,
    ) -> Result<FeeEstimatesResult, AnyError> {
        send_message!(self, EstimateFeeRate, (estimate_mode, enable_fallback))
    }

    /// Sends suspend chunk process cmd
    pub fn suspend_chunk_process(&self) -> Result<(), AnyError> {
        //debug!("[verify-test] run suspend_chunk_process");
        self.chunk_tx
            .send(ChunkCommand::Suspend)
            .map_err(handle_send_cmd_error)
            .map_err(Into::into)
    }

    /// Sends continue chunk process cmd
    pub fn continue_chunk_process(&self) -> Result<(), AnyError> {
        //debug!("[verify-test] run continue_chunk_process");
        self.chunk_tx
            .send(ChunkCommand::Resume)
            .map_err(handle_send_cmd_error)
            .map_err(Into::into)
    }

    /// Load persisted txs into pool, assume that all txs are sorted
    fn load_persisted_data(&self, txs: Vec<TransactionView>) -> Result<(), AnyError> {
        if !txs.is_empty() {
            info!("Loading persistent tx-pool data, total {} txs", txs.len());
            let mut failed_txs = 0;
            for tx in txs {
                if self.submit_local_tx(tx)?.is_err() {
                    failed_txs += 1;
                }
            }
            if failed_txs == 0 {
                info!("Persistent tx-pool data is loaded");
            } else {
                info!(
                    "Persistent tx-pool data is loaded, {} stale txs are ignored",
                    failed_txs
                );
            }
        }
        Ok(())
    }

    /// Plug tx-pool entry to tx-pool, skip verification. only for test
    #[cfg(feature = "internal")]
    pub fn plug_entry(&self, entries: Vec<TxEntry>, target: PlugTarget) -> Result<(), AnyError> {
        send_message!(self, PlugEntry, (entries, target))
    }

    /// Package txs with specified bytes_limit. for test
    #[cfg(feature = "internal")]
    pub fn package_txs(&self, bytes_limit: Option<u64>) -> Result<Vec<TxEntry>, AnyError> {
        send_message!(self, PackageTxs, bytes_limit)
    }

    /// Submit local test tx to tx-pool, this tx will be put into verify queue directly.
    pub fn submit_local_test_tx(&self, tx: TransactionView) -> Result<SubmitTxResult, AnyError> {
        send_message!(self, SubmitLocalTestTx, tx)
    }

    /// get total recent reject num
    pub fn get_total_recent_reject_num(&self) -> Result<Option<u64>, AnyError> {
        send_message!(self, GetTotalRecentRejectNum, ())
    }
}

#[cfg(test)]
mod tests;

/// A builder used to create TxPoolService.
pub struct TxPoolServiceBuilder {
    pub(crate) tx_pool_config: TxPoolConfig,
    pub(crate) tx_pool_controller: TxPoolController,
    pub(crate) snapshot: Arc<Snapshot>,
    pub(crate) block_assembler: Option<BlockAssembler>,
    pub(crate) txs_verify_cache: Arc<RwLock<TxVerificationCache>>,
    pub(crate) callbacks: Callbacks,
    pub(crate) receiver: mpsc::Receiver<Message>,
    pub(crate) reorg_receiver: mpsc::Receiver<Notify<ChainReorgArgs>>,
    pub(crate) signal_receiver: CancellationToken,
    pub(crate) handle: Handle,
    pub(crate) tx_relay_sender: ckb_channel::Sender<TxVerificationResult>,
    pub(crate) chunk_rx: watch::Receiver<ChunkCommand>,
    pub(crate) started: Arc<AtomicBool>,
    pub(crate) block_assembler_channel: (
        mpsc::Sender<BlockAssemblerMessage>,
        mpsc::Receiver<BlockAssemblerMessage>,
    ),
    pub(crate) fee_estimator: FeeEstimator,
    pub(crate) recent_reject: Option<Arc<RecentReject>>,
}

impl TxPoolServiceBuilder {
    /// Creates a new TxPoolServiceBuilder.
    pub fn new(
        tx_pool_config: TxPoolConfig,
        snapshot: Arc<Snapshot>,
        block_assembler_config: Option<BlockAssemblerConfig>,
        txs_verify_cache: Arc<RwLock<TxVerificationCache>>,
        handle: &Handle,
        tx_relay_sender: ckb_channel::Sender<TxVerificationResult>,
        fee_estimator: FeeEstimator,
    ) -> (TxPoolServiceBuilder, TxPoolController) {
        let (sender, receiver) = mpsc::channel(DEFAULT_CHANNEL_SIZE);
        let block_assembler_channel = mpsc::channel(BLOCK_ASSEMBLER_CHANNEL_SIZE);
        let (reorg_sender, reorg_receiver) = mpsc::channel(DEFAULT_CHANNEL_SIZE);
        let signal_receiver: CancellationToken = new_tokio_exit_rx();
        let (chunk_tx, chunk_rx) = watch::channel(ChunkCommand::Resume);
        let started = Arc::new(AtomicBool::new(false));

        let controller = TxPoolController {
            sender,
            reorg_sender,
            handle: handle.clone(),
            chunk_tx: Arc::new(chunk_tx),
            started: Arc::clone(&started),
            signal: signal_receiver.clone(),
        };

        let block_assembler = block_assembler_config.and_then(|config| {
            BlockAssembler::new(config, Arc::clone(&snapshot))
                .inspect_err(|err| error!("failed to initialize block assembler: {}", err))
                .ok()
        });
        let recent_reject = Self::build_recent_reject(&tx_pool_config).map(Arc::new);
        let builder = TxPoolServiceBuilder {
            tx_pool_config,
            tx_pool_controller: controller.clone(),
            snapshot,
            block_assembler,
            txs_verify_cache,
            callbacks: Callbacks::new(),
            receiver,
            reorg_receiver,
            signal_receiver,
            handle: handle.clone(),
            tx_relay_sender,
            chunk_rx,
            started,
            block_assembler_channel,
            fee_estimator,
            recent_reject,
        };

        (builder, controller)
    }

    /// Register new pending callback
    pub fn register_pending(&mut self, callback: PendingCallback) {
        self.callbacks.register_pending(callback);
    }

    /// Return cloned tx relayer sender
    pub fn tx_relay_sender(&self) -> ckb_channel::Sender<TxVerificationResult> {
        self.tx_relay_sender.clone()
    }

    /// Register new proposed callback
    pub fn register_proposed(&mut self, callback: ProposedCallback) {
        self.callbacks.register_proposed(callback);
    }

    /// Register new abandon callback
    pub fn register_reject(&mut self, callback: RejectCallback) {
        self.callbacks.register_reject(callback);
    }

    /// Access the shared recent-reject database (for registering callbacks).
    pub fn recent_reject(&self) -> Option<Arc<RecentReject>> {
        self.recent_reject.clone()
    }

    pub(crate) fn build_recent_reject(config: &TxPoolConfig) -> Option<RecentReject> {
        if !config.recent_reject.as_os_str().is_empty() {
            let recent_reject_ttl =
                u8::max(1, config.keep_rejected_tx_hashes_days) as i32 * SECONDS_PER_DAY;
            match RecentReject::new(
                &config.recent_reject,
                config.keep_rejected_tx_hashes_count,
                recent_reject_ttl,
            ) {
                Ok(recent_reject) => Some(recent_reject),
                Err(err) => {
                    error!(
                        "Failed to open the recent reject database {:?} {}",
                        config.recent_reject, err
                    );
                    None
                }
            }
        } else {
            warn!("Recent reject database is disabled!");
            None
        }
    }

    /// Start a background thread tx-pool service by taking ownership of the Builder, and returns a TxPoolController.
    pub fn start<N: TxPoolNetwork>(self, network: N) {
        let network: TxPoolNetworkHandle = Arc::new(network);
        let consensus = self.snapshot.cloned_consensus();

        let ordered_resolve_queue = Arc::new(RwLock::new(
            crate::component::ordered_resolve_queue::OrderedResolveQueue::new(),
        ));
        let verify_queue = Arc::new(RwLock::new(VerifyQueue::new(
            self.tx_pool_config.max_tx_verify_cycles,
            self.tx_pool_config.verify_ordering,
        )));

        let tx_pool = TxPool::new(self.tx_pool_config, self.snapshot);
        let recent_reject = self.recent_reject;
        let txs = match tx_pool.load_from_file() {
            Ok(txs) => txs,
            Err(e) => {
                error!("{}", e.to_string());
                error!("Failed to load txs from tx-pool persistent data file, all txs are ignored");
                Vec::new()
            }
        };

        let (block_assembler_sender, mut block_assembler_receiver) = self.block_assembler_channel;
        #[cfg(feature = "pipeline")]
        let max_workers = tx_pool.config.max_tx_verify_workers.max(1);
        // Cap pre-check concurrency to the number of available CPU cores so that
        // cheap pre-resolution does not starve the heavier verification workers
        // on the shared tokio runtime.
        #[cfg(feature = "pipeline")]
        let pre_check_workers =
            max_workers.min(std::thread::available_parallelism().map_or(4, |n| n.get()));
        #[cfg(feature = "pipeline")]
        let pre_check_cancel = self.signal_receiver.child_token();
        #[cfg(feature = "pipeline")]
        let pre_check_queue = Arc::new(crate::component::pre_check_queue::PreCheckQueue::new(
            pre_check_cancel.clone(),
        ));

        let (deferred_sender, deferred_receiver) =
            mpsc::channel::<DeferredTask>(crate::constants::DEFERRED_CHANNEL_SIZE);

        let service = TxPoolService {
            tx_pool_config: Arc::new(tx_pool.config.clone()),
            tx_pool: Arc::new(RwLock::new(tx_pool)),
            orphan: Arc::new(RwLock::new(OrphanPool::new())),
            block_assembler: self.block_assembler,
            txs_verify_cache: self.txs_verify_cache,
            callbacks: Arc::new(self.callbacks),
            tx_relay_sender: self.tx_relay_sender,
            block_assembler_sender,
            ordered_resolve_queue: Arc::clone(&ordered_resolve_queue),
            verify_queue: Arc::clone(&verify_queue),
            network,
            consensus,
            fee_estimator: self.fee_estimator,
            recent_reject,
            #[cfg(feature = "pipeline")]
            pre_check_queue: Arc::clone(&pre_check_queue),
            #[cfg(feature = "pipeline")]
            chunk_rx: self.chunk_rx.clone(),
            #[cfg(feature = "pipeline")]
            rbf_candidates: Arc::new(RwLock::new(RbfCandidates::new())),
            deferred_sender,
        };

        // Spawn the deferred task worker with panic-respawn protection.
        // Recovery tx re-enqueue and verify cache updates run sequentially
        // in a single background task, with automatic respawn on panic.
        {
            // The worker only needs the ordered resolve queue and the verify
            // cache.  It must NOT keep a clone of `TxPoolService` (which holds
            // `deferred_sender`): doing so would keep the channel open forever
            // because the receiver task itself would be holding a sender.
            let ordered_resolve_queue = Arc::clone(&service.ordered_resolve_queue);
            let txs_verify_cache = Arc::clone(&service.txs_verify_cache);
            self.handle.spawn(async move {
                let mut deferred_rx = deferred_receiver;
                loop {
                    // recv() is outside catch_unwind: the mpsc receiver is not
                    // poisoned by panics in the message handler below.
                    let task = match deferred_rx.recv().await {
                        Some(task) => task,
                        None => break, // channel closed, exit
                    };
                    let ordered_resolve_queue = Arc::clone(&ordered_resolve_queue);
                    let ordered_resolve_queue_retry = Arc::clone(&ordered_resolve_queue);
                    let txs_verify_cache = Arc::clone(&txs_verify_cache);
                    let recover_txs_for_retry = match &task {
                        DeferredTask::RecoverTxs(txs) => Some(txs.clone()),
                        DeferredTask::CacheUpdate { .. } => None,
                    };
                    let handler = async move {
                        match task {
                            DeferredTask::RecoverTxs(txs) => {
                                enqueue_recover_txs(ordered_resolve_queue, txs).await;
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
                            error!("deferred task worker panicked: {}; respawning", message);
                            // If a RecoverTxs task panicked, retry it directly
                            // without going back through the channel.
                            if let Some(txs) = recover_txs_for_retry {
                                enqueue_recover_txs(ordered_resolve_queue_retry, txs).await;
                            }
                        }
                    }
                }
                info!("deferred task worker exited (channel closed)");
            });
        }

        #[cfg(feature = "pipeline")]
        {
            for _ in 0..pre_check_workers {
                let svc = service.clone();
                let queue = Arc::clone(&pre_check_queue);
                let cancel = pre_check_cancel.child_token();
                self.handle.spawn(async move {
                    loop {
                        let svc = svc.clone();
                        let queue = Arc::clone(&queue);
                        let worker = async move {
                            while let Some(job) = queue.pop().await {
                                let _ = svc
                                    .classify_and_enqueue_tx(job.tx, job.is_proposal_tx, job.remote)
                                    .await;
                            }
                        };
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
                                error!(
                                    "tx-pool pre-check worker panicked: {}; respawning",
                                    message
                                );
                            }
                        }
                    }
                });
            }
        }

        let chunk_rx_for_resolvers = self.chunk_rx.clone();
        let mut verify_mgr =
            VerifyMgr::new(service.clone(), self.chunk_rx, self.signal_receiver.clone());
        self.handle.spawn(async move { verify_mgr.run().await });

        let resolver_exit_signal = self.signal_receiver.child_token();
        let service_for_resolver = service.clone();
        self.handle.spawn(async move {
            loop {
                let resolver = crate::resolve_mgr::OrderedResolver::new(
                    service_for_resolver.clone(),
                    Arc::clone(&ordered_resolve_queue),
                    Arc::clone(&verify_queue),
                    chunk_rx_for_resolvers.clone(),
                    resolver_exit_signal.clone(),
                );
                let (exit_tx, mut exit_rx) = tokio::sync::mpsc::unbounded_channel();
                let handle = resolver.start(exit_tx);

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
                    }
                }
            }
            info!("TxPool ordered resolver monitor exited");
        });

        let mut receiver = self.receiver;
        let mut reorg_receiver = self.reorg_receiver;
        let handle_clone = self.handle.clone();

        let process_service = service.clone();
        let signal_receiver = self.signal_receiver.clone();
        let max_workers = service.tx_pool_config.max_tx_verify_workers.max(1);
        let semaphore = Arc::new(Semaphore::new(
            max_workers * crate::constants::MESSAGE_CONCURRENCY_MULTIPLIER,
        ));
        self.handle.spawn(async move {
            loop {
                tokio::select! {
                    Some(message) = receiver.recv() => {
                        let service_clone = process_service.clone();
                        let permit = Arc::clone(&semaphore).acquire_owned().await.unwrap();
                        handle_clone.spawn(async move {
                            let _permit = permit;
                            process(service_clone, message).await;
                        });
                    },
                    _ = signal_receiver.cancelled() => {
                        info!("TxPool is draining in-flight tasks...");
                        // Wait for all in-flight message-processing tasks to
                        // complete before persisting the pool state.  The
                        // semaphore bounds concurrent message handlers at
                        // max_workers * crate::constants::MESSAGE_CONCURRENCY_MULTIPLIER, so acquiring all permits guarantees
                        // no handler is still running.
                        let _ = semaphore
                            .acquire_many(max_workers as u32 * crate::constants::MESSAGE_CONCURRENCY_MULTIPLIER as u32)
                            .await;
                        info!("TxPool is saving, please wait...");
                        process_service.save_pool().await;
                        info!("TxPool process_service exit now");
                        break
                    },
                    else => break,
                }
            }
        });

        let process_service = service.clone();
        if let Some(ref block_assembler) = service.block_assembler {
            let signal_receiver = self.signal_receiver.clone();
            let interval = Duration::from_millis(block_assembler.config.update_interval_millis);
            if interval.is_zero() {
                // block_assembler.update_interval_millis set zero interval should only be used for tests,
                // external notification will be disabled.
                ckb_logger::warn!(
                    "block_assembler.update_interval_millis set to zero interval. \
                    This should only be used for tests, as external notification will be disabled."
                );
                self.handle.spawn(async move {
                    loop {
                        tokio::select! {
                            Some(message) = block_assembler_receiver.recv() => {
                                let service_clone = process_service.clone();
                                block_assembler::process(service_clone, &message).await;
                            },
                            _ = signal_receiver.cancelled() => {
                                info!("TxPool block_assembler process service received exit signal, exit now");
                                break
                            },
                            else => break,
                        }
                    }
                });
            } else {
                self.handle.spawn(async move {
                    let mut interval = tokio::time::interval(interval);
                    let mut queue = LinkedHashSet::new();
                    loop {
                        tokio::select! {
                            Some(message) = block_assembler_receiver.recv() => {
                                if let BlockAssemblerMessage::Reset(..) = message {
                                    let service_clone = process_service.clone();
                                    queue.clear();
                                    block_assembler::process(service_clone, &message).await;
                                } else {
                                    queue.insert(message);
                                }
                            },
                            _ = interval.tick() => {
                                for message in &queue {
                                    let service_clone = process_service.clone();
                                    block_assembler::process(service_clone, message).await;
                                }
                                if !queue.is_empty()
                                    && let Some(ref block_assembler) = process_service.block_assembler {
                                        block_assembler.notify().await;
                                    }
                                queue.clear();
                            }
                            _ = signal_receiver.cancelled() => {
                                info!("TxPool block_assembler process service received exit signal, exit now");
                                break
                            },
                            else => break,
                        }
                    }
                });
            }
        }

        let signal_receiver = self.signal_receiver;
        self.handle.spawn(async move {
            loop {
                let service = service.clone();
                let reorg_result = AssertUnwindSafe(async {
                    tokio::select! {
                        Some(message) = reorg_receiver.recv() => {
                            let Notify {
                                arguments: (detached_blocks, attached_blocks, detached_proposal_id, snapshot),
                            } = message;
                            let snapshot_clone = Arc::clone(&snapshot);
                            let detached_blocks_clone = detached_blocks.clone();
                            service.update_block_assembler_before_tx_pool_reorg(
                                detached_blocks_clone,
                                snapshot_clone
                            ).await;

                            let snapshot_clone = Arc::clone(&snapshot);
                            service
                            .update_tx_pool_for_reorg(
                                detached_blocks,
                                attached_blocks,
                                detached_proposal_id,
                                snapshot_clone,
                            )
                            .await;

                            service.update_block_assembler_after_tx_pool_reorg().await;
                            true
                        },
                        _ = signal_receiver.cancelled() => {
                            info!("TxPool reorg process service received exit signal, exit now");
                            false
                        },
                        else => false,
                    }
                })
                .catch_unwind()
                .await;

                match reorg_result {
                    Ok(true) => continue,
                    Ok(false) => break,
                    Err(payload) => {
                        let message = panic_payload_to_string(payload.as_ref());
                        error!("tx-pool reorg handler panicked: {}; respawning", message);
                        continue;
                    }
                }
            }
        });
        self.started.store(true, Ordering::Release);
        if let Err(err) = self.tx_pool_controller.load_persisted_data(txs) {
            error!("Failed to import persistent txs, cause: {}", err);
        }
    }

    /// Build a bare [`TxPoolService`] and its supporting queues **without**
    /// spawning any background workers (pre-check pool, [`VerifyMgr`],
    /// [`OrderedResolver`]).
    ///
    /// This is the single source of truth for service construction used by
    /// both [`Self::start`] (production) and the benchmark harness.  The
    /// caller is responsible for spawning the pipeline workers that the
    /// returned service depends on.
    #[cfg(feature = "internal")]
    pub(crate) fn build_bench_service(self, network: TxPoolNetworkHandle) -> BenchServiceParts {
        let consensus = self.snapshot.cloned_consensus();

        let ordered_resolve_queue = Arc::new(RwLock::new(
            crate::component::ordered_resolve_queue::OrderedResolveQueue::new(),
        ));
        let verify_queue = Arc::new(RwLock::new(VerifyQueue::new(
            self.tx_pool_config.max_tx_verify_cycles,
            self.tx_pool_config.verify_ordering,
        )));

        let tx_pool = TxPool::new(self.tx_pool_config, self.snapshot);
        let recent_reject = self.recent_reject;
        let (block_assembler_sender, _) = self.block_assembler_channel;

        #[cfg(feature = "pipeline")]
        let signal = self.signal_receiver;
        #[cfg(feature = "pipeline")]
        let pre_check_cancel = signal.child_token();
        #[cfg(feature = "pipeline")]
        let pre_check_queue = Arc::new(crate::component::pre_check_queue::PreCheckQueue::new(
            pre_check_cancel,
        ));

        let (deferred_sender, deferred_receiver) =
            mpsc::channel::<DeferredTask>(crate::constants::DEFERRED_CHANNEL_SIZE);

        let service = TxPoolService {
            tx_pool_config: Arc::new(tx_pool.config.clone()),
            tx_pool: Arc::new(RwLock::new(tx_pool)),
            orphan: Arc::new(RwLock::new(OrphanPool::new())),
            block_assembler: self.block_assembler,
            txs_verify_cache: self.txs_verify_cache,
            callbacks: Arc::new(self.callbacks),
            tx_relay_sender: self.tx_relay_sender,
            block_assembler_sender,
            ordered_resolve_queue: Arc::clone(&ordered_resolve_queue),
            verify_queue: Arc::clone(&verify_queue),
            network,
            consensus,
            fee_estimator: self.fee_estimator,
            recent_reject,
            #[cfg(feature = "pipeline")]
            pre_check_queue: Arc::clone(&pre_check_queue),
            #[cfg(feature = "pipeline")]
            chunk_rx: self.chunk_rx,
            #[cfg(feature = "pipeline")]
            rbf_candidates: Arc::new(RwLock::new(RbfCandidates::new())),
            deferred_sender,
        };

        BenchServiceParts {
            service,
            #[cfg(feature = "pipeline")]
            signal,
            #[cfg(feature = "pipeline")]
            pre_check_queue,
            deferred_receiver,
        }
    }
}

/// Components returned by [`TxPoolServiceBuilder::build_bench_service`].
///
/// The caller must spawn the pre-check workers, [`VerifyMgr`], and
/// [`OrderedResolver`] using these components.
#[cfg(feature = "internal")]
pub(crate) struct BenchServiceParts {
    pub service: TxPoolService,
    #[cfg(feature = "pipeline")]
    pub signal: CancellationToken,
    #[cfg(feature = "pipeline")]
    pub pre_check_queue: Arc<crate::component::pre_check_queue::PreCheckQueue>,
    pub deferred_receiver: mpsc::Receiver<DeferredTask>,
}

#[derive(Clone)]
pub(crate) struct TxPoolService {
    pub(crate) tx_pool: Arc<RwLock<TxPool>>,
    pub(crate) orphan: Arc<RwLock<OrphanPool>>,
    pub(crate) consensus: Arc<Consensus>,
    pub(crate) tx_pool_config: Arc<TxPoolConfig>,
    pub(crate) block_assembler: Option<BlockAssembler>,
    pub(crate) txs_verify_cache: Arc<RwLock<TxVerificationCache>>,
    pub(crate) callbacks: Arc<Callbacks>,
    pub(crate) network: TxPoolNetworkHandle,
    pub(crate) tx_relay_sender: ckb_channel::Sender<TxVerificationResult>,
    pub(crate) ordered_resolve_queue:
        Arc<RwLock<crate::component::ordered_resolve_queue::OrderedResolveQueue>>,
    pub(crate) verify_queue: Arc<RwLock<VerifyQueue>>,
    pub(crate) block_assembler_sender: mpsc::Sender<BlockAssemblerMessage>,
    pub(crate) fee_estimator: FeeEstimator,
    /// Lock-free recent-reject database (RocksDB with TTL).
    /// Owned by the service rather than `TxPool` so that `put` / `get` never
    /// need the tx-pool write lock.
    pub(crate) recent_reject: Option<Arc<RecentReject>>,
    /// Queue used to offload independent tx classification to a fixed-size
    /// worker pool.  Dependent txs are still handled synchronously in the
    /// service actor to preserve ordering.
    #[cfg(feature = "pipeline")]
    pub(crate) pre_check_queue: Arc<crate::component::pre_check_queue::PreCheckQueue>,
    /// Chunk command receiver used by the synchronous reorg recovery path so
    /// that detached transactions are not verified while the pipeline is
    /// suspended.
    #[cfg(feature = "pipeline")]
    pub(crate) chunk_rx: watch::Receiver<ChunkCommand>,
    /// Fee-ordering gate for conflicting RBF replacements that are concurrently
    /// in flight through the pipeline.  Ensures the highest-fee candidate wins.
    #[cfg(feature = "pipeline")]
    pub(crate) rbf_candidates: Arc<RwLock<RbfCandidates>>,
    /// Bounded channel for deferred side-effects (recovery tx re-enqueue,
    /// verify cache updates). A single background worker drains this channel,
    /// preventing unbounded task accumulation under high RBF frequency.
    pub(crate) deferred_sender: mpsc::Sender<DeferredTask>,
}

/// Location and metadata of a transaction found in the pipeline queues.
pub(crate) enum PipelineTxLocation {
    /// In the pre-check queue (awaiting initial resolution).
    #[cfg(feature = "pipeline")]
    PreChecking { tx: TransactionView },
    /// In the ordered resolve queue (not yet resolved/verified).
    Ordered { tx: TransactionView },
    /// In the verify queue (resolved, awaiting verification).
    Verifying {
        tx: TransactionView,
        fee: Capacity,
        status: crate::process::TxStatus,
    },
    /// In the orphan pool (missing inputs).
    Orphan { tx: TransactionView, cycle: Cycle },
}

impl TxPoolService {
    /// Search the pipeline queues for a transaction by short id.
    pub(crate) async fn find_tx_in_pipeline(
        &self,
        id: &ProposalShortId,
    ) -> Option<PipelineTxLocation> {
        #[cfg(feature = "pipeline")]
        {
            if let Some(tx) = self.pre_check_queue.get_tx(id) {
                return Some(PipelineTxLocation::PreChecking { tx });
            }
        }
        {
            let ordered = self.ordered_resolve_queue.read().await;
            if let Some(tx) = ordered.get_tx(id) {
                return Some(PipelineTxLocation::Ordered { tx: tx.clone() });
            }
        }
        {
            let verify_queue = self.verify_queue.read().await;
            if let Some(resolved) = verify_queue.get_tx_by_id(id) {
                return Some(PipelineTxLocation::Verifying {
                    tx: resolved.tx.clone(),
                    fee: resolved.fee,
                    status: resolved.status,
                });
            }
        }
        {
            let orphan = self.orphan.read().await;
            if let Some(entry) = orphan.get(id) {
                return Some(PipelineTxLocation::Orphan {
                    tx: entry.tx.clone(),
                    cycle: entry.cycle,
                });
            }
        }
        None
    }
}

/// Deferred side-effects that are processed by a single background worker
/// instead of fire-and-forget `tokio::spawn` calls.
pub(crate) enum DeferredTask {
    /// Push RBF-displaced transactions back into the ordered resolve queue
    /// so they can be re-resolved, re-verified and re-submitted.
    RecoverTxs(Vec<TransactionView>),
    /// Store a successful verification result in the cache (keyed by wtx_hash).
    CacheUpdate {
        wtx_hash: Byte32,
        verified: ckb_verification::cache::Completed,
    },
}

/// tx verification result
#[derive(Debug)]
pub enum TxVerificationResult {
    /// tx is verified
    Ok {
        /// original peer
        original_peer: Option<PeerIndex>,
        /// transaction hash
        tx_hash: Byte32,
    },
    /// tx parent is unknown
    UnknownParents {
        /// original peer
        peer: PeerIndex,
        /// parents hashes
        parents: HashSet<Byte32>,
    },
    /// tx is rejected
    Reject {
        /// transaction hash
        tx_hash: Byte32,
    },
}

async fn process(mut service: TxPoolService, message: Message) {
    match message {
        Message::GetTxPoolInfo(req) => service.handle_get_tx_pool_info(req).await,
        Message::GetLiveCell(req) => service.handle_get_live_cell(req).await,
        Message::BlockTemplate(req) => service.handle_block_template(req).await,
        Message::SubmitLocalTx(req) => service.handle_submit_local_tx(req).await,
        Message::SubmitLocalTestTx(req) => service.handle_submit_local_test_tx(req).await,
        Message::RemoveLocalTx(req) => service.handle_remove_local_tx(req).await,
        Message::TestAcceptTx(req) => service.handle_test_accept_tx(req).await,
        Message::SubmitRemoteTx(req) => service.handle_submit_remote_tx(req).await,
        Message::NotifyTxs(req) => service.handle_notify_txs(req).await,
        Message::FreshProposalsFilter(req) => service.handle_fresh_proposals_filter(req).await,
        Message::GetTxStatus(req) => service.handle_get_tx_status(req).await,
        Message::GetTransactionWithStatus(req) => {
            service.handle_get_transaction_with_status(req).await;
        }
        Message::FetchTxs(req) => service.handle_fetch_txs(req).await,
        Message::FetchTxsWithCycles(req) => service.handle_fetch_txs_with_cycles(req).await,
        Message::NewUncle(req) => service.handle_new_uncle(req).await,
        Message::ClearPool(req) => service.handle_clear_pool(req).await,
        Message::ClearPipeline(req) => service.handle_clear_pipeline(req).await,
        Message::GetPoolTxDetails(req) => service.handle_get_pool_tx_details(req).await,
        Message::GetAllEntryInfo(req) => service.handle_get_all_entry_info(req).await,
        Message::GetAllIds(req) => service.handle_get_all_ids(req).await,
        Message::SavePool(req) => service.handle_save_pool(req).await,
        Message::UpdateIBDState(req) => service.handle_update_ibd_state(req).await,
        Message::EstimateFeeRate(req) => service.handle_estimate_fee_rate(req).await,
        #[cfg(feature = "internal")]
        Message::PlugEntry(req) => service.handle_plug_entry(req).await,
        #[cfg(feature = "internal")]
        Message::PackageTxs(req) => service.handle_package_txs(req).await,
        Message::GetTotalRecentRejectNum(req) => {
            service.handle_get_total_recent_reject_num(req).await;
        }
    }
}

impl TxPoolService {
    async fn handle_get_tx_pool_info(&self, req: SyncRequest<(), TxPoolInfo>) {
        let SyncRequest { responder, .. } = req;
        let info = self.info().await;
        respond(responder, info, "get_tx_pool_info");
    }

    async fn handle_get_live_cell(&self, req: SyncRequest<(OutPoint, bool), CellStatus>) {
        let SyncRequest {
            responder,
            arguments: (out_point, with_data),
        } = req;
        let live_cell_status = self.get_live_cell(out_point, with_data).await;
        respond(responder, live_cell_status, "get_live_cell");
    }

    async fn handle_block_template(
        &self,
        req: SyncRequest<BlockTemplateArgs, BlockTemplateResult>,
    ) {
        let SyncRequest { responder, .. } = req;
        let block_template_result = self.get_block_template().await;
        respond(responder, block_template_result, "block_template_result");
    }

    async fn handle_submit_local_tx(&self, req: SyncRequest<TransactionView, SubmitTxResult>) {
        let SyncRequest {
            responder,
            arguments: tx,
        } = req;
        let result = self.process_tx(tx, None).await.map(|_| ());
        respond(responder, result, "submit_local_tx");
    }

    async fn handle_submit_local_test_tx(&self, req: SyncRequest<TransactionView, SubmitTxResult>) {
        let SyncRequest {
            responder,
            arguments: tx,
        } = req;
        let result = self
            .resumable_process_tx_sync(tx, false, None)
            .await
            .map(|_| ());
        respond(responder, result, "submit_local_test_tx");
    }

    async fn handle_remove_local_tx(&self, req: SyncRequest<Byte32, bool>) {
        let SyncRequest {
            responder,
            arguments: tx_hash,
        } = req;
        let result = self.remove_tx(tx_hash).await;
        respond(responder, result, "remove_tx");
    }

    async fn handle_test_accept_tx(&self, req: SyncRequest<TransactionView, TestAcceptTxResult>) {
        let SyncRequest {
            responder,
            arguments: tx,
        } = req;
        let result = self.test_accept_tx(tx).await;
        respond(responder, result.map(|r| r.into()), "test_accept_tx");
    }

    async fn handle_submit_remote_tx(
        &self,
        req: SyncRequest<(TransactionView, Cycle, PeerIndex), ()>,
    ) {
        let SyncRequest {
            responder,
            arguments: (tx, declared_cycles, peer),
        } = req;
        let _result = self.submit_remote_tx(tx, declared_cycles, peer).await;
        respond(responder, (), "submit_remote_tx");
    }

    async fn handle_notify_txs(&self, req: Notify<Vec<TransactionView>>) {
        let Notify { arguments: txs } = req;
        for tx in txs {
            let _ret = self.notify_tx(tx).await;
        }
    }

    async fn handle_fresh_proposals_filter(
        &self,
        req: AsyncRequest<Vec<ProposalShortId>, Vec<ProposalShortId>>,
    ) {
        let AsyncRequest {
            responder,
            arguments: proposals,
        } = req;
        let new_proposals = self.exclude_existing_proposal(proposals).await;
        respond_async(responder, new_proposals, "fresh_proposals_filter");
    }

    async fn handle_get_tx_status(&self, req: SyncRequest<Byte32, GetTxStatusResult>) {
        let SyncRequest {
            responder,
            arguments: hash,
        } = req;
        let id = ProposalShortId::from_tx_hash(&hash);
        let pool_cycles = {
            let tx_pool = self.tx_pool.read().await;
            tx_pool
                .pool_map
                .get_by_id(&id)
                .map(|entry| (entry.status, entry.inner.cycles))
        };
        let ret = if let Some((status, cycles)) = pool_cycles {
            let status = if status == Status::Proposed {
                TxStatus::Proposed
            } else {
                TxStatus::Pending
            };
            Ok((status, Some(cycles)))
        } else if self.find_tx_in_pipeline(&id).await.is_some() {
            Ok((TxStatus::Pending, None))
        } else {
            self.lookup_recent_reject(
                &hash,
                |record| (TxStatus::Rejected(record), None),
                || (TxStatus::Unknown, None),
            )
            .await
        };
        respond(responder, ret, "get_tx_status");
    }

    async fn handle_get_transaction_with_status(
        &self,
        req: SyncRequest<Byte32, GetTransactionWithStatusResult>,
    ) {
        let SyncRequest {
            responder,
            arguments: hash,
        } = req;
        let id = ProposalShortId::from_tx_hash(&hash);
        let pool_entry = {
            let tx_pool = self.tx_pool.read().await;
            tx_pool.pool_map.get_by_id(&id).map(|entry| {
                let status = entry.status;
                let entry = entry.inner.clone();
                let min_replace_fee = if status == Status::Proposed {
                    None
                } else {
                    tx_pool.min_replace_fee(&entry)
                };
                (status, entry, min_replace_fee)
            })
        };
        let ret = if let Some((status, entry, min_replace_fee)) = pool_entry {
            let tx_status = if status == Status::Proposed {
                TxStatus::Proposed
            } else {
                TxStatus::Pending
            };
            Ok(TransactionWithStatus::with_status(
                Some(entry.transaction().clone()),
                entry.cycles,
                entry.timestamp,
                tx_status,
                Some(entry.fee),
                min_replace_fee,
            ))
        } else if let Some(location) = self.find_tx_in_pipeline(&id).await {
            let (tx, tx_status, cycles, fee) = match location {
                #[cfg(feature = "pipeline")]
                PipelineTxLocation::PreChecking { tx } => (tx, TxStatus::Pending, None, None),
                PipelineTxLocation::Ordered { tx } => (tx, TxStatus::Pending, None, None),
                PipelineTxLocation::Verifying { tx, fee, status } => {
                    let tx_status = if status == crate::process::TxStatus::Proposed {
                        TxStatus::Proposed
                    } else {
                        TxStatus::Pending
                    };
                    (tx, tx_status, None, Some(fee))
                }
                PipelineTxLocation::Orphan { tx, cycle } => {
                    (tx, TxStatus::Pending, Some(cycle), None)
                }
            };
            Ok(TransactionWithStatus {
                transaction: Some(tx),
                tx_status,
                cycles,
                fee,
                min_replace_fee: None,
                time_added_to_pool: None,
            })
        } else {
            self.lookup_recent_reject(
                &hash,
                TransactionWithStatus::with_rejected,
                TransactionWithStatus::with_unknown,
            )
            .await
        };
        respond(responder, ret, "get_transaction_with_status");
    }

    async fn handle_fetch_txs(
        &self,
        req: AsyncRequest<HashSet<ProposalShortId>, HashMap<ProposalShortId, TransactionView>>,
    ) {
        let AsyncRequest {
            responder,
            arguments: short_ids,
        } = req;
        let txs_map = self.get_tx_for_compact_block(short_ids).await;
        respond_async(responder, txs_map, "fetch_txs");
    }

    async fn handle_fetch_txs_with_cycles(
        &self,
        req: AsyncRequest<HashSet<ProposalShortId>, FetchTxsWithCyclesResult>,
    ) {
        let AsyncRequest {
            responder,
            arguments: short_ids,
        } = req;
        let tx_pool = self.tx_pool.read().await;
        let txs = short_ids
            .into_iter()
            .filter_map(|short_id| {
                tx_pool
                    .get_tx_with_cycles(&short_id)
                    .map(|(tx, cycles)| (short_id, (tx, cycles)))
            })
            .collect();
        respond_async(responder, txs, "fetch_txs_with_cycles");
    }

    async fn handle_new_uncle(&self, req: Notify<UncleBlockView>) {
        let Notify { arguments: uncle } = req;
        self.receive_candidate_uncle(uncle).await;
    }

    async fn handle_clear_pool(&mut self, req: SyncRequest<Arc<Snapshot>, ()>) {
        let SyncRequest {
            responder,
            arguments: new_snapshot,
        } = req;
        self.clear_pool(new_snapshot).await;
        respond(responder, (), "clear_pool");
    }

    async fn handle_clear_pipeline(&self, req: SyncRequest<(), ()>) {
        let SyncRequest { responder, .. } = req;
        self.ordered_resolve_queue.write().await.clear();
        self.verify_queue.write().await.clear();
        self.orphan.write().await.clear();
        #[cfg(feature = "pipeline")]
        self.pre_check_queue.clear();
        #[cfg(feature = "pipeline")]
        self.rbf_candidates.write().await.clear();
        respond(responder, (), "clear_pipeline");
    }

    async fn handle_get_pool_tx_details(&self, req: SyncRequest<Byte32, PoolTxDetailInfo>) {
        let SyncRequest {
            responder,
            arguments: tx_hash,
        } = req;
        let tx_pool = self.tx_pool.read().await;
        let id = ProposalShortId::from_tx_hash(&tx_hash);
        let tx_details = tx_pool
            .get_tx_detail(&id)
            .unwrap_or(PoolTxDetailInfo::with_unknown());
        respond(responder, tx_details, "get_pool_tx_details");
    }

    async fn handle_get_all_entry_info(&self, req: SyncRequest<(), TxPoolEntryInfo>) {
        let SyncRequest { responder, .. } = req;
        let tx_pool = self.tx_pool.read().await;
        let info = tx_pool.get_all_entry_info();
        respond(responder, info, "get_all_entry_info");
    }

    async fn handle_get_all_ids(&self, req: SyncRequest<(), TxPoolIds>) {
        let SyncRequest { responder, .. } = req;
        let tx_pool = self.tx_pool.read().await;
        let ids = tx_pool.get_ids();
        respond(responder, ids, "get_ids");
    }

    async fn handle_save_pool(&self, req: SyncRequest<(), ()>) {
        let SyncRequest { responder, .. } = req;
        self.save_pool().await;
        respond(responder, (), "save_pool");
    }

    async fn handle_update_ibd_state(&self, req: SyncRequest<bool, ()>) {
        let SyncRequest {
            responder,
            arguments: in_ibd,
        } = req;
        self.update_ibd_state(in_ibd).await;
        respond(responder, (), "update_ibd_state");
    }

    async fn handle_estimate_fee_rate(
        &self,
        req: SyncRequest<(EstimateMode, bool), FeeEstimatesResult>,
    ) {
        let SyncRequest {
            responder,
            arguments: (estimate_mode, enable_fallback),
        } = req;
        let fee_estimates_result = self.estimate_fee_rate(estimate_mode, enable_fallback).await;
        respond(responder, fee_estimates_result, "fee_estimates_result");
    }

    #[cfg(feature = "internal")]
    async fn handle_plug_entry(&self, req: SyncRequest<(Vec<TxEntry>, PlugTarget), ()>) {
        let SyncRequest {
            responder,
            arguments: (entries, target),
        } = req;
        self.plug_entry(entries, target).await;
        respond(responder, (), "plug_entry");
    }

    #[cfg(feature = "internal")]
    async fn handle_package_txs(&self, req: SyncRequest<Option<u64>, Vec<TxEntry>>) {
        let SyncRequest {
            responder,
            arguments: bytes_limit,
        } = req;
        let max_block_cycles = self.consensus.max_block_cycles();
        let max_block_bytes = self.consensus.max_block_bytes();
        let tx_pool = self.tx_pool.read().await;
        let (txs, _size, _cycles) = tx_pool.package_txs(
            max_block_cycles,
            bytes_limit.unwrap_or(max_block_bytes) as usize,
        );
        respond(responder, txs, "package_txs");
    }

    async fn handle_get_total_recent_reject_num(&self, req: SyncRequest<(), Option<u64>>) {
        let SyncRequest { responder, .. } = req;
        let total_recent_reject_num = self.get_total_recent_reject_num();
        respond(
            responder,
            total_recent_reject_num,
            "total_recent_reject_num",
        );
    }

    /// Tx-pool information
    async fn info(&self) -> TxPoolInfo {
        let tx_pool = self.tx_pool.read().await;
        let orphan = self.orphan.read().await;
        let verify_queue = self.verify_queue.read().await;
        let tip_header = tx_pool.snapshot.tip_header();
        TxPoolInfo {
            tip_hash: tip_header.hash(),
            tip_number: tip_header.number(),
            pending_size: tx_pool.pool_map.pending_size(),
            proposed_size: tx_pool.pool_map.proposed_size(),
            orphan_size: orphan.len(),
            total_tx_size: tx_pool.pool_map.stats.total_tx_size.get(),
            total_tx_cycles: tx_pool.pool_map.stats.total_tx_cycles.get(),
            min_fee_rate: self.tx_pool_config.min_fee_rate,
            min_rbf_rate: self.tx_pool_config.min_rbf_rate,
            last_txs_updated_at: tx_pool.pool_map.get_max_update_time(),
            tx_size_limit: TRANSACTION_SIZE_LIMIT,
            max_tx_pool_size: self.tx_pool_config.max_tx_pool_size as u64,
            verify_queue_size: verify_queue.len(),
        }
    }

    fn get_total_recent_reject_num(&self) -> Option<u64> {
        self.recent_reject
            .as_ref()
            .map(|r| r.get_estimate_total_keys_num())
    }

    /// Look up a transaction hash in the recent-reject database.
    ///
    /// Returns `on_rejected(record)` if the tx was recently rejected,
    /// `on_unknown()` if it is unknown or there is no recent-reject db.
    async fn lookup_recent_reject<T>(
        &self,
        hash: &Byte32,
        on_rejected: impl FnOnce(String) -> T,
        on_unknown: impl FnOnce() -> T,
    ) -> Result<T, AnyError> {
        if let Some(ref db) = self.recent_reject {
            match db.get(hash) {
                Ok(Some(record)) => Ok(on_rejected(record)),
                Ok(_) => Ok(on_unknown()),
                Err(err) => Err(err),
            }
        } else {
            Ok(on_unknown())
        }
    }

    /// Get Live Cell Status
    async fn get_live_cell(&self, out_point: OutPoint, eager_load: bool) -> CellStatus {
        let tx_pool = self.tx_pool.read().await;
        let snapshot = tx_pool.snapshot();
        let pool_cell = PoolCell::new(&tx_pool.pool_map, false);
        let provider = OverlayCellProvider::new(&pool_cell, snapshot);

        match provider.cell(&out_point, false) {
            CellStatus::Live(mut cell_meta) => {
                if eager_load && let Some((data, data_hash)) = snapshot.get_cell_data(&out_point) {
                    cell_meta.mem_cell_data = Some(data);
                    cell_meta.mem_cell_data_hash = Some(data_hash);
                }
                CellStatus::live_cell(cell_meta)
            }
            _ => CellStatus::Unknown,
        }
    }

    pub fn should_notify_block_assembler(&self) -> bool {
        self.block_assembler.is_some()
    }

    /// Excludes proposals that already exist in the tx-pool or any pipeline queue.
    ///
    /// Any proposal that appears in any of the following structures is considered
    /// "already exists" and will be filtered out:
    /// - already accepted and stored in the main pool (`pool_map`),
    /// - waiting for missing parents in the `orphan` pool,
    /// - waiting for resolution/verification in the `ordered_resolve_queue`,
    /// - undergoing pre-check in the `pre_check_queue` (pipeline),
    /// - currently being verified (`verify_queue`).
    ///
    /// # Returns
    ///
    /// A new `Vec<ProposalShortId>` containing only the proposals that are **completely new**.
    pub async fn exclude_existing_proposal(
        &self,
        mut proposals: Vec<ProposalShortId>,
    ) -> Vec<ProposalShortId> {
        {
            let ordered = self.ordered_resolve_queue.read().await;
            proposals.retain(|id| !ordered.contains_key(id));
        }
        #[cfg(feature = "pipeline")]
        {
            proposals.retain(|id| !self.pre_check_queue.contains_key(id));
        }
        {
            let verify_queue = self.verify_queue.read().await;
            proposals.retain(|id| !verify_queue.contains_key(id));
        }
        {
            let orphan = self.orphan.read().await;
            proposals.retain(|id| !orphan.contains_key(id));
        }
        {
            let tx_pool = self.tx_pool.read().await;
            proposals.retain(|id| !tx_pool.contains_proposal_id(id));
        }
        proposals
    }

    /// Retrieves transactions required for compact block reconstruction.
    ///
    /// During compact block relay, a node may receive a block that contains transactions
    /// still being verified and not yet present in the main mempool. This method searches
    /// **both** primary locations where a transaction can reside when its short ID is known:
    ///
    /// 1. `pool_map` – the main mempool (already accepted transactions)
    /// 2. `verify_queue` – transactions currently undergoing background validation
    /// 3. `orphan_pool`   – Orphan transactions that are waiting for missing parents
    ///
    /// # Returns
    /// A map containing only the transactions that were found, keyed by their short ID.
    /// Missing entries are simply omitted (caller should treat absence as "need to request")
    /// Returning a `HashMap` allows the caller (compact block reconstructor) to:
    /// - Immediately obtain all locally-available transactions in a single call
    /// - Quickly identify which short IDs are missing
    pub async fn get_tx_for_compact_block(
        &self,
        short_ids: HashSet<ProposalShortId>,
    ) -> HashMap<ProposalShortId, TransactionView> {
        let mut txs = HashMap::with_capacity(short_ids.len());
        {
            let verify_queue = self.verify_queue.read().await;
            txs.extend(short_ids.iter().filter_map(|short_id| {
                verify_queue
                    .get_tx_by_id(short_id)
                    .map(|resolved| (short_id.to_owned(), resolved.tx.to_owned()))
            }));
        }
        {
            let orphan = self.orphan.read().await;
            txs.extend(short_ids.iter().filter_map(|short_id| {
                orphan
                    .get(short_id)
                    .map(|entry| (short_id.to_owned(), entry.tx.to_owned()))
            }));
        }
        {
            let tx_pool = self.tx_pool.read().await;
            txs.extend(short_ids.iter().filter_map(|short_id| {
                tx_pool
                    .get_tx_from_pool_or_store(short_id)
                    .map(|tx| (short_id.to_owned(), tx))
            }));
        }
        txs
    }

    pub async fn receive_candidate_uncle(&self, uncle: UncleBlockView) {
        if let Some(ref block_assembler) = self.block_assembler {
            {
                block_assembler.candidate_uncles.lock().await.insert(uncle);
            }
            if self
                .block_assembler_sender
                .send(BlockAssemblerMessage::Uncle)
                .await
                .is_err()
            {
                error!("block_assembler receiver dropped");
            }
        }
    }

    pub async fn update_block_assembler_before_tx_pool_reorg(
        &self,
        detached_blocks: VecDeque<BlockView>,
        snapshot: Arc<Snapshot>,
    ) {
        if let Some(ref block_assembler) = self.block_assembler {
            {
                let mut candidate_uncles = block_assembler.candidate_uncles.lock().await;
                for detached_block in detached_blocks {
                    candidate_uncles.insert(detached_block.as_uncle());
                }
            }

            if let Err(e) = block_assembler.reset_template(snapshot, true).await {
                error!("block_assembler reset_template error {}", e);
            }
            block_assembler.notify().await;
        }
    }

    pub async fn update_block_assembler_after_tx_pool_reorg(&self) {
        if let Some(ref block_assembler) = self.block_assembler {
            if let Err(e) = block_assembler.update_full(&self.tx_pool).await {
                error!("block_assembler update failed {:?}", e);
            }
            block_assembler.notify().await;
        }
    }

    #[cfg(feature = "internal")]
    pub async fn plug_entry(&self, entries: Vec<TxEntry>, target: PlugTarget) {
        {
            let mut tx_pool = self.tx_pool.write().await;
            match target {
                PlugTarget::Pending => {
                    for entry in entries {
                        tx_pool
                            .add_pending(entry)
                            .expect("Plug entry add_pending error");
                    }
                }
                PlugTarget::Proposed => {
                    for entry in entries {
                        tx_pool
                            .add_proposed(entry)
                            .expect("Plug entry add_proposed error");
                    }
                }
            };
        }

        if self.should_notify_block_assembler() {
            let msg = match target {
                PlugTarget::Pending => BlockAssemblerMessage::Pending,
                PlugTarget::Proposed => BlockAssemblerMessage::Proposed,
            };
            if self.block_assembler_sender.send(msg).await.is_err() {
                error!("block_assembler receiver dropped");
            }
        }
    }
}
