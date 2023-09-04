//! CKB chain service.
#![allow(missing_docs)]

use crate::forkchanges::ForkChanges;
use crate::orphan_block_pool::OrphanBlockPool;
use ckb_chain_spec::versionbits::VersionbitsIndexer;
use ckb_channel::{self as channel, select, Receiver, SendError, Sender};
use ckb_constant::sync::BLOCK_DOWNLOAD_WINDOW;
use ckb_error::{Error, InternalErrorKind};
use ckb_logger::Level::Trace;
use ckb_logger::{
    self, debug, error, info, log_enabled, log_enabled_target, trace, trace_target, warn,
};
use ckb_merkle_mountain_range::leaf_index_to_mmr_size;
use ckb_network::PeerId;
use ckb_proposal_table::ProposalTable;
#[cfg(debug_assertions)]
use ckb_rust_unstable_port::IsSorted;
use ckb_shared::block_status::BlockStatus;
use ckb_shared::shared::Shared;
use ckb_shared::types::VerifyFailedBlockInfo;
use ckb_stop_handler::{new_crossbeam_exit_rx, register_thread};
use ckb_store::{attach_block_cell, detach_block_cell, ChainStore, StoreTransaction};
use ckb_systemtime::unix_time_as_millis;
use ckb_types::{
    core::{
        cell::{
            resolve_transaction, BlockCellProvider, HeaderChecker, OverlayCellProvider,
            ResolvedTransaction,
        },
        service::{Request, DEFAULT_CHANNEL_SIZE},
        BlockExt, BlockNumber, BlockView, Cycle, HeaderView,
    },
    packed::Byte32,
    utilities::merkle_mountain_range::ChainRootMMR,
    H256, U256,
};
use ckb_util::Mutex;
use ckb_verification::cache::Completed;
use ckb_verification::{BlockVerifier, InvalidParentError, NonContextualBlockTxsVerifier};
use ckb_verification_contextual::{ContextualBlockVerifier, VerifyContext};
use ckb_verification_traits::{Switch, Verifier};
use crossbeam::channel::SendTimeoutError;
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use std::{cmp, thread};

const ORPHAN_BLOCK_SIZE: usize = (BLOCK_DOWNLOAD_WINDOW * 2) as usize;

type ProcessBlockRequest = Request<(LonelyBlock), Vec<VerifyFailedBlockInfo>>;
type TruncateRequest = Request<Byte32, Result<(), Error>>;

/// Controller to the chain service.
///
/// The controller is internally reference-counted and can be freely cloned.
///
/// A controller can invoke [`ChainService`] methods.
#[cfg_attr(feature = "mock", faux::create)]
#[derive(Clone)]
pub struct ChainController {
    process_block_sender: Sender<ProcessBlockRequest>,
    truncate_sender: Sender<TruncateRequest>,
    orphan_block_broker: Arc<OrphanBlockPool>,
}

#[cfg_attr(feature = "mock", faux::methods)]
impl ChainController {
    pub fn new(
        process_block_sender: Sender<ProcessBlockRequest>,
        truncate_sender: Sender<TruncateRequest>,
        orphan_block_broker: Arc<OrphanBlockPool>,
    ) -> Self {
        ChainController {
            process_block_sender,
            truncate_sender,
            orphan_block_broker,
        }
    }
    /// Inserts the block into database.
    ///
    /// Expects the block's header to be valid and already verified.
    ///
    /// If the block already exists, does nothing and false is returned.
    ///
    /// [BlockVerifier] [NonContextualBlockTxsVerifier] [ContextualBlockVerifier] will performed
    pub fn process_block(
        &self,
        lonely_block: LonelyBlock,
    ) -> Result<Vec<VerifyFailedBlockInfo>, Error> {
        self.internal_process_block(lonely_block)
    }

    /// Internal method insert block for test
    ///
    /// switch bit flags for particular verify, make easier to generating test data
    pub fn internal_process_block(
        &self,
        lonely_block: LonelyBlock,
    ) -> Result<Vec<VerifyFailedBlockInfo>, Error> {
        Request::call(&self.process_block_sender, lonely_block).unwrap_or_else(|| {
            Err(InternalErrorKind::System
                .other("Chain service has gone")
                .into())
        })
    }

    /// Truncate chain to specified target
    ///
    /// Should use for testing only
    pub fn truncate(&self, target_tip_hash: Byte32) -> Result<(), Error> {
        Request::call(&self.truncate_sender, target_tip_hash).unwrap_or_else(|| {
            Err(InternalErrorKind::System
                .other("Chain service has gone")
                .into())
        })
    }

    // Relay need this
    pub fn get_orphan_block(&self, hash: &Byte32) -> Option<Arc<BlockView>> {
        self.orphan_block_broker.get_block(hash)
    }

    pub fn orphan_blocks_len(&self) -> usize {
        self.orphan_block_broker.len()
    }
}

pub(crate) struct GlobalIndex {
    pub(crate) number: BlockNumber,
    pub(crate) hash: Byte32,
    pub(crate) unseen: bool,
}

impl GlobalIndex {
    pub(crate) fn new(number: BlockNumber, hash: Byte32, unseen: bool) -> GlobalIndex {
        GlobalIndex {
            number,
            hash,
            unseen,
        }
    }

    pub(crate) fn forward(&mut self, hash: Byte32) {
        self.number -= 1;
        self.hash = hash;
    }
}

/// Chain background service
///
/// The ChainService provides a single-threaded background executor.
#[derive(Clone)]
pub struct ChainService {
    shared: Shared,
    proposal_table: Arc<Mutex<ProposalTable>>,

    orphan_blocks_broker: Arc<OrphanBlockPool>,

    lonely_block_tx: Sender<(LonelyBlock)>,
    lonely_block_rx: Receiver<(LonelyBlock)>,

    unverified_tx: Sender<UnverifiedBlock>,
    unverified_rx: Receiver<UnverifiedBlock>,

    verify_failed_blocks_tx: Sender<VerifyFailedBlockInfo>,
    verify_failed_blocks_rx: Receiver<VerifyFailedBlockInfo>,
}

pub struct LonelyBlock {
    pub block: Arc<BlockView>,
    pub peer_id: Option<PeerId>,
    pub switch: Switch,
}

