//! Tx-pool background service

use crate::block_assembler::{self, BlockAssembler};
use crate::callback::{Callback, Callbacks, ProposedCallback, RejectCallback};
use crate::chunk_process::Command;
use crate::component::{chunk::ChunkQueue, orphan::OrphanPool};
use crate::error::{handle_recv_error, handle_send_cmd_error, handle_try_send_error};
use crate::pool::{TxPool, TxPoolInfo};
use crate::util::after_delay_window;
use ckb_app_config::{BlockAssemblerConfig, TxPoolConfig};
use ckb_async_runtime::Handle;
use ckb_chain_spec::consensus::Consensus;
use ckb_channel::oneshot;
use ckb_error::AnyError;
use ckb_jsonrpc_types::{BlockTemplate, TransactionWithStatus, TxStatus};
use ckb_logger::error;
use ckb_logger::info;
use ckb_network::{NetworkController, PeerIndex};
use ckb_snapshot::Snapshot;
use ckb_stop_handler::{SignalSender, StopHandler, WATCH_INIT};
use ckb_types::{
    core::{
        tx_pool::{Reject, TxPoolEntryInfo, TxPoolIds},
        BlockView, Cycle, TransactionView, UncleBlockView, Version,
    },
    packed::{Byte32, ProposalShortId},
};
use ckb_util::LinkedHashMap;
use ckb_util::LinkedHashSet;
use ckb_verification::cache::TxVerificationCache;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::sync::watch;
use tokio::sync::{mpsc, RwLock};
use tokio::task::block_in_place;

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

type FetchTxRPCResult = Option<(bool, TransactionView)>;

type GetTxStatusResult = Result<TxStatus, AnyError>;

type GetTransactionWithStatusResult = Result<TransactionWithStatus, AnyError>;

type FetchTxsWithCyclesResult = Vec<(ProposalShortId, (TransactionView, Cycle))>;

pub(crate) type ChainReorgArgs = (
    VecDeque<BlockView>,
    VecDeque<BlockView>,
    HashSet<ProposalShortId>,
    Arc<Snapshot>,
);

pub(crate) enum Message {
    BlockTemplate(Request<BlockTemplateArgs, BlockTemplateResult>),
    SubmitLocalTx(Request<TransactionView, SubmitTxResult>),
    RemoveLocalTx(Request<Byte32, bool>),
    SubmitRemoteTx(Request<(TransactionView, Cycle, PeerIndex), ()>),
    NotifyTxs(Notify<Vec<TransactionView>>),
    FreshProposalsFilter(Request<Vec<ProposalShortId>, Vec<ProposalShortId>>),
    FetchTxs(Request<Vec<ProposalShortId>, HashMap<ProposalShortId, TransactionView>>),
    FetchTxsWithCycles(Request<Vec<ProposalShortId>, FetchTxsWithCyclesResult>),
    GetTxPoolInfo(Request<(), TxPoolInfo>),
    FetchTxRPC(Request<Byte32, Option<(bool, TransactionView)>>),
    GetTxStatus(Request<Byte32, GetTxStatusResult>),
    GetTransactionWithStatus(Request<Byte32, GetTransactionWithStatusResult>),
    NewUncle(Notify<UncleBlockView>),
    ClearPool(Request<Arc<Snapshot>, ()>),
    GetAllEntryInfo(Request<(), TxPoolEntryInfo>),
    GetAllIds(Request<(), TxPoolIds>),
    SavePool(Request<(), ()>),

    // test
    #[cfg(feature = "internal")]
    PlugEntry(Request<(Vec<TxEntry>, PlugTarget), ()>),
    #[cfg(feature = "internal")]
    PackageTxs(Request<Option<u64>, Vec<TxEntry>>),
}

#[derive(Debug, Hash, Eq, PartialEq)]
pub(crate) enum BlockAssemblerMessage {
    Pending,
    Proposed,
    Uncle,
}

