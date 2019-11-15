use crate::block_assembler::BlockAssembler;
use crate::component::entry::TxEntry;
use crate::config::BlockAssemblerConfig;
use crate::config::TxPoolConfig;
use crate::pool::{TxPool, TxPoolInfo};
use crate::process::{
    BlockTemplateBuilder, BlockTemplateCacheProcess, BuildCellbaseProcess, ChainReorgProcess,
    EstimateFeeRateProcess, EstimatorProcessBlockProcess, EstimatorTrackTxProcess, FetchCache,
    FetchTxRPCProcess, FetchTxsProcess, FetchTxsWithCyclesProcess, FreshProposalsFilterProcess,
    NewUncleProcess, PackageTxsProcess, PlugEntryProcess, PlugTarget, PreResolveTxsProcess,
    PrepareUnclesProcess, SubmitTxsProcess, TxPoolInfoProcess, UpdateBlockTemplateCache,
    UpdateCache, VerifyTxsProcess,
};
use crate::FeeRate;
use ckb_error::{Error, InternalErrorKind};
use ckb_future_executor::{new_executor, Executor};
use ckb_jsonrpc_types::BlockTemplate;
use ckb_logger::error;
use ckb_snapshot::{Snapshot, SnapshotMgr};
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_types::{
    core::{BlockView, Cycle, TransactionView, UncleBlockView, Version},
    packed::{Byte32, ProposalShortId},
};
use ckb_verification::cache::{CacheEntry, TxVerifyCache};
use crossbeam_channel;
use failure::Error as FailureError;
use futures::future::{self, Future};
use futures::stream::Stream;
use futures::sync::{mpsc, oneshot};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{atomic::AtomicU64, Arc};
use tokio::sync::lock::Lock;

pub const DEFAULT_CHANNEL_SIZE: usize = 512;

pub struct Request<A, R> {
    pub responder: crossbeam_channel::Sender<R>,
    pub arguments: A,
}