impl LonelyBlock {
    fn combine_parent_header(&self, parent_header: HeaderView) -> UnverifiedBlock {
        UnverifiedBlock {
            block: self.block.clone(),
            parent_header,
            peer_id: self.peer_id.clone(),
            switch: self.switch,
        }
    }
}

#[derive(Clone)]
struct UnverifiedBlock {
    block: Arc<BlockView>,
    parent_header: HeaderView,
    peer_id: Option<PeerId>,
    switch: Switch,
}

impl ChainService {
    /// Create a new ChainService instance with shared and initial proposal_table.
    pub fn new(shared: Shared, proposal_table: ProposalTable) -> ChainService {
        let (unverified_tx, unverified_rx) =
            channel::bounded::<UnverifiedBlock>(BLOCK_DOWNLOAD_WINDOW as usize * 3);

        let (new_block_tx, new_block_rx) =
            channel::bounded::<LonelyBlock>(BLOCK_DOWNLOAD_WINDOW as usize);

        let (verify_failed_blocks_tx, verify_failed_blocks_rx) = channel::unbounded();

        ChainService {
            shared,
            proposal_table: Arc::new(Mutex::new(proposal_table)),
            orphan_blocks_broker: Arc::new(OrphanBlockPool::with_capacity(ORPHAN_BLOCK_SIZE)),
            unverified_tx,
            unverified_rx,
            lonely_block_tx: new_block_tx,
            lonely_block_rx: new_block_rx,
            verify_failed_blocks_tx,
            verify_failed_blocks_rx,
        }
    }

    /// start background single-threaded service with specified thread_name.
    pub fn start<S: ToString>(mut self, thread_name: Option<S>) -> ChainController {
        let orphan_blocks_broker_clone = Arc::clone(&self.orphan_blocks_broker);

        let signal_receiver = new_crossbeam_exit_rx();
        let (process_block_sender, process_block_receiver) =
            channel::bounded(BLOCK_DOWNLOAD_WINDOW as usize);

        let (truncate_sender, truncate_receiver) = channel::bounded(1);

        // Mainly for test: give an empty thread_name
        let mut thread_builder = thread::Builder::new();
        if let Some(name) = thread_name {
            thread_builder = thread_builder.name(name.to_string());
        }
        let tx_control = self.shared.tx_pool_controller().clone();
        let (unverified_queue_stop_tx, unverified_queue_stop_rx) = ckb_channel::bounded::<()>(1);
        let (search_orphan_pool_stop_tx, search_orphan_pool_stop_rx) =
            ckb_channel::bounded::<()>(1);

        let unverified_consumer_thread = thread::Builder::new()
            .name("verify_blocks".into())
            .spawn({
                let chain_service = self.clone();
                move || chain_service.start_consume_unverified_blocks(unverified_queue_stop_rx)
            })
            .expect("start unverified_queue consumer thread should ok");

        let search_orphan_pool_thread = thread::Builder::new()
            .name("search_orphan".into())
            .spawn({
                let chain_service = self.clone();
                move || chain_service.start_search_orphan_pool(search_orphan_pool_stop_rx)
            })
            .expect("start search_orphan_pool thread should ok");

        let chain_jh = thread_builder
            .spawn(move || loop {
                select! {
                    recv(process_block_receiver) -> msg => match msg {
                        Ok(Request { responder, arguments: (block, peer_id, verify) }) => {
                            let _ = tx_control.suspend_chunk_process();
                            let _ = responder.send(self.process_block_v2(block, peer_id, verify));
                            let _ = tx_control.continue_chunk_process();

                            if let Some(metrics) = ckb_metrics::handle() {
                                metrics
                                    .ckb_block_process_duration
                                    .observe(instant.elapsed().as_secs_f64());
                            }
                        },
                        _ => {
                            error!("process_block_receiver closed");
                            break;
                        },
                    },
                    recv(truncate_receiver) -> msg => match msg {
                        Ok(Request { responder, arguments: target_tip_hash }) => {
                            let _ = tx_control.suspend_chunk_process();
                            let _ = responder.send(self.truncate(&target_tip_hash));
                            let _ = tx_control.continue_chunk_process();
                        },
                        _ => {
                            error!("truncate_receiver closed");
                            break;
                        },
                    },
                    recv(signal_receiver) -> _ => {
                        info!("ChainService received exit signal, exit now");
                        unverified_queue_stop_tx.send(());
                        search_orphan_pool_stop_tx.send(());

                        search_orphan_pool_thread.join();
                        unverified_consumer_thread.join();
                        break;
                    }
                }
            })
            .expect("Start ChainService failed");

        register_thread("ChainService", chain_jh);

        ChainController::new(
            process_block_sender,
            truncate_sender,
            orphan_blocks_broker_clone,
        )
    }

    fn start_consume_unverified_blocks(&self, unverified_queue_stop_rx: Receiver<()>) {
        let mut begin_loop = std::time::Instant::now();
        loop {
            begin_loop = std::time::Instant::now();
            select! {
                recv(unverified_queue_stop_rx) -> _ => {
                        info!("unverified_queue_consumer got exit signal, exit now");
                        return;
                },
                recv(self.unverified_rx) -> msg => match msg {
                    Ok(unverified_task) => {
                    // process this unverified block
                        trace!("got an unverified block, wait cost: {:?}", begin_loop.elapsed());
                        self.consume_unverified_blocks(unverified_task);
                        trace!("consume_unverified_blocks cost: {:?}", begin_loop.elapsed());
                    },
                    Err(err) => {
                        error!("unverified_rx err: {}", err);
                        return;
                    },
                },
                default => {},
            }
        }
    }