/// Controller to the tx-pool service.
///
/// The Controller is internally reference-counted and can be freely cloned. A Controller can be obtained when tx-pool service construct.
#[derive(Clone)]
pub struct TxPoolController {
    sender: mpsc::Sender<Message>,
    reorg_sender: mpsc::Sender<Notify<ChainReorgArgs>>,
    chunk_tx: ckb_channel::Sender<Command>,
    handle: Handle,
    stop: StopHandler<()>,
    chunk_stop: StopHandler<Command>,
    started: Arc<AtomicBool>,
}

impl Drop for TxPoolController {
    fn drop(&mut self) {
        if self.service_started() {
            self.chunk_stop.try_send(Command::Stop);
            self.stop.try_send(());
        }
    }
}

impl TxPoolController {
    /// Return whether tx-pool service is started
    pub fn service_started(&self) -> bool {
        self.started.load(Ordering::Relaxed)
    }

    /// Set tx-pool service started, should only used for test
    #[cfg(feature = "internal")]
    pub fn set_service_started(&self, v: bool) {
        self.started.store(v, Ordering::Relaxed);
    }

    /// Return reference of tokio runtime handle
    pub fn handle(&self) -> &Handle {
        &self.handle
    }

    /// Generate and return block_template
    pub fn get_block_template(
        &self,
        bytes_limit: Option<u64>,
        proposals_limit: Option<u64>,
        max_version: Option<Version>,
    ) -> Result<BlockTemplateResult, AnyError> {
        let (responder, response) = oneshot::channel();
        let request = Request::call((bytes_limit, proposals_limit, max_version), responder);
        self.sender
            .try_send(Message::BlockTemplate(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;

        block_in_place(|| response.recv())
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }

    /// Notify new uncle
    pub fn notify_new_uncle(&self, uncle: UncleBlockView) -> Result<(), AnyError> {
        let notify = Notify::new(uncle);
        self.sender
            .try_send(Message::NewUncle(notify))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e.into()
            })
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
        let (responder, response) = oneshot::channel();
        let request = Request::call(tx, responder);
        self.sender
            .try_send(Message::SubmitLocalTx(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        block_in_place(|| response.recv())
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }

    /// Remove tx from tx-pool
    pub fn remove_local_tx(&self, tx_hash: Byte32) -> Result<bool, AnyError> {
        let (responder, response) = oneshot::channel();
        let request = Request::call(tx_hash, responder);
        self.sender
            .try_send(Message::RemoveLocalTx(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        block_in_place(|| response.recv())
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }

    /// Submit remote tx with declared cycles and origin to tx-pool
    pub async fn submit_remote_tx(
        &self,
        tx: TransactionView,
        declared_cycles: Cycle,
        peer: PeerIndex,
    ) -> Result<(), AnyError> {
        let (responder, response) = oneshot::channel();
        let request = Request::call((tx, declared_cycles, peer), responder);
        self.sender
            .try_send(Message::SubmitRemoteTx(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        block_in_place(|| response.recv())
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }

    /// Receive txs from network, try to add txs to tx-pool
    pub fn notify_txs(&self, txs: Vec<TransactionView>) -> Result<(), AnyError> {
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
        let (responder, response) = oneshot::channel();
        let request = Request::call((), responder);
        self.sender
            .try_send(Message::GetTxPoolInfo(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        block_in_place(|| response.recv())
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }

    /// Return fresh proposals
    pub fn fresh_proposals_filter(
        &self,
        proposals: Vec<ProposalShortId>,
    ) -> Result<Vec<ProposalShortId>, AnyError> {
        let (responder, response) = oneshot::channel();
        let request = Request::call(proposals, responder);
        self.sender
            .try_send(Message::FreshProposalsFilter(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        block_in_place(|| response.recv())
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }

    /// Return tx for rpc
    pub fn fetch_tx_for_rpc(&self, hash: Byte32) -> Result<FetchTxRPCResult, AnyError> {
        let (responder, response) = oneshot::channel();
        let request = Request::call(hash, responder);
        self.sender
            .try_send(Message::FetchTxRPC(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        block_in_place(|| response.recv())
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }

    /// Return tx_status for rpc (get_transaction verbosity = 1)
    pub fn get_tx_status(&self, hash: Byte32) -> Result<GetTxStatusResult, AnyError> {
        let (responder, response) = oneshot::channel();
        let request = Request::call(hash, responder);
        self.sender
            .try_send(Message::GetTxStatus(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        block_in_place(|| response.recv())
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }

    /// Return transaction_with_status for rpc (get_transaction verbosity = 2)
    pub fn get_transaction_with_status(
        &self,
        hash: Byte32,
    ) -> Result<GetTransactionWithStatusResult, AnyError> {
        let (responder, response) = oneshot::channel();
        let request = Request::call(hash, responder);
        self.sender
            .try_send(Message::GetTransactionWithStatus(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        block_in_place(|| response.recv())
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }

    /// Return txs for network
    pub fn fetch_txs(
        &self,
        short_ids: Vec<ProposalShortId>,
    ) -> Result<HashMap<ProposalShortId, TransactionView>, AnyError> {
        let (responder, response) = oneshot::channel();
        let request = Request::call(short_ids, responder);
        self.sender
            .try_send(Message::FetchTxs(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        block_in_place(|| response.recv())
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }

    /// Return txs with cycles
    pub fn fetch_txs_with_cycles(
        &self,
        short_ids: Vec<ProposalShortId>,
    ) -> Result<FetchTxsWithCyclesResult, AnyError> {
        let (responder, response) = oneshot::channel();
        let request = Request::call(short_ids, responder);
        self.sender
            .try_send(Message::FetchTxsWithCycles(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        block_in_place(|| response.recv())
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }

    /// Clears the tx-pool, removing all txs, update snapshot.
    pub fn clear_pool(&self, new_snapshot: Arc<Snapshot>) -> Result<(), AnyError> {
        let (responder, response) = oneshot::channel();
        let request = Request::call(new_snapshot, responder);
        self.sender
            .try_send(Message::ClearPool(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        block_in_place(|| response.recv())
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }

    /// TODO(doc): @zhangsoledad
    pub fn get_all_entry_info(&self) -> Result<TxPoolEntryInfo, AnyError> {
        let (responder, response) = oneshot::channel();
        let request = Request::call((), responder);
        self.sender
            .try_send(Message::GetAllEntryInfo(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        block_in_place(|| response.recv())
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }

    /// TODO(doc): @zhangsoledad
    pub fn get_all_ids(&self) -> Result<TxPoolIds, AnyError> {
        let (responder, response) = oneshot::channel();
        let request = Request::call((), responder);
        self.sender
            .try_send(Message::GetAllIds(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        block_in_place(|| response.recv())
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }

    /// Sends suspend chunk process cmd
    pub fn suspend_chunk_process(&self) -> Result<(), AnyError> {
        self.chunk_tx
            .try_send(Command::Suspend)
            .map_err(handle_send_cmd_error)
            .map_err(Into::into)
    }

    /// Sends continue chunk process cmd
    pub fn continue_chunk_process(&self) -> Result<(), AnyError> {
        self.chunk_tx
            .try_send(Command::Continue)
            .map_err(handle_send_cmd_error)
            .map_err(Into::into)
    }

    /// Saves tx pool into disk.
    pub fn save_pool(&self) -> Result<(), AnyError> {
        info!("Please be patient, tx-pool are saving data into disk ...");
        let (responder, response) = oneshot::channel();
        let request = Request::call((), responder);
        self.sender
            .try_send(Message::SavePool(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        block_in_place(|| response.recv())
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }

    /// Load persisted txs into pool, assume that all txs are sorted
    fn load_persisted_data(&self, txs: Vec<TransactionView>) -> Result<(), AnyError> {
        if !txs.is_empty() {
            info!("Loading persisted tx-pool data, total {} txs", txs.len());
            let mut failed_txs = 0;
            for tx in txs {
                if self.submit_local_tx(tx)?.is_err() {
                    failed_txs += 1;
                }
            }
            if failed_txs == 0 {
                info!("Persisted tx-pool data is loaded");
            } else {
                info!(
                    "Persisted tx-pool data is loaded, {} stale txs are ignored",
                    failed_txs
                );
            }
        }
        Ok(())
    }

    /// Plug tx-pool entry to tx-pool, skip verification. only for test
    #[cfg(feature = "internal")]
    pub fn plug_entry(&self, entries: Vec<TxEntry>, target: PlugTarget) -> Result<(), AnyError> {
        let (responder, response) = oneshot::channel();
        let request = Request::call((entries, target), responder);
        self.sender
            .try_send(Message::PlugEntry(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        block_in_place(|| response.recv())
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }

    /// Package txs with specified bytes_limit. for test
    #[cfg(feature = "internal")]
    pub fn package_txs(&self, bytes_limit: Option<u64>) -> Result<Vec<TxEntry>, AnyError> {
        let (responder, response) = oneshot::channel();
        let request = Request::call(bytes_limit, responder);
        self.sender
            .try_send(Message::PackageTxs(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        block_in_place(|| response.recv())
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }
}

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
    pub(crate) signal_receiver: watch::Receiver<u8>,
    pub(crate) handle: Handle,
    pub(crate) tx_relay_sender: ckb_channel::Sender<TxVerificationResult>,
    pub(crate) chunk_rx: ckb_channel::Receiver<Command>,
    pub(crate) chunk: Arc<RwLock<ChunkQueue>>,
    pub(crate) started: Arc<AtomicBool>,
    pub(crate) block_assembler_channel: (
        mpsc::Sender<BlockAssemblerMessage>,
        mpsc::Receiver<BlockAssemblerMessage>,
    ),
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
    ) -> (TxPoolServiceBuilder, TxPoolController) {
        let (sender, receiver) = mpsc::channel(DEFAULT_CHANNEL_SIZE);
        let block_assembler_channel = mpsc::channel(BLOCK_ASSEMBLER_CHANNEL_SIZE);
        let (reorg_sender, reorg_receiver) = mpsc::channel(DEFAULT_CHANNEL_SIZE);
        let (signal_sender, signal_receiver) = watch::channel(WATCH_INIT);
        let (chunk_tx, chunk_rx) = ckb_channel::bounded(12);
        let chunk = Arc::new(RwLock::new(ChunkQueue::new()));
        let started = Arc::new(AtomicBool::new(false));

        let stop = StopHandler::new(
            SignalSender::Watch(signal_sender),
            None,
            "tx-pool".to_string(),
        );
        let chunk_stop = StopHandler::new(
            SignalSender::Crossbeam(chunk_tx.clone()),
            None,
            "chunk".to_string(),
        );
        let controller = TxPoolController {
            sender,
            reorg_sender,
            handle: handle.clone(),
            chunk_stop,
            chunk_tx,
            stop,
            started: Arc::clone(&started),
        };

        let block_assembler =
            block_assembler_config.map(|config| BlockAssembler::new(config, Arc::clone(&snapshot)));
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
            chunk,
            started,
            block_assembler_channel,
        };

        (builder, controller)
    }

    /// Register new pending callback
    pub fn register_pending(&mut self, callback: Callback) {
        self.callbacks.register_pending(callback);
    }

    /// Register new proposed callback
    pub fn register_proposed(&mut self, callback: ProposedCallback) {
        self.callbacks.register_proposed(callback);
    }

    /// Register new committed callback
    pub fn register_committed(&mut self, callback: Callback) {
        self.callbacks.register_committed(callback);
    }

    /// Register new abandon callback
    pub fn register_reject(&mut self, callback: RejectCallback) {
        self.callbacks.register_reject(callback);
    }

    /// Start a background thread tx-pool service by taking ownership of the Builder, and returns a TxPoolController.
    pub fn start(self, network: NetworkController) {
        let consensus = self.snapshot.cloned_consensus();

        let after_delay_window = after_delay_window(&self.snapshot);
        let tx_pool = TxPool::new(self.tx_pool_config, self.snapshot);

        let txs = match tx_pool.load_from_file() {
            Ok(txs) => txs,
            Err(e) => {
                error!("{}", e.to_string());
                error!("Failed to load txs from tx-pool persisted data file, all txs are ignored");
                Vec::new()
            }
        };

        let (block_assembler_sender, mut block_assembler_receiver) = self.block_assembler_channel;
        let service = TxPoolService {
            tx_pool_config: Arc::new(tx_pool.config.clone()),
            tx_pool: Arc::new(RwLock::new(tx_pool)),
            orphan: Arc::new(RwLock::new(OrphanPool::new())),
            delay: Arc::new(RwLock::new(LinkedHashMap::new())),
            after_delay: Arc::new(AtomicBool::new(after_delay_window)),
            block_assembler: self.block_assembler,
            txs_verify_cache: self.txs_verify_cache,
            callbacks: Arc::new(self.callbacks),
            tx_relay_sender: self.tx_relay_sender,
            block_assembler_sender,
            chunk: self.chunk,
            network,
            consensus,
        };

        let mut chunk_process = crate::chunk_process::TxChunkProcess::new(
            service.clone(),
            self.handle.clone(),
            self.chunk_rx,
        );

        self.handle.spawn_blocking(move || chunk_process.run());

        let mut receiver = self.receiver;
        let mut reorg_receiver = self.reorg_receiver;
        let handle_clone = self.handle.clone();

        let process_service = service.clone();
        let mut signal_receiver = self.signal_receiver.clone();
        self.handle.spawn(async move {
            loop {
                tokio::select! {
                    Some(message) = receiver.recv() => {
                        let service_clone = process_service.clone();
                        handle_clone.spawn(process(service_clone, message));
                    },
                    _ = signal_receiver.changed() => break,
                    else => break,
                }
            }
        });

        let process_service = service.clone();
        if let Some(ref block_assembler) = service.block_assembler {
            let mut signal_receiver = self.signal_receiver.clone();
            let interval = Duration::from_millis(block_assembler.config.update_interval_millis);
            self.handle.spawn(async move {
                let mut interval = tokio::time::interval(interval);
                let mut queue = LinkedHashSet::new();
                loop {
                    tokio::select! {
                        Some(message) = block_assembler_receiver.recv() => {
                            queue.insert(message);
                        },
                        _ = interval.tick() => {
                            for message in &queue {
                                let service_clone = process_service.clone();
                                block_assembler::process(service_clone, message).await;
                            }
                            if !queue.is_empty() {
                                if let Some(ref block_assembler) = process_service.block_assembler {
                                    block_assembler.notify().await;
                                }
                            }
                            queue.clear();
                        }
                        _ = signal_receiver.changed() => break,
                        else => break,
                    }
                }
            });
        }

        let mut signal_receiver = self.signal_receiver;
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
                    _ = signal_receiver.changed() => break,
                    else => break,
                }
            }
        });
        if let Err(err) = self.tx_pool_controller.load_persisted_data(txs) {
            error!("Failed to import persisted txs, cause: {}", err);
        }
        self.started.store(true, Ordering::Relaxed);
    }
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
    pub(crate) chunk: Arc<RwLock<ChunkQueue>>,
    pub(crate) delay: Arc<RwLock<LinkedHashMap<ProposalShortId, TransactionView>>>,
    pub(crate) after_delay: Arc<AtomicBool>,
    pub(crate) block_assembler_sender: mpsc::Sender<BlockAssemblerMessage>,
}

/// tx verification result
pub enum TxVerificationResult {
    /// tx is verified
    Ok {
        /// original peer
        original_peer: Option<PeerIndex>,
        /// verified by ckb vm version
        with_vm_2021: bool,
        /// transaction hash
        tx_hash: Byte32,
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
                error!("responder send get_tx_pool_info failed {:?}", e);
            };
        }
        Message::BlockTemplate(Request {
            responder,
            arguments: (_bytes_limit, _proposals_limit, _max_version),
        }) => {
            let block_template_result = service.get_block_template().await;
            if let Err(e) = responder.send(block_template_result) {
                error!("responder send block_template_result failed {:?}", e);
            };
        }
        Message::SubmitLocalTx(Request {
            responder,
            arguments: tx,
        }) => {
            let result = service.resumeble_process_tx(tx, None).await;
            if let Err(e) = responder.send(result) {
                error!("responder send submit_tx result failed {:?}", e);
            };
        }
        Message::RemoveLocalTx(Request {
            responder,
            arguments: tx_hash,
        }) => {
            let result = service.remove_tx(tx_hash).await;
            if let Err(e) = responder.send(result) {
                error!("responder send remove_tx result failed {:?}", e);
            };
        }
        Message::SubmitRemoteTx(Request {
            responder,
            arguments: (tx, declared_cycles, peer),
        }) => {
            if declared_cycles > service.tx_pool_config.max_tx_verify_cycles {
                let _result = service
                    .resumeble_process_tx(tx, Some((declared_cycles, peer)))
                    .await;
                if let Err(e) = responder.send(()) {
                    error!("responder send submit_tx result failed {:?}", e);
                };
            } else {
                let _result = service.process_tx(tx, Some((declared_cycles, peer))).await;
                if let Err(e) = responder.send(()) {
                    error!("responder send submit_tx result failed {:?}", e);
                };
            }
        }
        Message::NotifyTxs(Notify { arguments: txs }) => {
            for tx in txs {
                let _ret = service.resumeble_process_tx(tx, None).await;
            }
        }
        Message::FreshProposalsFilter(Request {
            responder,
            arguments: mut proposals,
        }) => {
            let tx_pool = service.tx_pool.read().await;
            proposals.retain(|id| !tx_pool.contains_proposal_id(id));
            if let Err(e) = responder.send(proposals) {
                error!("responder send fresh_proposals_filter failed {:?}", e);
            };
        }
        Message::FetchTxRPC(Request {
            responder,
            arguments: hash,
        }) => {
            let id = ProposalShortId::from_tx_hash(&hash);
            let tx_pool = service.tx_pool.read().await;
            let tx = tx_pool
                .proposed()
                .get(&id)
                .map(|entry| (true, entry.transaction()))
                .or_else(|| {
                    tx_pool
                        .get_tx_from_pending_or_else_gap(&id)
                        .map(|tx| (false, tx))
                })
                .map(|(proposed, tx)| (proposed, tx.clone()));
            if let Err(e) = responder.send(tx) {
                error!("responder send fetch_tx_for_rpc failed {:?}", e)
            };
        }
        Message::GetTxStatus(Request {
            responder,
            arguments: hash,
        }) => {
            let id = ProposalShortId::from_tx_hash(&hash);
            let tx_pool = service.tx_pool.read().await;

            let ret = if tx_pool.proposed.contains_key(&id) {
                Ok(TxStatus::proposed())
            } else if tx_pool.pending.contains_key(&id) || tx_pool.gap.contains_key(&id) {
                Ok(TxStatus::pending())
            } else if let Some(ref recent_reject_db) = tx_pool.recent_reject {
                let recent_reject_result = recent_reject_db.get(&hash);
                if let Ok(recent_reject) = recent_reject_result {
                    if let Some(record) = recent_reject {
                        Ok(TxStatus::rejected(record))
                    } else {
                        Ok(TxStatus::unknown())
                    }
                } else {
                    Err(recent_reject_result.unwrap_err())
                }
            } else {
                Ok(TxStatus::unknown())
            };

            if let Err(e) = responder.send(ret) {
                error!("responder send get_tx_status failed {:?}", e)
            };
        }
        Message::GetTransactionWithStatus(Request {
            responder,
            arguments: hash,
        }) => {
            let id = ProposalShortId::from_tx_hash(&hash);
            let tx_pool = service.tx_pool.read().await;

            let ret = if let Some(tx) = tx_pool.proposed.get_tx(&id) {
                Ok(TransactionWithStatus::with_proposed(Some(tx.clone())))
            } else if let Some(tx) = tx_pool.get_tx_from_pending_or_else_gap(&id) {
                Ok(TransactionWithStatus::with_pending(Some(tx.clone())))
            } else if let Some(ref recent_reject_db) = tx_pool.recent_reject {
                let recent_reject_result = recent_reject_db.get(&hash);
                if let Ok(recent_reject) = recent_reject_result {
                    if let Some(record) = recent_reject {
                        Ok(TransactionWithStatus::with_rejected(record))
                    } else {
                        Ok(TransactionWithStatus::with_unknown())
                    }
                } else {
                    Err(recent_reject_result.unwrap_err())
                }
            } else {
                Ok(TransactionWithStatus::with_unknown())
            };

            if let Err(e) = responder.send(ret) {
                error!("responder send get_tx_status failed {:?}", e)
            };
        }
        Message::FetchTxs(Request {
            responder,
            arguments: short_ids,
        }) => {
            let tx_pool = service.tx_pool.read().await;
            let txs = short_ids
                .into_iter()
                .filter_map(|short_id| {
                    tx_pool
                        .get_tx_from_pool_or_store(&short_id)
                        .map(|tx| (short_id, tx))
                })
                .collect();
            if let Err(e) = responder.send(txs) {
                error!("responder send fetch_txs failed {:?}", e);
            };
        }
        Message::FetchTxsWithCycles(Request {
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
                error!("responder send fetch_txs_with_cycles failed {:?}", e);
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
                error!("responder send clear_pool failed {:?}", e)
            };
        }
        Message::GetAllEntryInfo(Request { responder, .. }) => {
            let tx_pool = service.tx_pool.read().await;
            let info = tx_pool.get_all_entry_info();
            if let Err(e) = responder.send(info) {
                error!("responder send get_all_entry_info failed {:?}", e)
            };
        }
        Message::GetAllIds(Request { responder, .. }) => {
            let tx_pool = service.tx_pool.read().await;
            let ids = tx_pool.get_ids();
            if let Err(e) = responder.send(ids) {
                error!("responder send get_ids failed {:?}", e)
            };
        }
        Message::SavePool(Request { responder, .. }) => {
            service.save_pool().await;
            if let Err(e) = responder.send(()) {
                error!("responder send save_pool failed {:?}", e)
            };
        }
        #[cfg(feature = "internal")]
        Message::PlugEntry(Request {
            responder,
            arguments: (entries, target),
        }) => {
            service.plug_entry(entries, target).await;

            if let Err(e) = responder.send(()) {
                error!("responder send plug_entry failed {:?}", e);
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
                error!("responder send plug_entry failed {:?}", e);
            };
        }
    }
}

impl TxPoolService {
    /// Tx-pool information
    async fn info(&self) -> TxPoolInfo {
        let tx_pool = self.tx_pool.read().await;
        let orphan = self.orphan.read().await;
        let tip_header = tx_pool.snapshot.tip_header();
        TxPoolInfo {
            tip_hash: tip_header.hash(),
            tip_number: tip_header.number(),
            pending_size: tx_pool.pending.size() + tx_pool.gap.size(),
            proposed_size: tx_pool.proposed.size(),
            orphan_size: orphan.len(),
            total_tx_size: tx_pool.total_tx_size,
            total_tx_cycles: tx_pool.total_tx_cycles,
            last_txs_updated_at: 0,
        }
    }

    pub fn after_delay(&self) -> bool {
        self.after_delay.load(Ordering::Relaxed)
    }

    pub fn set_after_delay_true(&self) {
        self.after_delay.store(true, Ordering::Relaxed);
    }

    pub fn should_notify_block_assembler(&self) -> bool {
        self.block_assembler.is_some()
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
                        tx_pool.add_pending(entry);
                    }
                }
                PlugTarget::Proposed => {
                    for entry in entries {
                        if let Err(err) = tx_pool.add_proposed(entry) {
                            error!("plug entry error {}", err);
                        }
                    }
                }
            };
        }

        if self.should_notify_block_assembler() {
            match target {
                PlugTarget::Pending => {
                    if self
                        .block_assembler_sender
                        .send(BlockAssemblerMessage::Pending)
                        .await
                        .is_err()
                    {
                        error!("block_assembler receiver dropped");
                    }
                }
                PlugTarget::Proposed => {
                    if self
                        .block_assembler_sender
                        .send(BlockAssemblerMessage::Proposed)
                        .await
                        .is_err()
                    {
                        error!("block_assembler receiver dropped");
                    }
                }
            };
        }
    }
}
