use crate::block_assembler::BlockAssembler;
use crate::config::BlockAssemblerConfig;
use crate::config::TxPoolConfig;
use crate::error::{BlockAssemblerError, PoolError};
use crate::pool::{TxPool, TxPoolInfo};
use crate::process::{
    BlockTemplateProcess, ChainReorgProcess, FetchCache, FetchTxRPCProcess, FetchTxsProcess,
    FetchTxsWithCyclesProcess, FreshProposalsFilterProcess, NewUncleProcess, SubmitTxsProcess,
    TxPoolInfoProcess, UpdateCache,
};
use ckb_jsonrpc_types::BlockTemplate;
use ckb_logger::error;
use ckb_script::ScriptConfig;
use ckb_snapshot::Snapshot;
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_types::{
    core::{
        service::DEFAULT_CHANNEL_SIZE, BlockView, Cycle, TransactionView, UncleBlockView, Version,
    },
    packed::{Byte32, ProposalShortId},
};
use failure::Error as FailureError;
use futures::future::{self, Future};
use futures::stream::Stream;
use futures::sync::{mpsc, oneshot};
use lru_cache::LruCache;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::thread;
use tokio::sync::lock::Lock;

pub struct Request<A, R> {
    pub responder: oneshot::Sender<R>,
    pub arguments: A,
}

impl<A, R> Request<A, R> {
    pub fn call(arguments: A, responder: oneshot::Sender<R>) -> Request<A, R> {
        Request {
            responder,
            arguments,
        }
    }
}

pub struct Notify<A> {
    pub arguments: A,
}

impl<A> Notify<A> {
    pub fn notify(arguments: A) -> Notify<A> {
        Notify { arguments }
    }
}

pub type BlockTemplateResult = Result<BlockTemplate, FailureError>;
type BlockTemplateArgs = (Option<u64>, Option<u64>, Option<Version>);

type SubmitTxsArgs = Vec<TransactionView>;
pub type SubmitTxsResult = Result<Vec<Cycle>, PoolError>;

type ChainReorgArgs = (
    VecDeque<BlockView>,
    VecDeque<BlockView>,
    HashSet<ProposalShortId>,
    Arc<Snapshot>,
);

pub enum Message {
    BlockTemplate(Request<BlockTemplateArgs, BlockTemplateResult>),
    SubmitTxs(Request<SubmitTxsArgs, SubmitTxsResult>),
    ChainReorg(Notify<ChainReorgArgs>),
    FreshProposalsFilter(Request<Vec<ProposalShortId>, Vec<ProposalShortId>>),
    FetchTxs(Request<HashSet<ProposalShortId>, HashMap<ProposalShortId, TransactionView>>),
    FetchTxsWithCycles(
        Request<HashSet<ProposalShortId>, HashMap<ProposalShortId, (TransactionView, Cycle)>>,
    ),
    GetTxPoolInfo(Request<(), TxPoolInfo>),
    FetchTxRPC(Request<ProposalShortId, Option<(bool, TransactionView)>>),
    NewUncle(Notify<UncleBlockView>),
}

#[derive(Clone)]
pub struct TxPoolController {
    sender: mpsc::Sender<Message>,
    stop: StopHandler<()>,
}

impl TxPoolController {
    pub fn get_block_template(
        &self,
        bytes_limit: Option<u64>,
        proposals_limit: Option<u64>,
        max_version: Option<Version>,
    ) -> Result<Box<dyn Future<Item = BlockTemplateResult, Error = oneshot::Canceled>>, FailureError>
    {
        let mut sender = self.sender.clone();
        let (responder, response) = oneshot::channel();
        let request = Request::call((bytes_limit, proposals_limit, max_version), responder);
        sender.try_send(Message::BlockTemplate(request))?;
        Ok(Box::new(response))
    }