    fn consume_unverified_blocks(&self, unverified_block: UnverifiedBlock) {
        // process this unverified block
        match self.verify_block(&unverified_block) {
            Ok(_) => {
                let log_now = std::time::Instant::now();
                self.shared
                    .remove_block_status(&unverified_block.block.hash());
                let log_elapsed_remove_block_status = log_now.elapsed();
                self.shared
                    .remove_header_view(&unverified_block.block.hash());
                debug!(
                    "block {} remove_block_status cost: {:?}, and header_view cost: {:?}",
                    unverified_block.block.hash(),
                    log_elapsed_remove_block_status,
                    log_now.elapsed()
                );
            }
            Err(err) => {
                error!(
                    "verify [{:?}]'s block {} failed: {}",
                    unverified_block.peer_id,
                    unverified_block.block.hash(),
                    err
                );
                if let Some(peer_id) = unverified_block.peer_id {
                    if let Err(SendError(peer_id)) =
                        self.verify_failed_blocks_tx.send(VerifyFailedBlockInfo {
                            block_hash: unverified_block.block.hash(),
                            peer_id,
                        })
                    {
                        error!(
                            "send verify_failed_blocks_tx failed for peer: {:?}",
                            peer_id
                        );
                    }
                }

                let tip = self
                    .shared
                    .store()
                    .get_tip_header()
                    .expect("tip_header must exist");
                let tip_ext = self
                    .shared
                    .store()
                    .get_block_ext(&tip.hash())
                    .expect("tip header's ext must exist");

                self.shared.set_unverified_tip(ckb_shared::HeaderIndex::new(
                    tip.clone().number(),
                    tip.clone().hash(),
                    tip_ext.total_difficulty,
                ));

                self.shared
                    .insert_block_status(unverified_block.block.hash(), BlockStatus::BLOCK_INVALID);
                error!(
                    "set_unverified tip to {}-{}, because verify {} failed: {}",
                    tip.number(),
                    tip.hash(),
                    unverified_block.block.hash(),
                    err
                );
            }
        }
    }

    fn start_search_orphan_pool(&self, search_orphan_pool_stop_rx: Receiver<()>) {
        loop {
            select! {
                recv(search_orphan_pool_stop_rx) -> _ => {
                        info!("unverified_queue_consumer got exit signal, exit now");
                        return;
                },
                recv(self.lonely_block_rx) -> msg => match msg {
                    Ok(lonely_block) => {
                        self.orphan_blocks_broker.insert(lonely_block);
                        self.search_orphan_pool()
                    },
                    Err(err) => {
                        error!("lonely_block_rx err: {}", err);
                        return
                    }
                },
            }
        }
    }
    fn search_orphan_pool(&self, switch: Switch) {
        for leader_hash in self.orphan_blocks_broker.clone_leaders() {
            if !self
                .shared
                .contains_block_status(&leader_hash, BlockStatus::BLOCK_PARTIAL_STORED)
            {
                trace!("orphan leader: {} not partial stored", leader_hash);
                continue;
            }

            let descendants: Vec<LonelyBlock> = self
                .orphan_blocks_broker
                .remove_blocks_by_parent(&leader_hash);
            if descendants.is_empty() {
                error!(
                    "leader {} does not have any descendants, this shouldn't happen",
                    leader_hash
                );
                continue;
            }

            let mut accept_error_occurred = false;
            for descendant_block in &descendants {
                let &LonelyBlock {
                    block: descendant,
                    peer_id,
                    switch,
                } = descendant_block;
                match self.accept_block(descendant.to_owned()) {
                    Err(err) => {
                        accept_error_occurred = true;
                        error!("accept block {} failed: {}", descendant.hash(), err);
                        continue;
                    }
                    Ok(accepted_opt) => match accepted_opt {
                        Some((parent_header, total_difficulty)) => {
                            let unverified_block: UnverifiedBlock =
                                descendant_block.combine_parent_header(parent_header);
                            match self.unverified_tx.send(unverified_block) {
                                Ok(_) => {}
                                Err(err) => error!("send unverified_tx failed: {}", err),
                            };

                            if total_difficulty
                                .gt(self.shared.get_unverified_tip().total_difficulty())
                            {
                                self.shared.set_unverified_tip(ckb_shared::HeaderIndex::new(
                                    descendant.header().number(),
                                    descendant.header().hash(),
                                    total_difficulty,
                                ));
                                debug!("set unverified_tip to {}-{}, while unverified_tip - verified_tip = {}",
                            descendant.number(),
                            descendant.hash(),
                            descendant
                                .number()
                                .saturating_sub(self.shared.snapshot().tip_number()))
                            } else {
                                debug!("received a block {}-{} with lower or equal difficulty than unverified_tip {}-{}",
                                    descendant.number(),
                                    descendant.hash(),
                                    self.shared.get_unverified_tip().number(),
                                    self.shared.get_unverified_tip().hash(),
                                    );
                            }
                        }
                        None => {
                            info!(
                                "doesn't accept block {}, because it has been stored",
                                descendant.hash()
                            );
                        }
                    },
                }
            }

            if !accept_error_occurred {
                debug!(
                    "accept {} blocks [{}->{}] success",
                    descendants.len(),
                    descendants.first().expect("descendants not empty").number(),
                    descendants.last().expect("descendants not empty").number(),
                )
            }
        }
    }

    fn make_fork_for_truncate(&self, target: &HeaderView, current_tip: &HeaderView) -> ForkChanges {
        let mut fork = ForkChanges::default();
        let store = self.shared.store();
        for bn in (target.number() + 1)..=current_tip.number() {
            let hash = store.get_block_hash(bn).expect("index checked");
            let old_block = store.get_block(&hash).expect("index checked");
            fork.detached_blocks.push_back(old_block);
        }
        is_sorted_assert(&fork);
        fork
    }

    // Truncate the main chain
    // Use for testing only
    pub(crate) fn truncate(&mut self, target_tip_hash: &Byte32) -> Result<(), Error> {
        let snapshot = Arc::clone(&self.shared.snapshot());
        assert!(snapshot.is_main_chain(target_tip_hash));

        let target_tip_header = snapshot.get_block_header(target_tip_hash).expect("checked");
        let target_block_ext = snapshot.get_block_ext(target_tip_hash).expect("checked");
        let target_epoch_ext = snapshot
            .get_block_epoch_index(target_tip_hash)
            .and_then(|index| snapshot.get_epoch_ext(&index))
            .expect("checked");
        let origin_proposals = snapshot.proposals();
        let mut fork = self.make_fork_for_truncate(&target_tip_header, snapshot.tip_header());

        let db_txn = self.shared.store().begin_transaction();
        self.rollback(&fork, &db_txn)?;

        db_txn.insert_tip_header(&target_tip_header)?;
        db_txn.insert_current_epoch_ext(&target_epoch_ext)?;

        for blk in fork.attached_blocks() {
            db_txn.delete_block(blk)?;
        }
        db_txn.commit()?;

        self.update_proposal_table(&fork);
        let (detached_proposal_id, new_proposals) = self
            .proposal_table
            .lock()
            .finalize(origin_proposals, target_tip_header.number());
        fork.detached_proposal_id = detached_proposal_id;

        let new_snapshot = self.shared.new_snapshot(
            target_tip_header,
            target_block_ext.total_difficulty,
            target_epoch_ext,
            new_proposals,
        );

        self.shared.store_snapshot(Arc::clone(&new_snapshot));

        // NOTE: Dont update tx-pool when truncate

        Ok(())
    }

