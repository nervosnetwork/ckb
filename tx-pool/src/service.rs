//! Tx-pool background service

use crate::block_assembler::{self, BlockAssembler};
use crate::callback::{Callbacks, PendingCallback, ProposedCallback, RejectCallback};
use crate::component::orphan::OrphanPool;
use crate::component::pool_map::{PoolEntry, Status};
use crate::component::recent_reject::RecentReject;
use crate::component::verify_queue::VerifyQueue;
use crate::error::{handle_recv_error, handle_send_cmd_error, handle_try_send_error};
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
use ckb_network::{NetworkController, PeerIndex};
use ckb_script::ChunkCommand;
use ckb_snapshot::Snapshot;
use ckb_stop_handler::new_tokio_exit_rx;
use ckb_store::ChainStore;
use ckb_types::{
    core::{
        BlockView, Cycle, EstimateMode, FeeRate, TransactionView, UncleBlockView, Version,
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

pub(crate) struct Request<A, R> {
    pub responder: oneshot::Sender<R>,
    pub arguments: A,
}

impl<A, R> Request<A, R> {
    pub(crate) fn call(arguments: A, responder: oneshot::Sender<R>) -> Request<A, R> {
        Request {
            responder,
            arguments,
        }
    }
}

pub(crate) struct AsyncRequest<A, R> {
    pub responder: tokio::sync::oneshot::Sender<R>,
    pub arguments: A,
}

impl<A, R> AsyncRequest<A, R> {
    pub(crate) fn call(
        arguments: A,
        responder: tokio::sync::oneshot::Sender<R>,
    ) -> AsyncRequest<A, R> {
        AsyncRequest {
            responder,
            arguments,
        }
    }
}

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
    BlockTemplate(Request<BlockTemplateArgs, BlockTemplateResult>),
    SubmitLocalTx(Request<TransactionView, SubmitTxResult>),
    RemoveLocalTx(Request<Byte32, bool>),
    TestAcceptTx(Request<TransactionView, TestAcceptTxResult>),
    SubmitRemoteTx(Request<(TransactionView, Cycle, PeerIndex), ()>),
    NotifyTxs(Notify<Vec<TransactionView>>),
    FreshProposalsFilter(AsyncRequest<Vec<ProposalShortId>, Vec<ProposalShortId>>),
    FetchTxs(AsyncRequest<HashSet<ProposalShortId>, HashMap<ProposalShortId, TransactionView>>),
    FetchTxsWithCycles(AsyncRequest<HashSet<ProposalShortId>, FetchTxsWithCyclesResult>),
    GetTxPoolInfo(Request<(), TxPoolInfo>),
    GetLiveCell(Request<(OutPoint, bool), CellStatus>),
    GetTxStatus(Request<Byte32, GetTxStatusResult>),
    GetTransactionWithStatus(Request<Byte32, GetTransactionWithStatusResult>),
    NewUncle(Notify<UncleBlockView>),
    ClearPool(Request<Arc<Snapshot>, ()>),
    ClearVerifyQueue(Request<(), ()>),
    GetAllEntryInfo(Request<(), TxPoolEntryInfo>),
    GetAllIds(Request<(), TxPoolIds>),
    SavePool(Request<(), ()>),
    GetPoolTxDetails(Request<Byte32, PoolTxDetailInfo>),
    GetTotalRecentRejectNum(Request<(), Option<u64>>),

    UpdateIBDState(Request<bool, ()>),
    EstimateFeeRate(Request<(EstimateMode, bool), FeeEstimatesResult>),

    // test
    #[cfg(feature = "internal")]
    PlugEntry(Request<(Vec<TxEntry>, PlugTarget), ()>),
    #[cfg(feature = "internal")]
    PackageTxs(Request<Option<u64>, Vec<TxEntry>>),
    SubmitLocalTestTx(Request<TransactionView, SubmitTxResult>),
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

    /// Clears the tx-verify-queue.
    pub fn clear_verify_queue(&self) -> Result<(), AnyError> {
        send_message!(self, ClearVerifyQueue, ())
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
                u8::max(1, config.keep_rejected_tx_hashes_days) as i32 * 24 * 60 * 60;
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
    pub fn start(self, network: NetworkController) {
        let consensus = self.snapshot.cloned_consensus();

        let ordered_resolve_queue = Arc::new(RwLock::new(
            crate::component::ordered_resolve_queue::OrderedResolveQueue::new(),
        ));
        let verify_queue = Arc::new(RwLock::new(VerifyQueue::new(
            self.tx_pool_config.max_tx_verify_cycles,
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
        let pre_check_queue = Arc::new(crate::process::PreCheckQueue::new(pre_check_cancel));

        let (deferred_sender, deferred_receiver) = mpsc::channel::<DeferredTask>(1024);

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
                    let txs_verify_cache = Arc::clone(&txs_verify_cache);
                    let handler = async move {
                        match task {
                            DeferredTask::RecoverTxs(txs) => {
                                let mut queue = ordered_resolve_queue.write().await;
                                for tx in txs {
                                    debug!("recover back: {:?}", tx.proposal_short_id());
                                    if let Err(reject) =
                                        queue.add_tx(crate::resolved_tx::ResolveJob {
                                            tx,
                                            remote: None,
                                            is_proposal_tx: false,
                                            attempts: 0,
                                        })
                                    {
                                        warn!(
                                            "failed to recover tx back to ordered resolve queue: {}",
                                            reject
                                        );
                                    }
                                }
                            }
                            DeferredTask::CacheUpdate {
                                wtx_hash,
                                verified,
                            } => {
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
                            // Loop continues: receiver is still valid, next recv() picks up.
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
                self.handle.spawn(async move {
                    while let Some(job) = queue.pop().await {
                        let _ = svc
                            .classify_and_enqueue_tx(job.tx, job.is_proposal_tx, job.remote)
                            .await;
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
                    Some(exit) = exit_rx.recv() => {
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
        let semaphore = Arc::new(Semaphore::new(max_workers * 2));
        self.handle.spawn(async move {
            loop {
                tokio::select! {
                    Some(message) = receiver.recv() => {
                        let service_clone = process_service.clone();
                        let permit = semaphore.clone().acquire_owned().await.unwrap();
                        handle_clone.spawn(async move {
                            let _permit = permit;
                            process(service_clone, message).await;
                        });
                    },
                    _ = signal_receiver.cancelled() => {
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
                    },
                    _ = signal_receiver.cancelled() => {
                        info!("TxPool reorg process service received exit signal, exit now");
                        break
                    },
                    else => break,
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
    pub(crate) fn build_bench_service(self, network: NetworkController) -> BenchServiceParts {
        let consensus = self.snapshot.cloned_consensus();

        let ordered_resolve_queue = Arc::new(RwLock::new(
            crate::component::ordered_resolve_queue::OrderedResolveQueue::new(),
        ));
        let verify_queue = Arc::new(RwLock::new(VerifyQueue::new(
            self.tx_pool_config.max_tx_verify_cycles,
        )));

        let tx_pool = TxPool::new(self.tx_pool_config, self.snapshot);
        let recent_reject = self.recent_reject;
        let (block_assembler_sender, _) = self.block_assembler_channel;

        #[cfg(feature = "pipeline")]
        let signal = self.signal_receiver;
        #[cfg(feature = "pipeline")]
        let pre_check_cancel = signal.child_token();
        #[cfg(feature = "pipeline")]
        let pre_check_queue =
            Arc::new(crate::process::PreCheckQueue::new(pre_check_cancel));

        let (deferred_sender, deferred_receiver) = mpsc::channel::<DeferredTask>(1024);

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
    pub pre_check_queue: Arc<crate::process::PreCheckQueue>,
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
    pub(crate) network: NetworkController,
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
    pub(crate) pre_check_queue: Arc<crate::process::PreCheckQueue>,
    /// Bounded channel for deferred side-effects (recovery tx re-enqueue,
    /// verify cache updates). A single background worker drains this channel,
    /// preventing unbounded task accumulation under high RBF frequency.
    pub(crate) deferred_sender: mpsc::Sender<DeferredTask>,
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

#[allow(clippy::cognitive_complexity)]
async fn process(mut service: TxPoolService, message: Message) {
    match message {
        Message::GetTxPoolInfo(Request { responder, .. }) => {
            let info = service.info().await;
            if let Err(e) = responder.send(info) {
                error!("Responder sending get_tx_pool_info failed {:?}", e);
            };
        }
        Message::GetLiveCell(Request {
            responder,
            arguments: (out_point, with_data),
        }) => {
            let live_cell_status = service.get_live_cell(out_point, with_data).await;
            if let Err(e) = responder.send(live_cell_status) {
                error!("Responder sending get_live_cell failed {:?}", e);
            };
        }
        Message::BlockTemplate(Request {
            responder,
            arguments: (_bytes_limit, _proposals_limit, _max_version),
        }) => {
            let block_template_result = service.get_block_template().await;
            if let Err(e) = responder.send(block_template_result) {
                error!("Responder sending block_template_result failed {:?}", e);
            };
        }
        Message::SubmitLocalTx(Request {
            responder,
            arguments: tx,
        }) => {
            let result = service.process_tx(tx, None).await.map(|_| ());
            if let Err(e) = responder.send(result) {
                error!("Responder sending submit_tx result failed {:?}", e);
            };
        }
        Message::SubmitLocalTestTx(Request {
            responder,
            arguments: tx,
        }) => {
            let result = service
                .resumeble_process_tx(tx, false, None)
                .await
                .map(|_| ());
            if let Err(e) = responder.send(result) {
                error!("Responder sending submit_tx result failed {:?}", e);
            };
        }
        Message::RemoveLocalTx(Request {
            responder,
            arguments: tx_hash,
        }) => {
            let result = service.remove_tx(tx_hash).await;
            if let Err(e) = responder.send(result) {
                error!("Responder sending remove_tx result failed {:?}", e);
            };
        }
        Message::TestAcceptTx(Request {
            responder,
            arguments: tx,
        }) => {
            let result = service.test_accept_tx(tx).await;
            if let Err(e) = responder.send(result.map(|r| r.into())) {
                error!("Responder sending test_accept_tx result failed {:?}", e);
            };
        }
        Message::SubmitRemoteTx(Request {
            responder,
            arguments: (tx, declared_cycles, peer),
        }) => {
            let _result = service.submit_remote_tx(tx, declared_cycles, peer).await;
            if let Err(e) = responder.send(()) {
                error!("Responder sending submit_tx result failed {:?}", e);
            };
        }
        Message::NotifyTxs(Notify { arguments: txs }) => {
            for tx in txs {
                let _ret = service.notify_tx(tx).await;
            }
        }
        Message::FreshProposalsFilter(AsyncRequest {
            responder,
            arguments: proposals,
        }) => {
            let new_proposals = service.exclude_existing_proposal(proposals).await;
            if let Err(e) = responder.send(new_proposals) {
                error!("Responder sending fresh_proposals_filter failed {:?}", e);
            };
        }
        Message::GetTxStatus(Request {
            responder,
            arguments: hash,
        }) => {
            let id = ProposalShortId::from_tx_hash(&hash);
            let tx_pool = service.tx_pool.read().await;
            let ret = if let Some(PoolEntry {
                status,
                inner: entry,
                ..
            }) = tx_pool.pool_map.get_by_id(&id)
            {
                let status = if status == &Status::Proposed {
                    TxStatus::Proposed
                } else {
                    TxStatus::Pending
                };
                Ok((status, Some(entry.cycles)))
            } else if let Some(ref recent_reject_db) = service.recent_reject {
                let recent_reject_result = recent_reject_db.get(&hash);
                if let Ok(recent_reject) = recent_reject_result {
                    if let Some(record) = recent_reject {
                        Ok((TxStatus::Rejected(record), None))
                    } else {
                        Ok((TxStatus::Unknown, None))
                    }
                } else {
                    Err(recent_reject_result.unwrap_err())
                }
            } else {
                Ok((TxStatus::Unknown, None))
            };

            if let Err(e) = responder.send(ret) {
                error!("Responder sending get_tx_status failed {:?}", e)
            };
        }
        Message::GetTransactionWithStatus(Request {
            responder,
            arguments: hash,
        }) => {
            let id = ProposalShortId::from_tx_hash(&hash);
            let tx_pool = service.tx_pool.read().await;
            let ret = if let Some(PoolEntry {
                status,
                inner: entry,
                ..
            }) = tx_pool.pool_map.get_by_id(&id)
            {
                let (tx_status, min_replace_fee) = if status == &Status::Proposed {
                    (TxStatus::Proposed, None)
                } else {
                    (TxStatus::Pending, tx_pool.min_replace_fee(entry))
                };
                Ok(TransactionWithStatus::with_status(
                    Some(entry.transaction().clone()),
                    entry.cycles,
                    entry.timestamp,
                    tx_status,
                    Some(entry.fee),
                    min_replace_fee,
                ))
            } else if let Some(ref recent_reject_db) = service.recent_reject {
                match recent_reject_db.get(&hash) {
                    Ok(Some(record)) => Ok(TransactionWithStatus::with_rejected(record)),
                    Ok(_) => Ok(TransactionWithStatus::with_unknown()),
                    Err(err) => Err(err),
                }
            } else {
                Ok(TransactionWithStatus::with_unknown())
            };

            if let Err(e) = responder.send(ret) {
                error!("Responder sending get_tx_status failed {:?}", e)
            };
        }
        Message::FetchTxs(AsyncRequest {
            responder,
            arguments: short_ids,
        }) => {
            let txs_map = service.get_tx_for_compact_block(short_ids).await;
            if let Err(e) = responder.send(txs_map) {
                error!("Responder sending fetch_txs failed {:?}", e);
            };
        }
        Message::FetchTxsWithCycles(AsyncRequest {
            responder,
            arguments: short_ids,
        }) => {
            let tx_pool = service.tx_pool.read().await;
            let txs = short_ids
                .into_iter()
                .filter_map(|short_id| {
                    tx_pool
                        .get_tx_with_cycles(&short_id)
                        .map(|(tx, cycles)| (short_id, (tx, cycles)))
                })
                .collect();
            if let Err(e) = responder.send(txs) {
                error!("Responder sending fetch_txs_with_cycles failed {:?}", e);
            };
        }
        Message::NewUncle(Notify { arguments: uncle }) => {
            service.receive_candidate_uncle(uncle).await;
        }
        Message::ClearPool(Request {
            responder,
            arguments: new_snapshot,
        }) => {
            service.clear_pool(new_snapshot).await;
            if let Err(e) = responder.send(()) {
                error!("Responder sending clear_pool failed {:?}", e)
            };
        }
        Message::ClearVerifyQueue(Request { responder, .. }) => {
            service.ordered_resolve_queue.write().await.clear();
            service.verify_queue.write().await.clear();
            if let Err(e) = responder.send(()) {
                error!("Responder sending clear_verify_queue failed {:?}", e)
            };
        }
        Message::GetPoolTxDetails(Request {
            responder,
            arguments: tx_hash,
        }) => {
            let tx_pool = service.tx_pool.read().await;
            let id = ProposalShortId::from_tx_hash(&tx_hash);
            let tx_details = tx_pool
                .get_tx_detail(&id)
                .unwrap_or(PoolTxDetailInfo::with_unknown());
            if let Err(e) = responder.send(tx_details) {
                error!("responder send get_pool_tx_details failed {:?}", e)
            };
        }
        Message::GetAllEntryInfo(Request { responder, .. }) => {
            let tx_pool = service.tx_pool.read().await;
            let info = tx_pool.get_all_entry_info();
            if let Err(e) = responder.send(info) {
                error!("Responder sending get_all_entry_info failed {:?}", e)
            };
        }
        Message::GetAllIds(Request { responder, .. }) => {
            let tx_pool = service.tx_pool.read().await;
            let ids = tx_pool.get_ids();
            if let Err(e) = responder.send(ids) {
                error!("Responder sending get_ids failed {:?}", e)
            };
        }
        Message::SavePool(Request { responder, .. }) => {
            service.save_pool().await;
            if let Err(e) = responder.send(()) {
                error!("Responder sending save_pool failed {:?}", e)
            };
        }
        Message::UpdateIBDState(Request {
            responder,
            arguments: in_ibd,
        }) => {
            service.update_ibd_state(in_ibd).await;
            if let Err(e) = responder.send(()) {
                error!("Responder sending update_ibd_state failed {:?}", e)
            };
        }
        Message::EstimateFeeRate(Request {
            responder,
            arguments: (estimate_mode, enable_fallback),
        }) => {
            let fee_estimates_result = service
                .estimate_fee_rate(estimate_mode, enable_fallback)
                .await;
            if let Err(e) = responder.send(fee_estimates_result) {
                error!("Responder sending fee_estimates_result failed {:?}", e)
            };
        }
        #[cfg(feature = "internal")]
        Message::PlugEntry(Request {
            responder,
            arguments: (entries, target),
        }) => {
            service.plug_entry(entries, target).await;

            if let Err(e) = responder.send(()) {
                error!("Responder sending plug_entry failed {:?}", e);
            };
        }
        #[cfg(feature = "internal")]
        Message::PackageTxs(Request {
            responder,
            arguments: bytes_limit,
        }) => {
            let max_block_cycles = service.consensus.max_block_cycles();
            let max_block_bytes = service.consensus.max_block_bytes();
            let tx_pool = service.tx_pool.read().await;
            let (txs, _size, _cycles) = tx_pool.package_txs(
                max_block_cycles,
                bytes_limit.unwrap_or(max_block_bytes) as usize,
            );
            if let Err(e) = responder.send(txs) {
                error!("Responder sending plug_entry failed {:?}", e);
            };
        }
        Message::GetTotalRecentRejectNum(Request { responder, .. }) => {
            let total_recent_reject_num = service.get_total_recent_reject_num();
            if let Err(e) = responder.send(total_recent_reject_num) {
                error!("Responder sending total_recent_reject_num failed {:?}", e)
            };
        }
    }
}

impl TxPoolService {
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
            total_tx_size: tx_pool.pool_map.total_tx_size,
            total_tx_cycles: tx_pool.pool_map.total_tx_cycles,
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

    /// Excludes proposals that already exist in either the proposal pool or the verification queue.
    ///
    /// Any proposal that appears in **either** of these two structures is considered "already exists"
    /// and will be filtered out.
    /// - already accepted and stored in the main pool (`pool_map`), or
    /// - orphan_pool that are waiting for missing parents
    /// - currently being verified (`verify_queue`).
    ///
    /// /// # Returns
    ///
    /// A new `Vec<ProposalShortId> ` containing only the proposals that are **completely new**
    /// (not present in `pool_map` nor in `verify_queue`).
    pub async fn exclude_existing_proposal(
        &self,
        mut proposals: Vec<ProposalShortId>,
    ) -> Vec<ProposalShortId> {
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
                    .map(|entry| (short_id.to_owned(), entry.tx().to_owned()))
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

            if let Err(e) = block_assembler.update_blank(snapshot).await {
                error!("block_assembler update_blank error {}", e);
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