    pub fn notify_new_uncle(&self, uncle: UncleBlockView) -> Result<(), FailureError> {
        let mut sender = self.sender.clone();
        let notify = Notify::notify(uncle);
        sender
            .try_send(Message::NewUncle(notify))
            .map_err(Into::into)
    }

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
        sender
            .try_send(Message::ChainReorg(notify))
            .map_err(Into::into)
    }

    pub fn submit_txs(
        &self,
        txs: Vec<TransactionView>,
    ) -> Result<Box<dyn Future<Item = SubmitTxsResult, Error = oneshot::Canceled>>, FailureError>
    {
        let mut sender = self.sender.clone();
        let (responder, response) = oneshot::channel();
        let request = Request::call(txs, responder);
        sender.try_send(Message::SubmitTxs(request))?;
        Ok(Box::new(response))
    }

    pub fn get_tx_pool_info(
        &self,
    ) -> Result<Box<dyn Future<Item = TxPoolInfo, Error = oneshot::Canceled>>, FailureError> {
        let mut sender = self.sender.clone();
        let (responder, response) = oneshot::channel();
        let request = Request::call((), responder);
        sender.try_send(Message::GetTxPoolInfo(request))?;
        Ok(Box::new(response))
    }

    pub fn fresh_proposals_filter(
        &self,
        proposals: Vec<ProposalShortId>,
    ) -> Result<Box<dyn Future<Item = Vec<ProposalShortId>, Error = oneshot::Canceled>>, FailureError>
    {
        let mut sender = self.sender.clone();
        let (responder, response) = oneshot::channel();
        let request = Request::call(proposals, responder);
        sender.try_send(Message::FreshProposalsFilter(request))?;
        Ok(Box::new(response))
    }

    pub fn fetch_tx_for_rpc(
        &self,
        id: ProposalShortId,
    ) -> Result<
        Box<dyn Future<Item = Option<(bool, TransactionView)>, Error = oneshot::Canceled>>,
        FailureError,
    > {
        let mut sender = self.sender.clone();
        let (responder, response) = oneshot::channel();
        let request = Request::call(id, responder);
        sender.try_send(Message::FetchTxRPC(request))?;
        Ok(Box::new(response))
    }

    pub fn fetch_txs(
        &self,
        short_ids: HashSet<ProposalShortId>,
    ) -> Result<
        Box<
            dyn Future<Item = HashMap<ProposalShortId, TransactionView>, Error = oneshot::Canceled>,
        >,
        FailureError,
    > {
        let mut sender = self.sender.clone();
        let (responder, response) = oneshot::channel();
        let request = Request::call(short_ids, responder);
        sender.try_send(Message::FetchTxs(request))?;
        Ok(Box::new(response))
    }

    pub fn fetch_txs_with_cycles(
        &self,
        short_ids: HashSet<ProposalShortId>,
    ) -> Result<
        Box<
            dyn Future<
                Item = HashMap<ProposalShortId, (TransactionView, Cycle)>,
                Error = oneshot::Canceled,
            >,
        >,
        FailureError,
    > {
        let mut sender = self.sender.clone();
        let (responder, response) = oneshot::channel();
        let request = Request::call(short_ids, responder);
        sender.try_send(Message::FetchTxsWithCycles(request))?;
        Ok(Box::new(response))
    }
}

impl Drop for TxPoolController {
    fn drop(&mut self) {
        self.stop.try_send();
    }
}

pub struct TxPoolServiceBuiler {
    service: Option<TxPoolService>,
}

impl TxPoolServiceBuiler {
    pub fn new(
        tx_pool_config: TxPoolConfig,
        snapshot: Arc<Snapshot>,
        script_config: ScriptConfig,
        block_assembler_config: Option<BlockAssemblerConfig>,
        txs_verify_cache: Lock<LruCache<Byte32, Cycle>>,
    ) -> TxPoolServiceBuiler {
        let tx_pool = TxPool::new(tx_pool_config, snapshot, script_config);
        let block_assembler = block_assembler_config.map(BlockAssembler::new);

        TxPoolServiceBuiler {
            service: Some(TxPoolService::new(
                tx_pool,
                block_assembler,
                txs_verify_cache,
            )),
        }
    }

