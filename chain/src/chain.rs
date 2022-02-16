//! CKB chain service.
#![allow(missing_docs)]

use ckb_channel::{self as channel, select, Sender};
use ckb_error::{Error, InternalErrorKind};
use ckb_logger::Level::Trace;
use ckb_logger::{
    self, debug, error, info, log_enabled, log_enabled_target, trace, trace_target, warn,
};
use ckb_merkle_mountain_range::leaf_index_to_mmr_size;
use ckb_metrics::{metrics, Timer};
use ckb_proposal_table::ProposalTable;
#[cfg(debug_assertions)]
use ckb_rust_unstable_port::IsSorted;
use ckb_shared::shared::Shared;
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_store::{attach_block_cell, detach_block_cell, ChainStore, StoreTransaction};
use ckb_types::{
    core::{
        cell::{resolve_transaction, BlockCellProvider, OverlayCellProvider, ResolvedTransaction},
        service::{Request, DEFAULT_CHANNEL_SIZE, SIGNAL_CHANNEL_SIZE},
        BlockExt, BlockNumber, BlockView, HeaderView,
    },
    packed::{Byte32, ProposalShortId},
    prelude::*,
    utilities::merkle_mountain_range::ChainRootMMR,
    U256,
};
use ckb_verification::{
    BlockErrorKind, BlockVerifier, InvalidParentError, NonContextualBlockTxsVerifier,
};
use ckb_verification_contextual::{ContextualBlockVerifier, VerifyContext};
use ckb_verification_traits::{Switch, Verifier};
use faketime::unix_time_as_millis;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::{cmp, thread};

type ProcessBlockRequest = Request<(Arc<BlockView>, Switch), Result<bool, Error>>;
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
    truncate_sender: Sender<TruncateRequest>, // Used for testing only
    stop: Option<StopHandler<()>>,
}

impl Drop for ChainController {
    fn drop(&mut self) {
        self.try_stop();
    }
}

#[cfg_attr(feature = "mock", faux::methods)]
impl ChainController {
    pub fn new(
        process_block_sender: Sender<ProcessBlockRequest>,
        truncate_sender: Sender<TruncateRequest>,
        stop: StopHandler<()>,
    ) -> Self {
        ChainController {
            process_block_sender,
            truncate_sender,
            stop: Some(stop),
        }
    }
    /// Inserts the block into database.
    ///
    /// Expects the block's header to be valid and already verified.
    ///
    /// If the block already exists, does nothing and false is returned.
    ///
    /// [BlockVerifier] [NonContextualBlockTxsVerifier] [ContextualBlockVerifier] will performed
    pub fn process_block(&self, block: Arc<BlockView>) -> Result<bool, Error> {
        self.internal_process_block(block, Switch::NONE)
    }

