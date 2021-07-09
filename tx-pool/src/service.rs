//! Tx-pool background service

use crate::block_assembler::BlockAssembler;
use crate::callback::{Callback, Callbacks, ProposedCallback, RejectCallback};
use crate::chunk_process::Command;
use crate::component::{chunk::ChunkQueue, entry::TxEntry, orphan::OrphanPool};
use crate::error::{handle_recv_error, handle_send_cmd_error, handle_try_send_error};
use crate::pool::{TxPool, TxPoolInfo};
use crate::process::PlugTarget;
use ckb_app_config::{BlockAssemblerConfig, TxPoolConfig};
use ckb_async_runtime::Handle;
use ckb_chain_spec::consensus::Consensus;
use ckb_error::{AnyError, Error};
use ckb_jsonrpc_types::BlockTemplate;
use ckb_logger::error;
use ckb_network::{NetworkController, PeerIndex};
use ckb_snapshot::{Snapshot, SnapshotMgr};
use ckb_stop_handler::{SignalSender, StopHandler, WATCH_INIT};
use ckb_types::{
    core::{
        tx_pool::{TxPoolEntryInfo, TxPoolIds},
        BlockView, Cycle, TransactionView, UncleBlockView, Version,
    },
    packed::{Byte32, ProposalShortId},
};
use ckb_verification::cache::{Completed, TxVerificationCache};
use faketime::unix_time_as_millis;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::{atomic::AtomicU64, Arc};
use tokio::sync::watch;
use tokio::sync::{mpsc, oneshot, RwLock};

pub(crate) const DEFAULT_CHANNEL_SIZE: usize = 512;

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
    pub(crate) fn notify(arguments: A) -> Notify<A> {
        Notify { arguments }
    }
}

pub(crate) type BlockTemplateResult = Result<BlockTemplate, AnyError>;
type BlockTemplateArgs = (
    Option<u64>,
    Option<u64>,
    Option<Version>,
    Option<BlockAssemblerConfig>,
);

pub(crate) type SubmitTxResult = Result<Completed, Error>;

type FetchTxRPCResult = Option<(bool, TransactionView)>;

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
    SubmitRemoteTx(Request<(TransactionView, Cycle, PeerIndex), ()>),
    NotifyTxs(Notify<Vec<TransactionView>>),
    FreshProposalsFilter(Request<Vec<ProposalShortId>, Vec<ProposalShortId>>),
    FetchTxs(Request<Vec<ProposalShortId>, HashMap<ProposalShortId, TransactionView>>),
    FetchTxsWithCycles(Request<Vec<ProposalShortId>, FetchTxsWithCyclesResult>),
    GetTxPoolInfo(Request<(), TxPoolInfo>),
    FetchTxRPC(Request<ProposalShortId, Option<(bool, TransactionView)>>),
    NewUncle(Notify<UncleBlockView>),
    PlugEntry(Request<(Vec<TxEntry>, PlugTarget), ()>),
    ClearPool(Request<Arc<Snapshot>, ()>),
    GetAllEntryInfo(Request<(), TxPoolEntryInfo>),
    GetAllIds(Request<(), TxPoolIds>),
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
}

impl Drop for TxPoolController {
    fn drop(&mut self) {
        self.chunk_stop.try_send(Command::Stop);
        self.stop.try_send(());
    }
}