    // visible pub just for test
    #[doc(hidden)]
    pub fn process_block(&mut self, block: Arc<BlockView>, switch: Switch) -> Result<bool, Error> {
        let block_number = block.number();
        let block_hash = block.hash();

        debug!("Begin processing block: {}-{}", block_number, block_hash);
        if block_number < 1 {
            warn!("Receive 0 number block: 0-{}", block_hash);
        }

        self.insert_block(block, switch).map(|ret| {
            debug!("Finish processing block");
            ret
        })
    }

    fn non_contextual_verify(&self, block: &BlockView) -> Result<(), Error> {
        let consensus = self.shared.consensus();
        BlockVerifier::new(consensus).verify(block).map_err(|e| {
            debug!("[process_block] BlockVerifier error {:?}", e);
            e
        })?;

        NonContextualBlockTxsVerifier::new(consensus)
            .verify(block)
            .map_err(|e| {
                debug!(
                    "[process_block] NonContextualBlockTxsVerifier error {:?}",
                    e
                );
                e
            })
            .map(|_| ())
    }

    // make block IO and verify asynchronize
    #[doc(hidden)]
    pub fn process_block_v2(&self, lonely_block: LonelyBlock) -> Vec<VerifyFailedBlockInfo> {
        let block_number = lonely_block.block.number();
        let block_hash = lonely_block.block.hash();
        if block_number < 1 {
            warn!("receive 0 number block: 0-{}", block_hash);
        }

        let mut failed_blocks_peer_ids: Vec<VerifyFailedBlockInfo> =
            self.verify_failed_blocks_rx.iter().collect();

        if !lonely_block.switch.disable_non_contextual() {
            let result = self.non_contextual_verify(&lonely_block.block);
            match result {
                Err(err) => {
                    if let Some(peer_id) = lonely_block.peer_id {
                        failed_blocks_peer_ids.push(VerifyFailedBlockInfo {
                            block_hash,
                            peer_id,
                        });
                    }
                    return failed_blocks_peer_ids;
                }
                _ => {}
            }
        }

        match self.lonely_block_tx.send(lonely_block) {
            Ok(_) => {}
            Err(err) => {
                error!("notify new block to orphan pool err: {}", err)
            }
        }
        debug!(
            "processing block: {}-{}, orphan_len: {}, (tip:unverified_tip):({}:{}), and return failed_blocks_peer_ids: {:?}",
            block_number,
            block_hash,
            self.orphan_blocks_broker.len(),
            self.shared.snapshot().tip_number(),
            self.shared.get_unverified_tip().number(),
            failed_blocks_peer_ids,
        );

        failed_blocks_peer_ids
    }

    fn accept_block(&self, block: Arc<BlockView>) -> Result<Option<(HeaderView, U256)>, Error> {
        let (block_number, block_hash) = (block.number(), block.hash());

        if self
            .shared
            .contains_block_status(&block_hash, BlockStatus::BLOCK_PARTIAL_STORED)
        {
            debug!("block {}-{} has been stored", block_number, block_hash);
            return Ok(None);
        }

        let parent_header = self
            .shared
            .store()
            .get_block_header(&block.data().header().raw().parent_hash())
            .expect("parent already store");

        if let Some(ext) = self.shared.store().get_block_ext(&block.hash()) {
            debug!("block {}-{} has stored BlockExt", block_number, block_hash);
            return Ok(Some((parent_header, ext.total_difficulty)));
        }

        trace!("begin accept block: {}-{}", block.number(), block.hash());

        let parent_ext = self
            .shared
            .store()
            .get_block_ext(&block.data().header().raw().parent_hash())
            .expect("parent already store");

        let cannon_total_difficulty =
            parent_ext.total_difficulty.to_owned() + block.header().difficulty();

        let db_txn = Arc::new(self.shared.store().begin_transaction());

        let txn_snapshot = db_txn.get_snapshot();
        let _snapshot_block_ext = db_txn.get_update_for_block_ext(&block.hash(), &txn_snapshot);

        db_txn.insert_block(block.as_ref())?;

        // if parent_ext.verified == Some(false) {
        //     return Err(InvalidParentError {
        //         parent_hash: parent_header.hash(),
        //     }
        //     .into());
        // }

        let next_block_epoch = self
            .shared
            .consensus()
            .next_epoch_ext(&parent_header, &db_txn.borrow_as_data_loader())
            .expect("epoch should be stored");
        let new_epoch = next_block_epoch.is_head();
        let epoch = next_block_epoch.epoch();

        db_txn.insert_block_epoch_index(
            &block.header().hash(),
            &epoch.last_block_hash_in_previous_epoch(),
        )?;
        if new_epoch {
            db_txn.insert_epoch_ext(&epoch.last_block_hash_in_previous_epoch(), &epoch)?;
        }

        let ext = BlockExt {
            received_at: unix_time_as_millis(),
            total_difficulty: cannon_total_difficulty.clone(),
            total_uncles_count: parent_ext.total_uncles_count + block.data().uncles().len() as u64,
            verified: None,
            txs_fees: vec![],
            cycles: None,
            txs_sizes: None,
        };

        db_txn.insert_block_ext(&block.header().hash(), &ext)?;

        db_txn.commit()?;

        self.shared
            .insert_block_status(block_hash, BlockStatus::BLOCK_PARTIAL_STORED);

        Ok(Some((parent_header, cannon_total_difficulty)))
    }