    pub fn start(mut self) -> TxPoolController {
        let (signal_sender, signal_receiver) = oneshot::channel();
        let (sender, receiver) = mpsc::channel(DEFAULT_CHANNEL_SIZE);

        let thread_builder = thread::Builder::new().name("TX-POOL".to_string());
        let service = self.service.take().expect("tx pool service start once");

        let thread = thread_builder
            .spawn(move || {
                let server = receiver
                    .for_each(move |message| {
                        let service_clone = service.clone();
                        tokio::spawn(service_clone.process(message))
                    })
                    .select2(signal_receiver)
                    .map(|_| ())
                    .map_err(|_| ());
                tokio::run(server);
            })
            .expect("Start TX-POOL failed");;


        let stop = StopHandler::new(SignalSender::Future(signal_sender), thread);

        TxPoolController { sender, stop }
    }
}

#[derive(Clone)]
pub struct TxPoolService {
    tx_pool: Lock<TxPool>,
    block_assembler: Option<Lock<BlockAssembler>>,
    txs_verify_cache: Lock<LruCache<Byte32, Cycle>>,
}

impl TxPoolService {
    pub fn new(
        tx_pool: TxPool,
        block_assembler: Option<BlockAssembler>,
        txs_verify_cache: Lock<LruCache<Byte32, Cycle>>,
    ) -> Self {
        Self {
            tx_pool: Lock::new(tx_pool),
            block_assembler: block_assembler.map(Lock::new),
            txs_verify_cache,
        }
    }