impl<A, R> Request<A, R> {
    pub fn call(arguments: A, responder: crossbeam_channel::Sender<R>) -> Request<A, R> {
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

pub type SubmitTxsResult = Result<Vec<CacheEntry>, Error>;
type NotifyTxsCallback = Option<Box<dyn FnOnce(SubmitTxsResult) + Send + Sync + 'static>>;

type FetchTxRPCResult = Option<(bool, TransactionView)>;

type FetchTxsWithCyclesResult = Vec<(ProposalShortId, (TransactionView, Cycle))>;

pub type ChainReorgArgs = (
    VecDeque<BlockView>,
    VecDeque<BlockView>,
    HashSet<ProposalShortId>,
    Arc<Snapshot>,
);

pub enum Message {
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
    EstimateFeeRate(Request<usize, FeeRate>),
    EstimatorTrackTx(Request<(Byte32, FeeRate, u64), ()>),
    EstimatorProcessBlock(Request<(u64, Vec<Byte32>), ()>),
}

#[derive(Clone)]
pub struct TxPoolController {
    sender: mpsc::Sender<Message>,
    executor: Executor,
    stop: StopHandler<()>,
}

impl Drop for TxPoolController {
    fn drop(&mut self) {
        self.stop.try_send();
    }
}

impl TxPoolController {
    pub fn executor(&self) -> &Executor {
        &self.executor
    }

    pub fn get_block_template(
        &self,
        bytes_limit: Option<u64>,
        proposals_limit: Option<u64>,
        max_version: Option<Version>,
    ) -> Result<BlockTemplateResult, FailureError> {
        let mut sender = self.sender.clone();
        let (responder, response) = crossbeam_channel::bounded(1);
        let request = Request::call((bytes_limit, proposals_limit, max_version), responder);
        sender.try_send(Message::BlockTemplate(request))?;
        response.recv().map_err(Into::into)
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

    pub fn submit_txs(&self, txs: Vec<TransactionView>) -> Result<SubmitTxsResult, FailureError> {
        let mut sender = self.sender.clone();
        let (responder, response) = crossbeam_channel::bounded(1);
        let request = Request::call(txs, responder);
        sender.try_send(Message::SubmitTxs(request))?;
        response.recv().map_err(Into::into)
    }

    pub fn plug_entry(
        &self,
        entries: Vec<TxEntry>,
        target: PlugTarget,
    ) -> Result<(), FailureError> {
        let mut sender = self.sender.clone();
        let (responder, response) = crossbeam_channel::bounded(1);
        let request = Request::call((entries, target), responder);
        sender.try_send(Message::PlugEntry(request))?;
        response.recv().map_err(Into::into)
    }

    pub fn notify_txs(
        &self,
        txs: Vec<TransactionView>,
        callback: NotifyTxsCallback,
    ) -> Result<(), FailureError> {
        let mut sender = self.sender.clone();
        let notify = Notify::notify((txs, callback));
        sender
            .try_send(Message::NotifyTxs(notify))
            .map_err(Into::into)
    }

    pub fn get_tx_pool_info(&self) -> Result<TxPoolInfo, FailureError> {
        let mut sender = self.sender.clone();
        let (responder, response) = crossbeam_channel::bounded(1);
        let request = Request::call((), responder);
        sender.try_send(Message::GetTxPoolInfo(request))?;
        response.recv().map_err(Into::into)
    }

    pub fn fresh_proposals_filter(
        &self,
        proposals: Vec<ProposalShortId>,
    ) -> Result<Vec<ProposalShortId>, FailureError> {
        let mut sender = self.sender.clone();
        let (responder, response) = crossbeam_channel::bounded(1);
        let request = Request::call(proposals, responder);
        sender.try_send(Message::FreshProposalsFilter(request))?;
        response.recv().map_err(Into::into)
    }

    pub fn fetch_tx_for_rpc(&self, id: ProposalShortId) -> Result<FetchTxRPCResult, FailureError> {
        let mut sender = self.sender.clone();
        let (responder, response) = crossbeam_channel::bounded(1);
        let request = Request::call(id, responder);
        sender.try_send(Message::FetchTxRPC(request))?;
        response.recv().map_err(Into::into)
    }

    pub fn fetch_txs(
        &self,
        short_ids: Vec<ProposalShortId>,
    ) -> Result<HashMap<ProposalShortId, TransactionView>, FailureError> {
        let mut sender = self.sender.clone();
        let (responder, response) = crossbeam_channel::bounded(1);
        let request = Request::call(short_ids, responder);
        sender.try_send(Message::FetchTxs(request))?;
        response.recv().map_err(Into::into)
    }

    pub fn fetch_txs_with_cycles(
        &self,
        short_ids: Vec<ProposalShortId>,
    ) -> Result<FetchTxsWithCyclesResult, FailureError> {
        let mut sender = self.sender.clone();
        let (responder, response) = crossbeam_channel::bounded(1);
        let request = Request::call(short_ids, responder);
        sender.try_send(Message::FetchTxsWithCycles(request))?;
        response.recv().map_err(Into::into)
    }

    pub fn estimate_fee_rate(&self, expect_confirm_blocks: usize) -> Result<FeeRate, FailureError> {
        let mut sender = self.sender.clone();
        let (responder, response) = crossbeam_channel::bounded(1);
        let request = Request::call(expect_confirm_blocks, responder);
        sender.try_send(Message::EstimateFeeRate(request))?;
        response.recv().map_err(Into::into)
    }

    pub fn estimator_track_tx(
        &self,
        tx_hash: Byte32,
        fee_rate: FeeRate,
        height: u64,
    ) -> Result<(), FailureError> {
        let mut sender = self.sender.clone();
        let (responder, response) = crossbeam_channel::bounded(1);
        let request = Request::call((tx_hash, fee_rate, height), responder);
        sender.try_send(Message::EstimatorTrackTx(request))?;
        response.recv().map_err(Into::into)
    }

    pub fn estimator_process_block(
        &self,
        height: u64,
        txs: impl Iterator<Item = Byte32>,
    ) -> Result<(), FailureError> {
        let mut sender = self.sender.clone();
        let (responder, response) = crossbeam_channel::bounded(1);
        let request = Request::call((height, txs.collect::<Vec<_>>()), responder);
        sender.try_send(Message::EstimatorProcessBlock(request))?;
        response.recv().map_err(Into::into)
    }
}

pub struct TxPoolServiceBuilder {
    service: Option<TxPoolService>,
}

impl TxPoolServiceBuilder {
    pub fn new(
        tx_pool_config: TxPoolConfig,
        snapshot: Arc<Snapshot>,
        block_assembler_config: Option<BlockAssemblerConfig>,
        txs_verify_cache: Lock<TxVerifyCache>,
        snapshot_mgr: Arc<SnapshotMgr>,
    ) -> TxPoolServiceBuilder {
        let last_txs_updated_at = Arc::new(AtomicU64::new(0));
        let tx_pool = TxPool::new(tx_pool_config, snapshot, Arc::clone(&last_txs_updated_at));
        let block_assembler = block_assembler_config.map(BlockAssembler::new);

        TxPoolServiceBuilder {
            service: Some(TxPoolService::new(
                tx_pool,
                block_assembler,
                txs_verify_cache,
                last_txs_updated_at,
                snapshot_mgr,
            )),
        }
    }

    pub fn start(mut self) -> TxPoolController {
        let (sender, receiver) = mpsc::channel(DEFAULT_CHANNEL_SIZE);
        let (signal_sender, signal_receiver) = oneshot::channel();

        let service = self.service.take().expect("tx pool service start once");
        let server = move |executor: Executor| {
            receiver
                .for_each(move |message| {
                    let service_clone = service.clone();
                    executor.spawn(service_clone.process(message));
                    future::ok(())
                })
                .select2(signal_receiver)
                .map(|_| ())
                .map_err(|_| ())
        };

        let (executor, thread) = new_executor(server);
        let stop = StopHandler::new(SignalSender::Future(signal_sender), thread);
        TxPoolController {
            sender,
            executor,
            stop,
        }
    }
}

#[derive(Clone)]
pub struct TxPoolService {
    tx_pool: Lock<TxPool>,
    tx_pool_config: TxPoolConfig,
    block_assembler: Option<BlockAssembler>,
    txs_verify_cache: Lock<TxVerifyCache>,
    last_txs_updated_at: Arc<AtomicU64>,
    snapshot_mgr: Arc<SnapshotMgr>,
}

impl TxPoolService {
    pub fn new(
        tx_pool: TxPool,
        block_assembler: Option<BlockAssembler>,
        txs_verify_cache: Lock<TxVerifyCache>,
        last_txs_updated_at: Arc<AtomicU64>,
        snapshot_mgr: Arc<SnapshotMgr>,
    ) -> Self {
        let tx_pool_config = tx_pool.config;
        Self {
            tx_pool: Lock::new(tx_pool),
            tx_pool_config,
            block_assembler,
            txs_verify_cache,
            last_txs_updated_at,
            snapshot_mgr,
        }
    }

    fn snapshot(&self) -> Arc<Snapshot> {
        Arc::clone(&self.snapshot_mgr.load())
    }

    fn process(&self, message: Message) -> Box<dyn Future<Item = (), Error = ()> + 'static + Send> {
        match message {
            Message::GetTxPoolInfo(Request { responder, .. }) => {
                Box::new(self.get_tx_pool_info().and_then(move |info| {
                    if let Err(e) = responder.send(info) {
                        error!("responder send get_tx_pool_info failed {:?}", e);
                    };
                    future::ok(())
                }))
            }
            Message::BlockTemplate(Request {
                responder,
                arguments: (bytes_limit, proposals_limit, max_version),
            }) => Box::new(
                self.get_block_template(bytes_limit, proposals_limit, max_version)
                    .then(move |block_template_result| {
                        if let Err(e) = responder.send(block_template_result) {
                            error!("responder send block_template_result failed {:?}", e);
                        };
                        future::ok(())
                    }),
            ),
            Message::SubmitTxs(Request {
                responder,
                arguments: txs,
            }) => Box::new(self.process_txs(txs).then(move |submit_txs_result| {
                if let Err(e) = responder.send(submit_txs_result) {
                    error!("responder send submit_txs_result failed {:?}", e);
                };
                future::ok(())
            })),
            Message::NotifyTxs(Notify {
                arguments: (txs, callback),
            }) => Box::new(self.process_txs(txs).then(|ret| {
                future::lazy(|| {
                    if let Some(call) = callback {
                        call(ret)
                    };
                    future::ok(())
                })
            })),
            Message::FreshProposalsFilter(Request {
                responder,
                arguments: proposals,
            }) => Box::new(self.fresh_proposals_filter(proposals).and_then(
                move |fresh_proposals_filter| {
                    if let Err(e) = responder.send(fresh_proposals_filter) {
                        error!("responder send fresh_proposals_filter failed {:?}", e);
                    };
                    future::ok(())
                },
            )),
            Message::FetchTxRPC(Request {
                responder,
                arguments: id,
            }) => Box::new(self.fetch_tx_for_rpc(id).and_then(move |tx| {
                if let Err(e) = responder.send(tx) {
                    error!("responder send fetch_tx_for_rpc failed {:?}", e)
                };
                future::ok(())
            })),
            Message::FetchTxs(Request {
                responder,
                arguments: short_ids,
            }) => Box::new(self.fetch_txs(short_ids).and_then(move |txs| {
                if let Err(e) = responder.send(txs) {
                    error!("responder send fetch_txs failed {:?}", e);
                };
                future::ok(())
            })),
            Message::FetchTxsWithCycles(Request {
                responder,
                arguments: short_ids,
            }) => Box::new(self.fetch_txs_with_cycles(short_ids).and_then(move |txs| {
                if let Err(e) = responder.send(txs) {
                    error!("responder send fetch_txs_with_cycles failed {:?}", e);
                };
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
            Message::PlugEntry(Request {
                responder,
                arguments: (entries, target),
            }) => Box::new(self.plug_entry(entries, target).and_then(move |_result| {
                if let Err(e) = responder.send(()) {
                    error!("responder send plug_entry failed {:?}", e);
                };
                future::ok(())
            })),
            Message::EstimateFeeRate(Request {
                responder,
                arguments: expect_confirm_blocks,
            }) => Box::new(self.estimate_fee_rate(expect_confirm_blocks).and_then(
                move |fee_rate| {
                    if let Err(e) = responder.send(fee_rate) {
                        error!("responder send estimate_fee_rate failed {:?}", e)
                    };
                    future::ok(())
                },
            )),
            Message::EstimatorTrackTx(Request {
                responder,
                arguments: (tx_hash, fee_rate, height),
            }) => Box::new(self.estimator_track_tx(tx_hash, fee_rate, height).and_then(
                move |_| {
                    if let Err(e) = responder.send(()) {
                        error!("responder send estimator_track_tx failed {:?}", e)
                    };
                    future::ok(())
                },
            )),
            Message::EstimatorProcessBlock(Request {
                responder,
                arguments: (height, txs),
            }) => Box::new(
                self.estimator_process_block(height, txs)
                    .and_then(move |_| {
                        if let Err(e) = responder.send(()) {
                            error!("responder send estimator_process_block failed {:?}", e)
                        };
                        future::ok(())
                    }),
            ),
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
    ) -> impl Future<Item = BlockTemplate, Error = FailureError> {
        if self.block_assembler.is_none() {
            future::Either::A(future::err(
                InternalErrorKind::System
                    .reason("BlockAssembler disabled")
                    .into(),
            ))
        } else {
            let block_assembler = self.block_assembler.clone().unwrap();
            let snapshot = self.snapshot();
            let consensus = snapshot.consensus();
            let cycles_limit = consensus.max_block_cycles();
            let args = BlockAssembler::transform_params(
                consensus,
                bytes_limit,
                proposals_limit,
                max_version,
            );
            let (bytes_limit, proposals_limit, version) = args;

            let cache = BlockTemplateCacheProcess::new(
                block_assembler.template_caches.clone(),
                Arc::clone(&self.last_txs_updated_at),
                Arc::clone(&block_assembler.last_uncles_updated_at),
                Arc::clone(&snapshot),
                args,
            );

            let build_cellbase = BuildCellbaseProcess::new(
                Arc::clone(&snapshot),
                Arc::clone(&block_assembler.config),
            );
            let prepare_uncle = PrepareUnclesProcess {
                snapshot: Arc::clone(&snapshot),
                last_uncles_updated_at: Arc::clone(&block_assembler.last_uncles_updated_at),
                candidate_uncles: block_assembler.candidate_uncles.clone(),
            };

            let tx_pool = self.tx_pool.clone();
            let last_txs_updated_at = Arc::clone(&self.last_txs_updated_at);

            let template_caches = block_assembler.template_caches.clone();
            let tip_hash = snapshot.tip_hash();

            let process = cache.or_else(move |_| {
                build_cellbase
                    .and_then(move |cellbase| {
                        prepare_uncle.and_then(move |(uncles, current_epoch, uncles_updated_at)| {
                            let package_txs = PackageTxsProcess {
                                tx_pool,
                                bytes_limit,
                                proposals_limit,
                                max_block_cycles: cycles_limit,
                                last_txs_updated_at,
                                cellbase: cellbase.clone(),
                                uncles: uncles.clone(),
                            };
                            package_txs.and_then(move |(proposals, entries, txs_updated_at)| {
                                BlockTemplateBuilder {
                                    snapshot: Arc::clone(&snapshot),
                                    entries,
                                    proposals,
                                    cellbase,
                                    work_id: Arc::clone(&block_assembler.work_id),
                                    current_epoch,
                                    uncles,
                                    args,
                                    uncles_updated_at,
                                    txs_updated_at,
                                }
                            })
                        })
                    })
                    .map(move |(template, uncles_updated_at, txs_updated_at)| {
                        let update_cache = UpdateBlockTemplateCache::new(
                            template_caches,
                            (tip_hash, bytes_limit, proposals_limit, version),
                            uncles_updated_at,
                            txs_updated_at,
                            template.clone(),
                        );
                        tokio::spawn(update_cache);
                        template
                    })
            });
            future::Either::B(process)
        }
    }

    fn process_txs(
        &self,
        txs: Vec<TransactionView>,
    ) -> impl Future<Item = Vec<CacheEntry>, Error = Error> {
        let keys: Vec<Byte32> = txs.iter().map(|tx| tx.hash()).collect();
        let fetched_cache = FetchCache::new(self.txs_verify_cache.clone(), keys);
        let txs_verify_cache = self.txs_verify_cache.clone();
        let tx_pool = self.tx_pool.clone();
        let max_tx_verify_cycles = self.tx_pool_config.max_tx_verify_cycles;

        let pre_resolve = PreResolveTxsProcess::new(tx_pool.clone(), txs);

        pre_resolve.and_then(move |(tip_hash, snapshot, rtxs, status)| {
            fetched_cache
                .then(move |cache| {
                    VerifyTxsProcess::new(
                        snapshot,
                        cache.expect("fetched_cache never fail"),
                        rtxs,
                        max_tx_verify_cycles,
                    )
                })
                .and_then(move |txs| SubmitTxsProcess::new(tx_pool, txs, tip_hash, status))
                .map(move |(map, cache_entry)| {
                    tokio::spawn(UpdateCache::new(txs_verify_cache, map));
                    cache_entry
                })
        })
    }

    fn plug_entry(
        &self,
        entries: Vec<TxEntry>,
        target: PlugTarget,
    ) -> impl Future<Item = (), Error = ()> {
        PlugEntryProcess::new(self.tx_pool.clone(), entries, target)
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
        short_ids: Vec<ProposalShortId>,
    ) -> impl Future<Item = HashMap<ProposalShortId, TransactionView>, Error = ()> {
        FetchTxsProcess::new(self.tx_pool.clone(), short_ids)
    }

    fn fetch_txs_with_cycles(
        &self,
        short_ids: Vec<ProposalShortId>,
    ) -> impl Future<Item = Vec<(ProposalShortId, (TransactionView, Cycle))>, Error = ()> {
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
            let block_assembler = self.block_assembler.clone().unwrap();
            future::Either::B(NewUncleProcess::new(
                block_assembler.candidate_uncles.clone(),
                Arc::clone(&block_assembler.last_uncles_updated_at),
                uncle,
            ))
        }
    }

    pub fn estimate_fee_rate(
        &self,
        expect_confirm_blocks: usize,
    ) -> impl Future<Item = FeeRate, Error = ()> {
        EstimateFeeRateProcess::new(self.tx_pool.clone(), expect_confirm_blocks)
    }

    pub fn estimator_track_tx(
        &self,
        tx_hash: Byte32,
        fee_rate: FeeRate,
        height: u64,
    ) -> impl Future<Item = (), Error = ()> {
        EstimatorTrackTxProcess::new(self.tx_pool.clone(), tx_hash, fee_rate, height)
    }

    pub fn estimator_process_block(
        &self,
        height: u64,
        txs: Vec<Byte32>,
    ) -> impl Future<Item = (), Error = ()> {
        EstimatorProcessBlockProcess::new(self.tx_pool.clone(), height, txs)
    }
}