impl TxPoolController {
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
        self.get_block_template_with_block_assembler_config(
            bytes_limit,
            proposals_limit,
            max_version,
            None,
        )
    }

    /// Generate and return block_template with block_assembler_config
    pub fn get_block_template_with_block_assembler_config(
        &self,
        bytes_limit: Option<u64>,
        proposals_limit: Option<u64>,
        max_version: Option<Version>,
        block_assembler_config: Option<BlockAssemblerConfig>,
    ) -> Result<BlockTemplateResult, AnyError> {
        let (responder, response) = oneshot::channel();
        let request = Request::call(
            (
                bytes_limit,
                proposals_limit,
                max_version,
                block_assembler_config,
            ),
            responder,
        );
        self.sender
            .try_send(Message::BlockTemplate(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;

        self.handle
            .block_on(response)
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }

    /// Notify new uncle
    pub fn notify_new_uncle(&self, uncle: UncleBlockView) -> Result<(), AnyError> {
        let notify = Notify::notify(uncle);
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
        let notify = Notify::notify((
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
        self.handle
            .block_on(response)
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
        response
            .await
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }

    /// Plug tx-pool entry to tx-pool, skip verification. only for test
    pub fn plug_entry(&self, entries: Vec<TxEntry>, target: PlugTarget) -> Result<(), AnyError> {
        let (responder, response) = oneshot::channel();
        let request = Request::call((entries, target), responder);
        self.sender
            .try_send(Message::PlugEntry(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        self.handle
            .block_on(response)
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }

    /// Receive txs from network, try to add txs to tx-pool
    pub fn notify_txs(&self, txs: Vec<TransactionView>) -> Result<(), AnyError> {
        let notify = Notify::notify(txs);
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
        self.handle
            .block_on(response)
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
        self.handle
            .block_on(response)
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }

    /// Return tx for rpc
    pub fn fetch_tx_for_rpc(&self, id: ProposalShortId) -> Result<FetchTxRPCResult, AnyError> {
        let (responder, response) = oneshot::channel();
        let request = Request::call(id, responder);
        self.sender
            .try_send(Message::FetchTxRPC(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        self.handle
            .block_on(response)
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
        self.handle
            .block_on(response)
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
        self.handle
            .block_on(response)
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
        self.handle
            .block_on(response)
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
        self.handle
            .block_on(response)
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
        self.handle
            .block_on(response)
            .map_err(handle_recv_error)
            .map_err(Into::into)
    }

    /// send suspend chunk process cmd
    pub fn suspend_chunk_process(&self) -> Result<(), AnyError> {
        self.chunk_tx
            .try_send(Command::Suspend)
            .map_err(handle_send_cmd_error)
            .map_err(Into::into)
    }

    /// send continue chunk process cmd
    pub fn continue_chunk_process(&self) -> Result<(), AnyError> {
        self.chunk_tx
            .try_send(Command::Continue)
            .map_err(handle_send_cmd_error)
            .map_err(Into::into)
    }
}

/// A builder used to create TxPoolService.
pub struct TxPoolServiceBuilder {
    pub(crate) tx_pool_config: TxPoolConfig,
    pub(crate) snapshot: Arc<Snapshot>,
    pub(crate) block_assembler: Option<BlockAssembler>,
    pub(crate) txs_verify_cache: Arc<RwLock<TxVerificationCache>>,
    pub(crate) snapshot_mgr: Arc<SnapshotMgr>,
    pub(crate) callbacks: Callbacks,
    pub(crate) receiver: mpsc::Receiver<Message>,
    pub(crate) reorg_receiver: mpsc::Receiver<Notify<ChainReorgArgs>>,
    pub(crate) signal_receiver: watch::Receiver<u8>,
    pub(crate) handle: Handle,
    pub(crate) tx_relay_sender: ckb_channel::Sender<(Option<PeerIndex>, bool, Byte32)>,
    pub(crate) chunk_rx: ckb_channel::Receiver<Command>,
    pub(crate) chunk: Arc<RwLock<ChunkQueue>>,
}

impl TxPoolServiceBuilder {
    /// Creates a new TxPoolServiceBuilder.
    pub fn new(
        tx_pool_config: TxPoolConfig,
        snapshot: Arc<Snapshot>,
        block_assembler_config: Option<BlockAssemblerConfig>,
        txs_verify_cache: Arc<RwLock<TxVerificationCache>>,
        snapshot_mgr: Arc<SnapshotMgr>,
        handle: &Handle,
        tx_relay_sender: ckb_channel::Sender<(Option<PeerIndex>, bool, Byte32)>,
    ) -> (TxPoolServiceBuilder, TxPoolController) {
        let (sender, receiver) = mpsc::channel(DEFAULT_CHANNEL_SIZE);
        let (reorg_sender, reorg_receiver) = mpsc::channel(DEFAULT_CHANNEL_SIZE);
        let (signal_sender, signal_receiver) = watch::channel(WATCH_INIT);
        let (chunk_tx, chunk_rx) = ckb_channel::bounded(12);
        let chunk = Arc::new(RwLock::new(ChunkQueue::new()));

        let builder = TxPoolServiceBuilder {
            tx_pool_config,
            snapshot,
            block_assembler: block_assembler_config.map(BlockAssembler::new),
            txs_verify_cache,
            snapshot_mgr,
            callbacks: Callbacks::new(),
            receiver,
            reorg_receiver,
            signal_receiver,
            handle: handle.clone(),
            tx_relay_sender,
            chunk_rx,
            chunk,
        };

        let stop = StopHandler::new(SignalSender::Watch(signal_sender), None);
        let chunk_stop = StopHandler::new(SignalSender::Crossbeam(chunk_tx.clone()), None);
        let controller = TxPoolController {
            sender,
            reorg_sender,
            handle: handle.clone(),
            chunk_stop,
            chunk_tx,
            stop,
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
        let last_txs_updated_at = Arc::new(AtomicU64::new(0));
        let consensus = self.snapshot.cloned_consensus();
        let tx_pool = TxPool::new(
            self.tx_pool_config,
            self.snapshot,
            Arc::clone(&last_txs_updated_at),
        );

        let service = TxPoolService {
            tx_pool_config: Arc::new(tx_pool.config),
            tx_pool: Arc::new(RwLock::new(tx_pool)),
            orphan: Arc::new(RwLock::new(OrphanPool::new())),
            block_assembler: self.block_assembler,
            txs_verify_cache: self.txs_verify_cache,
            snapshot_mgr: self.snapshot_mgr,
            callbacks: Arc::new(self.callbacks),
            tx_relay_sender: self.tx_relay_sender,
            chunk: self.chunk,
            network,
            consensus,
            last_txs_updated_at,
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

        let mut signal_receiver = self.signal_receiver;
        self.handle.spawn(async move {
            loop {
                tokio::select! {
                    Some(message) = reorg_receiver.recv() => {
                        let Notify {
                            arguments: (detached_blocks, attached_blocks, detached_proposal_id, snapshot),
                        } = message;
                        let service_clone = service.clone();
                        service_clone
                        .update_tx_pool_for_reorg(
                            detached_blocks,
                            attached_blocks,
                            detached_proposal_id,
                            snapshot,
                        )
                        .await
                    },
                    _ = signal_receiver.changed() => break,
                    else => break,
                }
            }
        });
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
    pub(crate) last_txs_updated_at: Arc<AtomicU64>,
    pub(crate) callbacks: Arc<Callbacks>,
    pub(crate) snapshot_mgr: Arc<SnapshotMgr>,
    pub(crate) network: NetworkController,
    pub(crate) tx_relay_sender: ckb_channel::Sender<(Option<PeerIndex>, bool, Byte32)>,
    pub(crate) chunk: Arc<RwLock<ChunkQueue>>,
}

impl TxPoolService {
    pub(crate) fn snapshot(&self) -> Arc<Snapshot> {
        Arc::clone(&self.snapshot_mgr.load())
    }
}

#[allow(clippy::cognitive_complexity)]
async fn process(mut service: TxPoolService, message: Message) {
    match message {
        Message::GetTxPoolInfo(Request { responder, .. }) => {
            let info = service.tx_pool.read().await.info();
            if let Err(e) = responder.send(info) {
                error!("responder send get_tx_pool_info failed {:?}", e);
            };
        }
        Message::BlockTemplate(Request {
            responder,
            arguments: (bytes_limit, proposals_limit, max_version, block_assembler_config),
        }) => {
            let block_template_result = service
                .get_block_template(
                    bytes_limit,
                    proposals_limit,
                    max_version,
                    block_assembler_config,
                )
                .await;
            if let Err(e) = responder.send(block_template_result) {
                error!("responder send block_template_result failed {:?}", e);
            };
        }
        Message::SubmitLocalTx(Request {
            responder,
            arguments: tx,
        }) => {
            let result = service.process_tx(tx, None).await;
            if let Err(e) = responder.send(result.map_err(Into::into)) {
                error!("responder send submit_tx result failed {:?}", e);
            };
        }
        Message::SubmitRemoteTx(Request {
            responder,
            arguments: (tx, declared_cycles, peer),
        }) => {
            if declared_cycles > service.tx_pool_config.max_tx_verify_cycles {
                let _result = service
                    .enqueue_remote_chunk_tx(tx, (declared_cycles, peer))
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
                let _ret = service.resumeble_process_tx(tx).await;
            }
        }
        Message::FreshProposalsFilter(Request {
            responder,
            arguments: mut proposals,
        }) => {
            let tx_pool = service.tx_pool.read().await;
            proposals.retain(|id| !tx_pool.contains_proposal_id(&id));
            if let Err(e) = responder.send(proposals) {
                error!("responder send fresh_proposals_filter failed {:?}", e);
            };
        }
        Message::FetchTxRPC(Request {
            responder,
            arguments: id,
        }) => {
            let tx_pool = service.tx_pool.read().await;
            let tx = tx_pool
                .proposed()
                .get(&id)
                .map(|entry| (true, entry.transaction()))
                .or_else(|| tx_pool.get_tx_without_conflict(&id).map(|tx| (false, tx)))
                .map(|(proposed, tx)| (proposed, tx.clone()));
            if let Err(e) = responder.send(tx) {
                error!("responder send fetch_tx_for_rpc failed {:?}", e)
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
                    if let Some(tx) = tx_pool.get_tx_from_pool_or_store(&short_id) {
                        Some((short_id, tx))
                    } else {
                        None
                    }
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
            if service.block_assembler.is_some() {
                let block_assembler = service.block_assembler.clone().unwrap();
                block_assembler.candidate_uncles.lock().await.insert(uncle);
                block_assembler
                    .last_uncles_updated_at
                    .store(unix_time_as_millis(), Ordering::SeqCst);
            }
        }
        Message::PlugEntry(Request {
            responder,
            arguments: (entries, target),
        }) => {
            let mut tx_pool = service.tx_pool.write().await;
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
            if let Err(e) = responder.send(()) {
                error!("responder send plug_entry failed {:?}", e);
            };
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
    }
}