    fn verify_block(&self, unverified_block: &UnverifiedBlock) -> Result<bool, Error> {
        let log_now = std::time::Instant::now();

        let UnverifiedBlock {
            block,
            parent_header,
            peer_id,
            switch,
        } = unverified_block;

        let parent_ext = self
            .shared
            .store()
            .get_block_ext(&block.data().header().raw().parent_hash())
            .expect("parent already store");

        if let Some(ext) = self.shared.store().get_block_ext(&block.hash()) {
            match ext.verified {
                Some(verified) => {
                    debug!(
                        "block {}-{} has been verified: {}",
                        block.number(),
                        block.hash(),
                        verified
                    );
                    return Ok(verified);
                }
                _ => {}
            }
        }

        let cannon_total_difficulty =
            parent_ext.total_difficulty.to_owned() + block.header().difficulty();

        if parent_ext.verified == Some(false) {
            return Err(InvalidParentError {
                parent_hash: parent_header.hash(),
            }
            .into());
        }

        let ext = BlockExt {
            received_at: unix_time_as_millis(),
            total_difficulty: cannon_total_difficulty.clone(),
            total_uncles_count: parent_ext.total_uncles_count + block.data().uncles().len() as u64,
            verified: None,
            txs_fees: vec![],
            cycles: None,
            txs_sizes: None,
        };

        let shared_snapshot = Arc::clone(&self.shared.snapshot());
        let origin_proposals = shared_snapshot.proposals();
        let current_tip_header = shared_snapshot.tip_header();
        let current_total_difficulty = shared_snapshot.total_difficulty().to_owned();

        // is_better_than
        let new_best_block = cannon_total_difficulty > current_total_difficulty;

        let mut fork = ForkChanges::default();

        let next_block_epoch = self
            .shared
            .consensus()
            .next_epoch_ext(&parent_header, &self.shared.store().borrow_as_data_loader())
            .expect("epoch should be stored");
        let new_epoch = next_block_epoch.is_head();
        let epoch = next_block_epoch.epoch();

        let db_txn = Arc::new(self.shared.store().begin_transaction());
        if new_best_block {
            debug!(
                "[verify block] new best block found: {} => {:#x}, difficulty diff = {:#x}, unverified_tip: {}",
                block.header().number(),
                block.header().hash(),
                &cannon_total_difficulty - &current_total_difficulty,
                self.shared.get_unverified_tip().number(),
            );
            self.find_fork(&mut fork, current_tip_header.number(), &block, ext);
            self.rollback(&fork, &db_txn)?;

            // update and verify chain root
            // MUST update index before reconcile_main_chain
            let begin_reconcile_main_chain = std::time::Instant::now();
            self.reconcile_main_chain(Arc::clone(&db_txn), &mut fork, switch.to_owned())?;
            trace!(
                "reconcile_main_chain cost {:?}",
                begin_reconcile_main_chain.elapsed()
            );

            db_txn.insert_tip_header(&block.header())?;
            if new_epoch || fork.has_detached() {
                db_txn.insert_current_epoch_ext(&epoch)?;
            }
        } else {
            db_txn.insert_block_ext(&block.header().hash(), &ext)?;
        }
        db_txn.commit()?;

        if new_best_block {
            let tip_header = block.header();
            info!(
                "block: {}, hash: {:#x}, epoch: {:#}, total_diff: {:#x}, txs: {}",
                tip_header.number(),
                tip_header.hash(),
                tip_header.epoch(),
                cannon_total_difficulty,
                block.transactions().len()
            );

            self.update_proposal_table(&fork);
            let (detached_proposal_id, new_proposals) = self
                .proposal_table
                .lock()
                .finalize(origin_proposals, tip_header.number());
            fork.detached_proposal_id = detached_proposal_id;

            let new_snapshot =
                self.shared
                    .new_snapshot(tip_header, cannon_total_difficulty, epoch, new_proposals);

            self.shared.store_snapshot(Arc::clone(&new_snapshot));

            let tx_pool_controller = self.shared.tx_pool_controller();
            if tx_pool_controller.service_started() {
                if let Err(e) = tx_pool_controller.update_tx_pool_for_reorg(
                    fork.detached_blocks().clone(),
                    fork.attached_blocks().clone(),
                    fork.detached_proposal_id().clone(),
                    new_snapshot,
                ) {
                    error!("[verify block] notify update_tx_pool_for_reorg error {}", e);
                }
            }

            let block_ref: &BlockView = &block;
            self.shared
                .notify_controller()
                .notify_new_block(block_ref.clone());
            if log_enabled!(ckb_logger::Level::Trace) {
                self.print_chain(10);
            }
            if let Some(metrics) = ckb_metrics::handle() {
                metrics.ckb_chain_tip.set(block.header().number() as i64);
            }
        } else {
            self.shared.refresh_snapshot();
            info!(
                "[verify block] uncle: {}, hash: {:#x}, epoch: {:#}, total_diff: {:#x}, txs: {}",
                block.header().number(),
                block.header().hash(),
                block.header().epoch(),
                cannon_total_difficulty,
                block.transactions().len()
            );

            let tx_pool_controller = self.shared.tx_pool_controller();
            if tx_pool_controller.service_started() {
                let block_ref: &BlockView = &block;
                if let Err(e) = tx_pool_controller.notify_new_uncle(block_ref.as_uncle()) {
                    error!("[verify block] notify new_uncle error {}", e);
                }
            }
        }
        Ok(true)
    }

