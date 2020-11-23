//! Tx-pool background service

use crate::block_assembler::BlockAssembler;
use crate::component::entry::TxEntry;
use crate::error::handle_try_send_error;
use crate::pool::{TxPool, TxPoolInfo};
use crate::process::PlugTarget;
use ckb_app_config::{BlockAssemblerConfig, TxPoolConfig};
use ckb_async_runtime::Handle;
use ckb_chain_spec::consensus::Consensus;
use ckb_error::Error;
use ckb_jsonrpc_types::BlockTemplate;
use ckb_logger::error;
use ckb_notify::NotifyController;
use ckb_snapshot::{Snapshot, SnapshotMgr};
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_types::{
    core::{
        tx_pool::{TxPoolEntryInfo, TxPoolIds},
        BlockView, Cycle, TransactionView, UncleBlockView, Version,
    },
    packed::ProposalShortId,
};
use ckb_verification::cache::{CacheEntry, TxVerifyCache};
use failure::Error as FailureError;
use faketime::unix_time_as_millis;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::{atomic::AtomicU64, Arc};
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

pub(crate) type BlockTemplateResult = Result<BlockTemplate, FailureError>;
type BlockTemplateArgs = (
    Option<u64>,
    Option<u64>,
    Option<Version>,
    Option<BlockAssemblerConfig>,
);

pub(crate) type SubmitTxsResult = Result<Vec<CacheEntry>, Error>;
type NotifyTxsCallback = Option<Box<dyn FnOnce(SubmitTxsResult) + Send + Sync + 'static>>;

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
    SubmitTxs(Request<Vec<TransactionView>, SubmitTxsResult>),
    NotifyTxs(Notify<(Vec<TransactionView>, NotifyTxsCallback)>),
    ChainReorg(Notify<ChainReorgArgs>),
    FreshProposalsFilter(Request<Vec<ProposalShortId>, Vec<ProposalShortId>>),
    FetchTxs(Request<Vec<ProposalShortId>, HashMap<ProposalShortId, TransactionView>>),
    FetchTxsWithCycles(Request<Vec<ProposalShortId>, FetchTxsWithCyclesResult>),
    GetTxPoolInfo(Request<(), TxPoolInfo>),
    FetchTxRPC(Request<ProposalShortId, Option<(bool, TransactionView)>>),
    NewUncle(Notify<UncleBlockView>),
    PlugEntry(Request<(Vec<TxEntry>, PlugTarget), ()>),
    ClearPool(Request<Arc<Snapshot>, ()>),
    /// TODO(doc): @zhangsoledad
    GetAllEntryInfo(Request<(), TxPoolEntryInfo>),
    /// TODO(doc): @zhangsoledad
    GetAllIds(Request<(), TxPoolIds>),
}

/// Controller to the tx-pool service.
///
/// The Controller is internally reference-counted and can be freely cloned. A Controller can be obtained when tx-pool service construct.
#[derive(Clone)]
pub struct TxPoolController {
    sender: mpsc::Sender<Message>,
    handle: Handle,
    stop: StopHandler<()>,
}