    /// Internal method insert block for test
    ///
    /// switch bit flags for particular verify, make easier to generating test data
    pub fn internal_process_block(
        &self,
        block: Arc<BlockView>,
        switch: Switch,
    ) -> Result<bool, Error> {
        Request::call(&self.process_block_sender, (block, switch)).unwrap_or_else(|| {
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

    pub fn try_stop(&mut self) {
        if let Some(ref mut stop) = self.stop {
            stop.try_send(());
        }
    }

    /// Since a non-owning reference does not count towards ownership,
    /// it will not prevent the value stored in the allocation from being dropped
    pub fn non_owning_clone(&self) -> Self {
        ChainController {
            stop: None,
            truncate_sender: self.truncate_sender.clone(),
            process_block_sender: self.process_block_sender.clone(),
        }
    }
}

/// The struct represent fork
#[derive(Debug, Default)]
pub struct ForkChanges {
    /// Blocks attached to index after forks
    pub(crate) attached_blocks: VecDeque<BlockView>,
    /// Blocks detached from index after forks
    pub(crate) detached_blocks: VecDeque<BlockView>,
    /// HashSet with proposal_id detached to index after forks
    pub(crate) detached_proposal_id: HashSet<ProposalShortId>,
    /// to be updated exts
    pub(crate) dirty_exts: VecDeque<BlockExt>,
}

impl ForkChanges {
    /// blocks attached to index after forks
    pub fn attached_blocks(&self) -> &VecDeque<BlockView> {
        &self.attached_blocks
    }

    /// blocks detached from index after forks
    pub fn detached_blocks(&self) -> &VecDeque<BlockView> {
        &self.detached_blocks
    }

    /// proposal_id detached to index after forks
    pub fn detached_proposal_id(&self) -> &HashSet<ProposalShortId> {
        &self.detached_proposal_id
    }

    /// are there any block should be detached
    pub fn has_detached(&self) -> bool {
        !self.detached_blocks.is_empty()
    }

    /// cached verified attached block num
    pub fn verified_len(&self) -> usize {
        self.attached_blocks.len() - self.dirty_exts.len()
    }

    /// assertion for make sure attached_blocks and detached_blocks are sorted
    #[cfg(debug_assertions)]
    pub fn is_sorted(&self) -> bool {
        IsSorted::is_sorted_by_key(&mut self.attached_blocks().iter(), |blk| {
            blk.header().number()
        }) && IsSorted::is_sorted_by_key(&mut self.detached_blocks().iter(), |blk| {
            blk.header().number()
        })
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
pub struct ChainService {
    shared: Shared,
    proposal_table: ProposalTable,
}

impl ChainService {
    /// Create a new ChainService instance with shared and initial proposal_table.
    pub fn new(shared: Shared, proposal_table: ProposalTable) -> ChainService {
        ChainService {
            shared,
            proposal_table,
        }
    }

    /// start background single-threaded service with specified thread_name.
    pub fn start<S: ToString>(mut self, thread_name: Option<S>) -> ChainController {
        let (signal_sender, signal_receiver) = channel::bounded::<()>(SIGNAL_CHANNEL_SIZE);
        let (process_block_sender, process_block_receiver) = channel::bounded(DEFAULT_CHANNEL_SIZE);
        let (truncate_sender, truncate_receiver) = channel::bounded(1);

        // Mainly for test: give an empty thread_name
        let mut thread_builder = thread::Builder::new();
        if let Some(name) = thread_name {
            thread_builder = thread_builder.name(name.to_string());
        }
        let tx_control = self.shared.tx_pool_controller().clone();

        let thread = thread_builder
            .spawn(move || loop {
                select! {
                    recv(signal_receiver) -> _ => {
                        break;
                    },
                    recv(process_block_receiver) -> msg => match msg {
                        Ok(Request { responder, arguments: (block, verify) }) => {
                            let _ = tx_control.suspend_chunk_process();
                            let _ = responder.send(self.process_block(block, verify));
                            let _ = tx_control.continue_chunk_process();
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
                    }
                }
            })
            .expect("Start ChainService failed");
        let stop = StopHandler::new(
            SignalSender::Crossbeam(signal_sender),
            Some(thread),
            "chain".to_string(),
        );

        ChainController::new(process_block_sender, truncate_sender, stop)
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

        debug!("begin processing block: {}-{}", block_number, block_hash);
        if block_number < 1 {
            warn!("receive 0 number block: 0-{}", block_hash);
        }

        self.insert_block(block, switch).map(|ret| {
            debug!("finish processing block");
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

    fn insert_block(&mut self, block: Arc<BlockView>, switch: Switch) -> Result<bool, Error> {
        let db_txn = self.shared.store().begin_transaction();
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
            .next_epoch_ext(&parent_header, &txn_snapshot.as_data_provider())
            .expect("epoch should be stored");
        let new_epoch = next_block_epoch.is_head();
        let epoch = next_block_epoch.epoch();

        let ext = BlockExt {
            received_at: unix_time_as_millis(),
            total_difficulty: cannon_total_difficulty.clone(),
            total_uncles_count: parent_ext.total_uncles_count + block.data().uncles().len() as u64,
            verified: None,
            txs_fees: vec![],
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
            "difficulty current = {:#x}, cannon = {:#x}",
            current_total_difficulty, cannon_total_difficulty,
        );

        // is_better_than
        let new_best_block = cannon_total_difficulty > current_total_difficulty;

        if new_best_block {
            debug!(
                "new best block found: {} => {:#x}, difficulty diff = {:#x}",
                block.header().number(),
                block.header().hash(),
                &cannon_total_difficulty - &current_total_difficulty
            );
            self.find_fork(&mut fork, current_tip_header.number(), &block, ext);
            self.rollback(&fork, &db_txn)?;

            self.update_chain_root_mmr_after_check(&fork, &db_txn)?;

            // update and verify chain root
            // MUST update index before reconcile_main_chain
            self.reconcile_main_chain(&db_txn, &mut fork, switch)?;

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
                    error!("notify update_tx_pool_for_reorg error {}", e);
                }
            }

            let block_ref: &BlockView = &block;
            self.shared
                .notify_controller()
                .notify_new_block(block_ref.clone());
            if log_enabled!(ckb_logger::Level::Debug) {
                self.print_chain(10);
            }
            metrics!(gauge, "ckb.chain_tip", block.header().number() as i64);
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
                    error!("notify new_uncle error {}", e);
                }
            }
        }

        Ok(true)
    }

    pub(crate) fn update_proposal_table(&mut self, fork: &ForkChanges) {
        for blk in fork.detached_blocks() {
            self.proposal_table.remove(blk.header().number());
        }
        for blk in fork.attached_blocks() {
            self.proposal_table
                .insert(blk.header().number(), blk.union_proposal_ids());
        }

        self.reload_proposal_table(fork);
    }

    // if rollback happen, go back check whether need reload proposal_table from block
    pub(crate) fn reload_proposal_table(&mut self, fork: &ForkChanges) {
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

            debug!("reload_proposal_table [{}, {}]", proposal_start, common);
            for bn in proposal_start..=common {
                let blk = self
                    .shared
                    .store()
                    .get_block_hash(bn)
                    .and_then(|hash| self.shared.store().get_block(&hash))
                    .expect("block stored");

                self.proposal_table.insert(bn, blk.union_proposal_ids());
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

    pub(crate) fn update_chain_root_mmr_after_check<'a>(
        &'a self,
        fork: &ForkChanges,
        txn: &StoreTransaction,
    ) -> Result<(), Error> {
        let attached_blocks = fork.attached_blocks();
        if attached_blocks.is_empty() {
            return Ok(());
        }

        let consensus = self.shared.consensus();
        let mmr_activated_number = consensus.mmr_activated_number();
        let mut activated_attached_blocks = attached_blocks
            .iter()
            .skip_while(|b| b.number() < mmr_activated_number)
            .collect::<Vec<_>>();

        if activated_attached_blocks.is_empty() {
            return Ok(());
        }

        let start_block_header = activated_attached_blocks[0].header();
        let mut mmr = if start_block_header.number() == mmr_activated_number {
            trace!("light-client: initialize the chain root MMR");
            let mut mmr = ChainRootMMR::new(0, txn);
            mmr.push(start_block_header.digest())
                .map_err(|e| InternalErrorKind::MMR.other(e))?;

            activated_attached_blocks.remove(0);

            mmr
        } else {
            let mmr_size =
                leaf_index_to_mmr_size(start_block_header.number() - mmr_activated_number - 1);
            trace!("light-client: new chain root MMR with size = {}", mmr_size);
            ChainRootMMR::new(mmr_size, txn)
        };

        for block in &activated_attached_blocks {
            trace!("light-client: attach block#{}", block.number());
            let chain_root = mmr
                .get_root()
                .map_err(|e| InternalErrorKind::MMR.other(e))?;
            let actual_root_hash = chain_root.calc_mmr_hash();
            let extension = block
                .extension()
                .expect("should be checked in BlockVerifier/BlockExtensionVerifier");
            let expected_root_hash = Byte32::new_unchecked(extension.raw_data().slice(..32));
            if actual_root_hash != expected_root_hash {
                return Err(BlockErrorKind::InvalidChainRoot.into());
            }
            mmr.push(block.digest())
                .map_err(|e| InternalErrorKind::MMR.other(e))?;
        }

        trace!("light-client: commit");
        // Before commit, all new MMR nodes are in memory only.
        mmr.commit().map_err(|e| InternalErrorKind::MMR.other(e))?;
        Ok(())
    }

    // we found new best_block
    pub(crate) fn reconcile_main_chain(
        &self,
        txn: &StoreTransaction,
        fork: &mut ForkChanges,
        switch: Switch,
    ) -> Result<(), Error> {
        let txs_verify_cache = self.shared.txs_verify_cache();
        let consensus = self.shared.consensus();
        let async_handle = self.shared.tx_pool_controller().handle();

        let verified_len = fork.verified_len();
        for b in fork.attached_blocks().iter().take(verified_len) {
            txn.attach_block(b)?;
            attach_block_cell(txn, b)?;
        }

        let verify_context = VerifyContext::new(txn, consensus);

        let mut found_error = None;
        for (ext, b) in fork
            .dirty_exts
            .iter()
            .zip(fork.attached_blocks.iter().skip(verified_len))
        {
            if !switch.disable_all() {
                if found_error.is_none() {
                    let contextual_block_verifier = ContextualBlockVerifier::new(&verify_context);
                    let mut seen_inputs = HashSet::new();
                    let block_cp = match BlockCellProvider::new(b) {
                        Ok(block_cp) => block_cp,
                        Err(err) => {
                            found_error = Some(err);
                            continue;
                        }
                    };

                    let transactions = b.transactions();
                    let resolved = {
                        let cell_provider = OverlayCellProvider::new(&block_cp, txn);
                        transactions
                            .iter()
                            .cloned()
                            .map(|x| {
                                resolve_transaction(
                                    x,
                                    &mut seen_inputs,
                                    &cell_provider,
                                    &verify_context,
                                )
                            })
                            .collect::<Result<Vec<ResolvedTransaction>, _>>()
                    };

                    match resolved {
                        Ok(resolved) => {
                            match contextual_block_verifier.verify(
                                &resolved,
                                b,
                                Arc::clone(&txs_verify_cache),
                                async_handle,
                                switch,
                            ) {
                                Ok((cycles, cache_entries)) => {
                                    let txs_fees =
                                        cache_entries.iter().map(|entry| entry.fee).collect();
                                    txn.attach_block(b)?;
                                    attach_block_cell(txn, b)?;
                                    let mut mut_ext = ext.clone();
                                    mut_ext.verified = Some(true);
                                    mut_ext.txs_fees = txs_fees;
                                    txn.insert_block_ext(&b.header().hash(), &mut_ext)?;
                                    if !switch.disable_script() && b.transactions().len() > 1 {
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
                                            for (rtx, cycles) in
                                                resolved.iter().skip(1).zip(cache_entries.iter())
                                            {
                                                trace_target!(
                                                    "ckb_tx_monitor",
                                                    r#"{{"tx_hash":"{:#x}","cycles":{}}}"#,
                                                    rtx.transaction.hash(),
                                                    cycles.cycles
                                                );
                                            }
                                        }
                                    }
                                }
                                Err(err) => {
                                    error!("block verify error, block number: {}, hash: {}, error: {:?}", b.header().number(),
                                            b.header().hash(), err);
                                    if log_enabled!(ckb_logger::Level::Trace) {
                                        trace!("block {}", b.data());
                                    }
                                    found_error = Some(err);
                                    let mut mut_ext = ext.clone();
                                    mut_ext.verified = Some(false);
                                    txn.insert_block_ext(&b.header().hash(), &mut_ext)?;
                                }
                            }
                        }
                        Err(err) => {
                            found_error = Some(err.into());
                            let mut mut_ext = ext.clone();
                            mut_ext.verified = Some(false);
                            txn.insert_block_ext(&b.header().hash(), &mut_ext)?;
                        }
                    }
                } else {
                    let mut mut_ext = ext.clone();
                    mut_ext.verified = Some(false);
                    txn.insert_block_ext(&b.header().hash(), &mut_ext)?;
                }
            } else {
                txn.attach_block(b)?;
                attach_block_cell(txn, b)?;
                let mut mut_ext = ext.clone();
                mut_ext.verified = Some(true);
                txn.insert_block_ext(&b.header().hash(), &mut_ext)?;
            }
        }

        if let Some(err) = found_error {
            Err(err)
        } else {
            Ok(())
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
                .unwrap_or_else(|| panic!("invalid block number({}), tip={}", number, tip_number));
            debug!("   {} => {}", number, hash);
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