    fn insert_block(&mut self, block: Arc<BlockView>, switch: Switch) -> Result<bool, Error> {
        let db_txn = Arc::new(self.shared.store().begin_transaction());
        let txn_snapshot = db_txn.get_snapshot();
        let _snapshot_tip_hash = db_txn.get_update_for_tip_hash(&txn_snapshot);

        // insert_block are assumed be executed in single thread
        if txn_snapshot.block_exists(&block.header().hash()) {
            return Ok(false);
        }
        // non-contextual verify
        if !switch.disable_non_contextual() {
            self.non_contextual_verify(&block)?;
        }

        let mut total_difficulty = U256::zero();
        let mut fork = ForkChanges::default();

        let parent_ext = txn_snapshot
            .get_block_ext(&block.data().header().raw().parent_hash())
            .expect("parent already store");

        let parent_header = txn_snapshot
            .get_block_header(&block.data().header().raw().parent_hash())
            .expect("parent already store");

        let cannon_total_difficulty =
            parent_ext.total_difficulty.to_owned() + block.header().difficulty();

        if parent_ext.verified == Some(false) {
            return Err(InvalidParentError {
                parent_hash: parent_header.hash(),
            }
            .into());
        }

        db_txn.insert_block(&block)?;

        let next_block_epoch = self
            .shared
            .consensus()
            .next_epoch_ext(&parent_header, &txn_snapshot.borrow_as_data_loader())
            .expect("epoch should be stored");
        let new_epoch = next_block_epoch.is_head();
        let epoch = next_block_epoch.epoch();

        let ext = BlockExt {
            received_at: unix_time_as_millis(),
            total_difficulty: cannon_total_difficulty.clone(),
            total_uncles_count: parent_ext.total_uncles_count + block.data().uncles().len() as u64,
            verified: None,
            txs_fees: vec![],
            cycles: None,
            txs_sizes: None,
        };

        db_txn.insert_block_epoch_index(
            &block.header().hash(),
            &epoch.last_block_hash_in_previous_epoch(),
        )?;
        if new_epoch {
            db_txn.insert_epoch_ext(&epoch.last_block_hash_in_previous_epoch(), &epoch)?;
        }

        let shared_snapshot = Arc::clone(&self.shared.snapshot());
        let origin_proposals = shared_snapshot.proposals();
        let current_tip_header = shared_snapshot.tip_header();

        let current_total_difficulty = shared_snapshot.total_difficulty().to_owned();
        debug!(
            "Current difficulty = {:#x}, cannon = {:#x}",
            current_total_difficulty, cannon_total_difficulty,
        );

        // is_better_than
        let new_best_block = cannon_total_difficulty > current_total_difficulty;

        if new_best_block {
            debug!(
                "Newly found best block : {} => {:#x}, difficulty diff = {:#x}",
                block.header().number(),
                block.header().hash(),
                &cannon_total_difficulty - &current_total_difficulty
            );
            self.find_fork(&mut fork, current_tip_header.number(), &block, ext);
            self.rollback(&fork, &db_txn)?;

            // update and verify chain root
            // MUST update index before reconcile_main_chain
            self.reconcile_main_chain(Arc::clone(&db_txn), &mut fork, switch)?;

            db_txn.insert_tip_header(&block.header())?;
            if new_epoch || fork.has_detached() {
                db_txn.insert_current_epoch_ext(&epoch)?;
            }
            total_difficulty = cannon_total_difficulty.clone();
        } else {
            db_txn.insert_block_ext(&block.header().hash(), &ext)?;
        }
        db_txn.commit()?;

        if new_best_block {
            let tip_header = block.header();
            info!(
                "block: {}, hash: {:#x}, epoch: {:#}, total_diff: {:#x}, txs: {}",
                tip_header.number(),
                tip_header.hash(),
                tip_header.epoch(),
                total_difficulty,
                block.transactions().len()
            );

            self.update_proposal_table(&fork);
            let (detached_proposal_id, new_proposals) = self
                .proposal_table
                .lock()
                .finalize(origin_proposals, tip_header.number());
            fork.detached_proposal_id = detached_proposal_id;

            let new_snapshot =
                self.shared
                    .new_snapshot(tip_header, total_difficulty, epoch, new_proposals);

            self.shared.store_snapshot(Arc::clone(&new_snapshot));

            let tx_pool_controller = self.shared.tx_pool_controller();
            if tx_pool_controller.service_started() {
                if let Err(e) = tx_pool_controller.update_tx_pool_for_reorg(
                    fork.detached_blocks().clone(),
                    fork.attached_blocks().clone(),
                    fork.detached_proposal_id().clone(),
                    new_snapshot,
                ) {
                    error!("Notify update_tx_pool_for_reorg error {}", e);
                }
            }

            let block_ref: &BlockView = &block;
            self.shared
                .notify_controller()
                .notify_new_block(block_ref.clone());
            if log_enabled!(ckb_logger::Level::Debug) {
                self.print_chain(10);
            }
            if let Some(metrics) = ckb_metrics::handle() {
                metrics.ckb_chain_tip.set(block.header().number() as i64);
            }
        } else {
            self.shared.refresh_snapshot();
            info!(
                "uncle: {}, hash: {:#x}, epoch: {:#}, total_diff: {:#x}, txs: {}",
                block.header().number(),
                block.header().hash(),
                block.header().epoch(),
                cannon_total_difficulty,
                block.transactions().len()
            );

            let tx_pool_controller = self.shared.tx_pool_controller();
            if tx_pool_controller.service_started() {
                let block_ref: &BlockView = &block;
                if let Err(e) = tx_pool_controller.notify_new_uncle(block_ref.as_uncle()) {
                    error!("Notify new_uncle error {}", e);
                }
            }
        }

        Ok(true)
    }

    pub(crate) fn update_proposal_table(&self, fork: &ForkChanges) {
        for blk in fork.detached_blocks() {
            self.proposal_table.lock().remove(blk.header().number());
        }
        for blk in fork.attached_blocks() {
            self.proposal_table
                .lock()
                .insert(blk.header().number(), blk.union_proposal_ids());
        }
        self.reload_proposal_table(fork);
    }

    // if rollback happen, go back check whether need reload proposal_table from block
    pub(crate) fn reload_proposal_table(&self, fork: &ForkChanges) {
        if fork.has_detached() {
            let proposal_window = self.shared.consensus().tx_proposal_window();
            let detached_front = fork
                .detached_blocks()
                .front()
                .map(|blk| blk.header().number())
                .expect("detached_blocks is not empty");
            if detached_front < 2 {
                return;
            }
            let common = detached_front - 1;
            let new_tip = fork
                .attached_blocks()
                .back()
                .map(|blk| blk.header().number())
                .unwrap_or(common);

            let proposal_start =
                cmp::max(1, (new_tip + 1).saturating_sub(proposal_window.farthest()));

            debug!("Reload_proposal_table [{}, {}]", proposal_start, common);
            for bn in proposal_start..=common {
                let blk = self
                    .shared
                    .store()
                    .get_block_hash(bn)
                    .and_then(|hash| self.shared.store().get_block(&hash))
                    .expect("block stored");

                self.proposal_table
                    .lock()
                    .insert(bn, blk.union_proposal_ids());
            }
        }
    }