impl Drop for TxPoolController {
    fn drop(&mut self) {
        self.stop.try_send();
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
    ) -> Result<BlockTemplateResult, FailureError> {
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
    ) -> Result<BlockTemplateResult, FailureError> {
        let mut sender = self.sender.clone();
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
        sender
            .try_send(Message::BlockTemplate(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;

        self.handle.block_on(response).map_err(Into::into)
    }

    /// Notify new uncle
    pub fn notify_new_uncle(&self, uncle: UncleBlockView) -> Result<(), FailureError> {
        let mut sender = self.sender.clone();
        let notify = Notify::notify(uncle);
        sender.try_send(Message::NewUncle(notify)).map_err(|e| {
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
    ) -> Result<(), FailureError> {
        let mut sender = self.sender.clone();
        let notify = Notify::notify((
            detached_blocks,
            attached_blocks,
            detached_proposal_id,
            snapshot,
        ));
        sender.try_send(Message::ChainReorg(notify)).map_err(|e| {
            let (_m, e) = handle_try_send_error(e);
            e.into()
        })
    }

    /// Submit local txs to tx-pool
    pub fn submit_txs(&self, txs: Vec<TransactionView>) -> Result<SubmitTxsResult, FailureError> {
        let mut sender = self.sender.clone();
        let (responder, response) = oneshot::channel();
        let request = Request::call(txs, responder);
        sender.try_send(Message::SubmitTxs(request)).map_err(|e| {
            let (_m, e) = handle_try_send_error(e);
            e
        })?;
        self.handle.block_on(response).map_err(Into::into)
    }

    /// Plug tx-pool entry to tx-pool, skip verification. only for test
    pub fn plug_entry(
        &self,
        entries: Vec<TxEntry>,
        target: PlugTarget,
    ) -> Result<(), FailureError> {
        let mut sender = self.sender.clone();
        let (responder, response) = oneshot::channel();
        let request = Request::call((entries, target), responder);
        sender.try_send(Message::PlugEntry(request)).map_err(|e| {
            let (_m, e) = handle_try_send_error(e);
            e
        })?;
        self.handle.block_on(response).map_err(Into::into)
    }

    /// Receive txs from network, try to add txs to tx-pool
    pub fn notify_txs(
        &self,
        txs: Vec<TransactionView>,
        callback: NotifyTxsCallback,
    ) -> Result<(), FailureError> {
        let mut sender = self.sender.clone();
        let notify = Notify::notify((txs, callback));
        sender.try_send(Message::NotifyTxs(notify)).map_err(|e| {
            let (_m, e) = handle_try_send_error(e);
            e.into()
        })
    }

    /// Return tx-pool information
    pub fn get_tx_pool_info(&self) -> Result<TxPoolInfo, FailureError> {
        let mut sender = self.sender.clone();
        let (responder, response) = oneshot::channel();
        let request = Request::call((), responder);
        sender
            .try_send(Message::GetTxPoolInfo(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        self.handle.block_on(response).map_err(Into::into)
    }

    /// Return fresh proposals
    pub fn fresh_proposals_filter(
        &self,
        proposals: Vec<ProposalShortId>,
    ) -> Result<Vec<ProposalShortId>, FailureError> {
        let mut sender = self.sender.clone();
        let (responder, response) = oneshot::channel();
        let request = Request::call(proposals, responder);
        sender
            .try_send(Message::FreshProposalsFilter(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        self.handle.block_on(response).map_err(Into::into)
    }

    /// Return tx for rpc
    pub fn fetch_tx_for_rpc(&self, id: ProposalShortId) -> Result<FetchTxRPCResult, FailureError> {
        let mut sender = self.sender.clone();
        let (responder, response) = oneshot::channel();
        let request = Request::call(id, responder);
        sender.try_send(Message::FetchTxRPC(request)).map_err(|e| {
            let (_m, e) = handle_try_send_error(e);
            e
        })?;
        self.handle.block_on(response).map_err(Into::into)
    }

    /// Return txs for network
    pub fn fetch_txs(
        &self,
        short_ids: Vec<ProposalShortId>,
    ) -> Result<HashMap<ProposalShortId, TransactionView>, FailureError> {
        let mut sender = self.sender.clone();
        let (responder, response) = oneshot::channel();
        let request = Request::call(short_ids, responder);
        sender.try_send(Message::FetchTxs(request)).map_err(|e| {
            let (_m, e) = handle_try_send_error(e);
            e
        })?;
        self.handle.block_on(response).map_err(Into::into)
    }

    /// Return txs with cycles
    pub fn fetch_txs_with_cycles(
        &self,
        short_ids: Vec<ProposalShortId>,
    ) -> Result<FetchTxsWithCyclesResult, FailureError> {
        let mut sender = self.sender.clone();
        let (responder, response) = oneshot::channel();
        let request = Request::call(short_ids, responder);
        sender
            .try_send(Message::FetchTxsWithCycles(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        self.handle.block_on(response).map_err(Into::into)
    }

    /// Clears the tx-pool, removing all txs, update snapshot.
    pub fn clear_pool(&self, new_snapshot: Arc<Snapshot>) -> Result<(), FailureError> {
        let mut sender = self.sender.clone();
        let (responder, response) = oneshot::channel();
        let request = Request::call(new_snapshot, responder);
        sender.try_send(Message::ClearPool(request)).map_err(|e| {
            let (_m, e) = handle_try_send_error(e);
            e
        })?;
        self.handle.block_on(response).map_err(Into::into)
    }

    /// TODO(doc): @zhangsoledad
    pub fn get_all_entry_info(&self) -> Result<TxPoolEntryInfo, FailureError> {
        let mut sender = self.sender.clone();
        let (responder, response) = oneshot::channel();
        let request = Request::call((), responder);
        sender
            .try_send(Message::GetAllEntryInfo(request))
            .map_err(|e| {
                let (_m, e) = handle_try_send_error(e);
                e
            })?;
        self.handle.block_on(response).map_err(Into::into)
    }

    /// TODO(doc): @zhangsoledad
    pub fn get_all_ids(&self) -> Result<TxPoolIds, FailureError> {
        let mut sender = self.sender.clone();
        let (responder, response) = oneshot::channel();
        let request = Request::call((), responder);
        sender.try_send(Message::GetAllIds(request)).map_err(|e| {
            let (_m, e) = handle_try_send_error(e);
            e
        })?;
        self.handle.block_on(response).map_err(Into::into)
    }
}

/// A builder used to create TxPoolService.
pub struct TxPoolServiceBuilder {
    service: Option<TxPoolService>,
}

impl TxPoolServiceBuilder {
    /// Creates a new TxPoolServiceBuilder.
    pub fn new(
        tx_pool_config: TxPoolConfig,
        snapshot: Arc<Snapshot>,
        block_assembler_config: Option<BlockAssemblerConfig>,
        txs_verify_cache: Arc<RwLock<TxVerifyCache>>,
        snapshot_mgr: Arc<SnapshotMgr>,
        notify_controller: NotifyController,
    ) -> TxPoolServiceBuilder {
        let last_txs_updated_at = Arc::new(AtomicU64::new(0));
        let consensus = snapshot.cloned_consensus();
        let tx_pool = TxPool::new(tx_pool_config, snapshot, Arc::clone(&last_txs_updated_at));
        let block_assembler = block_assembler_config.map(BlockAssembler::new);

        TxPoolServiceBuilder {
            service: Some(TxPoolService::new(
                tx_pool,
                consensus,
                block_assembler,
                txs_verify_cache,
                last_txs_updated_at,
                snapshot_mgr,
                notify_controller,
            )),
        }
    }

    /// Start a background thread tx-pool service by taking ownership of the Builder, and returns a TxPoolController.
    pub fn start(mut self, handle: &Handle) -> TxPoolController {
        let (sender, mut receiver) = mpsc::channel(DEFAULT_CHANNEL_SIZE);
        let (signal_sender, mut signal_receiver) = oneshot::channel();

        let service = self.service.take().expect("tx pool service start once");
        let handle_clone = handle.clone();

        handle.spawn(async move {
            loop {
                tokio::select! {
                    Some(message) = receiver.recv() => {
                        let service_clone = service.clone();
                        handle_clone.spawn(process(service_clone, message));
                    },
                    _ = &mut signal_receiver => break,
                    else => break,
                }
            }
        });

        let stop = StopHandler::new(SignalSender::Tokio(signal_sender), None);
        TxPoolController {
            sender,
            handle: handle.clone(),
            stop,
        }
    }
}

#[derive(Clone)]
pub(crate) struct TxPoolService {
    pub(crate) tx_pool: Arc<RwLock<TxPool>>,
    pub(crate) consensus: Arc<Consensus>,
    pub(crate) tx_pool_config: Arc<TxPoolConfig>,
    pub(crate) block_assembler: Option<BlockAssembler>,
    pub(crate) txs_verify_cache: Arc<RwLock<TxVerifyCache>>,
    pub(crate) last_txs_updated_at: Arc<AtomicU64>,
    snapshot_mgr: Arc<SnapshotMgr>,
    pub(crate) notify_controller: NotifyController,
}

impl TxPoolService {
    /// Creates a new TxPoolService.
    pub fn new(
        tx_pool: TxPool,
        consensus: Arc<Consensus>,
        block_assembler: Option<BlockAssembler>,
        txs_verify_cache: Arc<RwLock<TxVerifyCache>>,
        last_txs_updated_at: Arc<AtomicU64>,
        snapshot_mgr: Arc<SnapshotMgr>,
        notify_controller: NotifyController,
    ) -> Self {
        let tx_pool_config = Arc::new(tx_pool.config);
        Self {
            tx_pool: Arc::new(RwLock::new(tx_pool)),
            consensus,
            tx_pool_config,
            block_assembler,
            txs_verify_cache,
            last_txs_updated_at,
            snapshot_mgr,
            notify_controller,
        }
    }

    pub(crate) fn snapshot(&self) -> Arc<Snapshot> {
        Arc::clone(&self.snapshot_mgr.load())
    }
}

#[allow(clippy::cognitive_complexity)]
async fn process(service: TxPoolService, message: Message) {
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
        Message::SubmitTxs(Request {
            responder,
            arguments: txs,
        }) => {
            let submit_txs_result = service.process_txs(txs).await;
            if let Err(e) = responder.send(submit_txs_result) {
                error!("responder send submit_txs_result failed {:?}", e);
            };
        }
        Message::NotifyTxs(Notify {
            arguments: (txs, callback),
        }) => {
            let submit_txs_result = service.process_txs(txs).await;
            if let Some(call) = callback {
                call(submit_txs_result)
            };
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
                .map(|entry| (true, entry.transaction.clone()))
                .or_else(|| tx_pool.get_tx_without_conflict(&id).map(|tx| (false, tx)));
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
                        .and_then(|(tx, cycles)| cycles.map(|cycles| (short_id, (tx, cycles))))
                })
                .collect();
            if let Err(e) = responder.send(txs) {
                error!("responder send fetch_txs_with_cycles failed {:?}", e);
            };
        }
        Message::ChainReorg(Notify {
            arguments: (detached_blocks, attached_blocks, detached_proposal_id, snapshot),
        }) => {
            service
                .update_tx_pool_for_reorg(
                    detached_blocks,
                    attached_blocks,
                    detached_proposal_id,
                    snapshot,
                )
                .await
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
                        if let Err(err) = tx_pool.add_pending(entry) {
                            error!("plug entry error {}", err);
                        }
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