    fn process(&self, message: Message) -> Box<dyn Future<Item = (), Error = ()> + 'static + Send> {
        match message {
            Message::GetTxPoolInfo(Request {
                responder,
                arguments: _,
            }) => Box::new(self.get_tx_pool_info().and_then(|info| {
                responder
                    .send(info)
                    .map_err(|_| error!("responder send tx_pool_info failed"));
                future::ok(())
            })),

            Message::BlockTemplate(Request {
                responder,
                arguments: (bytes_limit, proposals_limit, max_version),
            }) => Box::new(
                self.get_block_template(bytes_limit, proposals_limit, max_version)
                    .and_then(|block_template_result| {
                        responder
                            .send(block_template_result)
                            .map_err(|_| error!("responder send block_template_result failed"));
                        future::ok(())
                    }),
            ),
            Message::SubmitTxs(Request {
                responder,
                arguments: txs,
            }) => Box::new(self.submit_txs(txs).and_then(|submit_txs_result| {
                responder
                    .send(submit_txs_result)
                    .map_err(|_| error!("responder send submit_txs_result failed"));
                future::ok(())
            })),
            Message::FreshProposalsFilter(Request {
                responder,
                arguments: proposals,
            }) => Box::new(self.fresh_proposals_filter(proposals).and_then(
                |fresh_proposals_filter| {
                    responder
                        .send(fresh_proposals_filter)
                        .map_err(|_| error!("responder send fresh_proposals_filter failed"));
                    future::ok(())
                },
            )),
            Message::FetchTxRPC(Request {
                responder,
                arguments: id,
            }) => Box::new(self.fetch_tx_for_rpc(id).and_then(|tx| {
                responder
                    .send(tx)
                    .map_err(|_| error!("responder send fresh_proposals_filter failed"));
                future::ok(())
            })),
            Message::FetchTxs(Request {
                responder,
                arguments: short_ids,
            }) => Box::new(self.fetch_txs(short_ids).and_then(|txs| {
                responder
                    .send(txs)
                    .map_err(|_| error!("responder send txs failed"));
                future::ok(())
            })),
            Message::FetchTxsWithCycles(Request {
                responder,
                arguments: short_ids,
            }) => Box::new(self.fetch_txs_with_cycles(short_ids).and_then(|txs| {
                responder
                    .send(txs)
                    .map_err(|_| error!("responder send txs failed"));
                future::ok(())
            })),
            Message::ChainReorg(Notify {
                arguments: (detached_blocks, attached_blocks, detached_proposal_id, snapshot),
            }) => Box::new(self.update_tx_pool_for_reorg(
                detached_blocks,
                attached_blocks,
                detached_proposal_id,
                snapshot,
            )),
            Message::NewUncle(Notify { arguments: uncle }) => Box::new(self.new_uncle(uncle)),
        }
    }

    fn get_tx_pool_info(&self) -> impl Future<Item = TxPoolInfo, Error = ()> {
        TxPoolInfoProcess {
            tx_pool: self.tx_pool.clone(),
        }
    }

    fn get_block_template(
        &self,
        bytes_limit: Option<u64>,
        proposals_limit: Option<u64>,
        max_version: Option<Version>,
    ) -> impl Future<Item = BlockTemplateResult, Error = ()> {
        if self.block_assembler.is_none() {
            future::Either::A(future::ok(Err(BlockAssemblerError::Disabled.into())))
        } else {
            future::Either::B(BlockTemplateProcess::new(
                self.tx_pool.clone(),
                self.block_assembler.clone().unwrap(),
                bytes_limit,
                proposals_limit,
                max_version,
            ))
        }
    }

    fn submit_txs(
        &self,
        txs: Vec<TransactionView>,
    ) -> impl Future<Item = SubmitTxsResult, Error = ()> {
        let keys: Vec<Byte32> = txs.iter().map(|tx| tx.hash()).collect();
        let fetched_cache = FetchCache::new(self.txs_verify_cache.clone(), keys);
        let txs_verify_cache = self.txs_verify_cache.clone();

        let tx_pool = self.tx_pool.clone();
        fetched_cache
            .and_then(move |cache| SubmitTxsProcess::new(tx_pool, cache, txs))
            .map(move |ret| {
                ret.map(|(map, cycles)| {
                    tokio::spawn(UpdateCache::new(txs_verify_cache, map));
                    cycles
                })
            })
    }

    fn fresh_proposals_filter(
        &self,
        proposals: Vec<ProposalShortId>,
    ) -> impl Future<Item = Vec<ProposalShortId>, Error = ()> {
        FreshProposalsFilterProcess::new(self.tx_pool.clone(), proposals)
    }

    fn fetch_tx_for_rpc(
        &self,
        id: ProposalShortId,
    ) -> impl Future<Item = Option<(bool, TransactionView)>, Error = ()> {
        FetchTxRPCProcess::new(self.tx_pool.clone(), id)
    }

    fn fetch_txs(
        &self,
        short_ids: HashSet<ProposalShortId>,
    ) -> impl Future<Item = HashMap<ProposalShortId, TransactionView>, Error = ()> {
        FetchTxsProcess::new(self.tx_pool.clone(), short_ids)
    }

    fn fetch_txs_with_cycles(
        &self,
        short_ids: HashSet<ProposalShortId>,
    ) -> impl Future<Item = HashMap<ProposalShortId, (TransactionView, Cycle)>, Error = ()> {
        FetchTxsWithCyclesProcess::new(self.tx_pool.clone(), short_ids)
    }

    pub fn update_tx_pool_for_reorg(
        &self,
        detached_blocks: VecDeque<BlockView>,
        attached_blocks: VecDeque<BlockView>,
        detached_proposal_id: HashSet<ProposalShortId>,
        snapshot: Arc<Snapshot>,
    ) -> impl Future<Item = (), Error = ()> {
        let mut detached = HashSet::new();
        let mut attached = HashSet::new();
        for blk in &detached_blocks {
            detached.extend(blk.transactions().iter().skip(1).map(|tx| tx.hash()))
        }
        for blk in &attached_blocks {
            attached.extend(blk.transactions().iter().skip(1).map(|tx| tx.hash()))
        }
        let retain: Vec<Byte32> = detached.difference(&attached).cloned().collect();

        let fetched_cache = FetchCache::new(self.txs_verify_cache.clone(), retain);
        let txs_verify_cache = self.txs_verify_cache.clone();
        let tx_pool = self.tx_pool.clone();
        fetched_cache
            .and_then(move |cache| {
                ChainReorgProcess::new(
                    tx_pool,
                    cache,
                    detached_blocks,
                    attached_blocks,
                    detached_proposal_id,
                    snapshot,
                )
            })
            .map(move |map| {
                tokio::spawn(UpdateCache::new(txs_verify_cache, map));
            })
    }

    pub fn new_uncle(&self, uncle: UncleBlockView) -> impl Future<Item = (), Error = ()> {
        if self.block_assembler.is_none() {
            future::Either::A(future::ok(()))
        } else {
            future::Either::B(NewUncleProcess::new(
                self.block_assembler.clone().unwrap(),
                uncle,
            ))
        }
    }
}