    pub(crate) fn rollback(&self, fork: &ForkChanges, txn: &StoreTransaction) -> Result<(), Error> {
        for block in fork.detached_blocks().iter().rev() {
            txn.detach_block(block)?;
            detach_block_cell(txn, block)?;
        }
        Ok(())
    }

    fn alignment_fork(
        &self,
        fork: &mut ForkChanges,
        index: &mut GlobalIndex,
        new_tip_number: BlockNumber,
        current_tip_number: BlockNumber,
    ) {
        if new_tip_number <= current_tip_number {
            for bn in new_tip_number..=current_tip_number {
                let hash = self
                    .shared
                    .store()
                    .get_block_hash(bn)
                    .expect("block hash stored before alignment_fork");
                let old_block = self
                    .shared
                    .store()
                    .get_block(&hash)
                    .expect("block data stored before alignment_fork");
                fork.detached_blocks.push_back(old_block);
            }
        } else {
            while index.number > current_tip_number {
                if index.unseen {
                    let ext = self
                        .shared
                        .store()
                        .get_block_ext(&index.hash)
                        .expect("block ext stored before alignment_fork");
                    if ext.verified.is_none() {
                        fork.dirty_exts.push_front(ext)
                    } else {
                        index.unseen = false;
                    }
                }
                let new_block = self
                    .shared
                    .store()
                    .get_block(&index.hash)
                    .expect("block data stored before alignment_fork");
                index.forward(new_block.data().header().raw().parent_hash());
                fork.attached_blocks.push_front(new_block);
            }
        }
    }

    fn find_fork_until_latest_common(&self, fork: &mut ForkChanges, index: &mut GlobalIndex) {
        loop {
            if index.number == 0 {
                break;
            }
            let detached_hash = self
                .shared
                .store()
                .get_block_hash(index.number)
                .expect("detached hash stored before find_fork_until_latest_common");
            if detached_hash == index.hash {
                break;
            }
            let detached_blocks = self
                .shared
                .store()
                .get_block(&detached_hash)
                .expect("detached block stored before find_fork_until_latest_common");
            fork.detached_blocks.push_front(detached_blocks);

            if index.unseen {
                let ext = self
                    .shared
                    .store()
                    .get_block_ext(&index.hash)
                    .expect("block ext stored before find_fork_until_latest_common");
                if ext.verified.is_none() {
                    fork.dirty_exts.push_front(ext)
                } else {
                    index.unseen = false;
                }
            }

            let attached_block = self
                .shared
                .store()
                .get_block(&index.hash)
                .expect("attached block stored before find_fork_until_latest_common");
            index.forward(attached_block.data().header().raw().parent_hash());
            fork.attached_blocks.push_front(attached_block);
        }
    }

    pub(crate) fn find_fork(
        &self,
        fork: &mut ForkChanges,
        current_tip_number: BlockNumber,
        new_tip_block: &BlockView,
        new_tip_ext: BlockExt,
    ) {
        let new_tip_number = new_tip_block.header().number();
        fork.dirty_exts.push_front(new_tip_ext);

        // attached_blocks = forks[latest_common + 1 .. new_tip]
        // detached_blocks = chain[latest_common + 1 .. old_tip]
        fork.attached_blocks.push_front(new_tip_block.clone());

        let mut index = GlobalIndex::new(
            new_tip_number - 1,
            new_tip_block.data().header().raw().parent_hash(),
            true,
        );

        // if new_tip_number <= current_tip_number
        // then detached_blocks.extend(chain[new_tip_number .. =current_tip_number])
        // if new_tip_number > current_tip_number
        // then attached_blocks.extend(forks[current_tip_number + 1 .. =new_tip_number])
        self.alignment_fork(fork, &mut index, new_tip_number, current_tip_number);

        // find latest common ancestor
        self.find_fork_until_latest_common(fork, &mut index);

        is_sorted_assert(fork);
    }

    // we found new best_block
    pub(crate) fn reconcile_main_chain(
        &self,
        txn: Arc<StoreTransaction>,
        fork: &mut ForkChanges,
        switch: Switch,
    ) -> Result<(), Error> {
        if fork.attached_blocks().is_empty() {
            return Ok(());
        }

        let txs_verify_cache = self.shared.txs_verify_cache();

        let consensus = self.shared.consensus();
        let hardfork_switch = consensus.hardfork_switch();
        let during_hardfork = fork.during_hardfork(hardfork_switch);
        let async_handle = self.shared.tx_pool_controller().handle();

        if during_hardfork {
            async_handle.block_on(async {
                txs_verify_cache.write().await.clear();
            });
        }

        let consensus = self.shared.cloned_consensus();
        let start_block_header = fork.attached_blocks()[0].header();
        let mmr_size = leaf_index_to_mmr_size(start_block_header.number() - 1);
        trace!("light-client: new chain root MMR with size = {}", mmr_size);
        let mut mmr = ChainRootMMR::new(mmr_size, txn.as_ref());

        let verified_len = fork.verified_len();
        for b in fork.attached_blocks().iter().take(verified_len) {
            txn.attach_block(b)?;
            attach_block_cell(&txn, b)?;
            mmr.push(b.digest())
                .map_err(|e| InternalErrorKind::MMR.other(e))?;
        }

        let verify_context = VerifyContext::new(Arc::clone(&txn), consensus);

        let mut found_error = None;
        for (ext, b) in fork
            .dirty_exts
            .iter()
            .zip(fork.attached_blocks.iter().skip(verified_len))
        {
            if !switch.disable_all() {
                if found_error.is_none() {
                    let log_now = std::time::Instant::now();
                    let resolved = self.resolve_block_transactions(&txn, b, &verify_context);
                    debug!(
                        "resolve_block_transactions {} cost: {:?}",
                        b.hash(),
                        log_now.elapsed()
                    );
                    match resolved {
                        Ok(resolved) => {
                            let verified = {
                                let contextual_block_verifier = ContextualBlockVerifier::new(
                                    verify_context.clone(),
                                    async_handle,
                                    switch,
                                    Arc::clone(&txs_verify_cache),
                                    &mmr,
                                );
                                let log_now = std::time::Instant::now();
                                let verify_result = contextual_block_verifier.verify(&resolved, b);
                                debug!(
                                    "contextual_block_verifier {} cost: {:?}",
                                    b.hash(),
                                    log_now.elapsed()
                                );
                                verify_result
                            };
                            match verified {
                                Ok((cycles, cache_entries)) => {
                                    let txs_sizes = resolved
                                        .iter()
                                        .map(|rtx| {
                                            rtx.transaction.data().serialized_size_in_block() as u64
                                        })
                                        .collect();
                                    txn.attach_block(b)?;
                                    attach_block_cell(&txn, b)?;
                                    mmr.push(b.digest())
                                        .map_err(|e| InternalErrorKind::MMR.other(e))?;

                                    self.insert_ok_ext(
                                        &txn,
                                        &b.header().hash(),
                                        ext.clone(),
                                        Some(&cache_entries),
                                        Some(txs_sizes),
                                    )?;

                                    if !switch.disable_script() && b.transactions().len() > 1 {
                                        self.monitor_block_txs_verified(
                                            b,
                                            &resolved,
                                            &cache_entries,
                                            cycles,
                                        );
                                    }
                                }
                                Err(err) => {
                                    self.print_error(b, &err);
                                    found_error = Some(err);
                                    self.insert_failure_ext(&txn, &b.header().hash(), ext.clone())?;
                                }
                            }
                        }
                        Err(err) => {
                            found_error = Some(err);
                            self.insert_failure_ext(&txn, &b.header().hash(), ext.clone())?;
                        }
                    }
                } else {
                    self.insert_failure_ext(&txn, &b.header().hash(), ext.clone())?;
                }
            } else {
                txn.attach_block(b)?;
                attach_block_cell(&txn, b)?;
                mmr.push(b.digest())
                    .map_err(|e| InternalErrorKind::MMR.other(e))?;
                self.insert_ok_ext(&txn, &b.header().hash(), ext.clone(), None, None)?;
            }
        }

        if let Some(err) = found_error {
            Err(err)
        } else {
            trace!("light-client: commit");
            // Before commit, all new MMR nodes are in memory only.
            mmr.commit().map_err(|e| InternalErrorKind::MMR.other(e))?;
            Ok(())
        }
    }

    fn resolve_block_transactions<HC: HeaderChecker>(
        &self,
        txn: &StoreTransaction,
        block: &BlockView,
        verify_context: &HC,
    ) -> Result<Vec<Arc<ResolvedTransaction>>, Error> {
        let mut seen_inputs = HashSet::new();
        let block_cp = BlockCellProvider::new(block)?;
        let transactions = block.transactions();
        let cell_provider = OverlayCellProvider::new(&block_cp, txn);
        let resolved = transactions
            .iter()
            .cloned()
            .map(|tx| {
                resolve_transaction(tx, &mut seen_inputs, &cell_provider, verify_context)
                    .map(Arc::new)
            })
            .collect::<Result<Vec<Arc<ResolvedTransaction>>, _>>()?;
        Ok(resolved)
    }

    fn insert_ok_ext(
        &self,
        txn: &StoreTransaction,
        hash: &Byte32,
        mut ext: BlockExt,
        cache_entries: Option<&[Completed]>,
        txs_sizes: Option<Vec<u64>>,
    ) -> Result<(), Error> {
        ext.verified = Some(true);
        if let Some(entries) = cache_entries {
            let (txs_fees, cycles) = entries
                .iter()
                .map(|entry| (entry.fee, entry.cycles))
                .unzip();
            ext.txs_fees = txs_fees;
            ext.cycles = Some(cycles);
        }
        ext.txs_sizes = txs_sizes;
        txn.insert_block_ext(hash, &ext)
    }

    fn insert_failure_ext(
        &self,
        txn: &StoreTransaction,
        hash: &Byte32,
        mut ext: BlockExt,
    ) -> Result<(), Error> {
        ext.verified = Some(false);
        txn.insert_block_ext(hash, &ext)
    }

    fn monitor_block_txs_verified(
        &self,
        b: &BlockView,
        resolved: &[Arc<ResolvedTransaction>],
        cache_entries: &[Completed],
        cycles: Cycle,
    ) {
        info!(
            "[block_verifier] block number: {}, hash: {}, size:{}/{}, cycles: {}/{}",
            b.number(),
            b.hash(),
            b.data().serialized_size_without_uncle_proposals(),
            self.shared.consensus().max_block_bytes(),
            cycles,
            self.shared.consensus().max_block_cycles()
        );

        // log tx verification result for monitor node
        if log_enabled_target!("ckb_tx_monitor", Trace) {
            // `cache_entries` already excludes cellbase tx, but `resolved` includes cellbase tx, skip it
            // to make them aligned
            for (rtx, cycles) in resolved.iter().skip(1).zip(cache_entries.iter()) {
                trace_target!(
                    "ckb_tx_monitor",
                    r#"{{"tx_hash":"{:#x}","cycles":{}}}"#,
                    rtx.transaction.hash(),
                    cycles.cycles
                );
            }
        }
    }

    fn print_error(&self, b: &BlockView, err: &Error) {
        error!(
            "Block verify error. Block number: {}, hash: {}, error: {:?}",
            b.header().number(),
            b.header().hash(),
            err
        );
        if log_enabled!(ckb_logger::Level::Trace) {
            trace!("Block {}", b);
        }
    }

    // TODO: beatify
    fn print_chain(&self, len: u64) {
        debug!("Chain {{");

        let snapshot = self.shared.snapshot();
        let tip_header = snapshot.tip_header();
        let tip_number = tip_header.number();

        let bottom = tip_number - cmp::min(tip_number, len);

        for number in (bottom..=tip_number).rev() {
            let hash = snapshot
                .get_block_hash(number)
                .unwrap_or_else(|| panic!("invalid block number({number}), tip={tip_number}"));
            debug!("   {number} => {hash}");
        }

        debug!("}}");
    }
}

#[cfg(debug_assertions)]
fn is_sorted_assert(fork: &ForkChanges) {
    assert!(fork.is_sorted())
}

#[cfg(not(debug_assertions))]
fn is_sorted_assert(_fork: &ForkChanges) {}
