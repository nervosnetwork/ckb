//! CKB chain service.
#![allow(missing_docs)]

use ckb_channel::{self as channel, select, Sender};
use ckb_error::{Error, InternalErrorKind};
use ckb_logger::Level::Trace;
use ckb_logger::{
    self, debug, error, info, log_enabled, log_enabled_target, trace, trace_target, warn,
};
use ckb_merkle_mountain_range::leaf_index_to_mmr_size;
use ckb_proposal_table::ProposalTable;
use ckb_shared::shared::Shared;
use ckb_stop_handler::{new_crossbeam_exit_rx, register_thread};
use ckb_store::{attach_block_cell, detach_block_cell, ChainStore, StoreTransaction};
use ckb_systemtime::unix_time_as_millis;
use ckb_types::{
    core::{
        cell::{
            resolve_transaction, BlockCellProvider, HeaderChecker, OverlayCellProvider,
            ResolvedTransaction,
        },
        hardfork::HardForks,
        service::Request,
        BlockExt, BlockNumber, BlockView, Cycle, HeaderView,
    },
    packed::{Byte32, ProposalShortId},
    utilities::merkle_mountain_range::ChainRootMMR,
    BlockNumberAndHash, U256,
};
use ckb_verification::cache::Completed;
use ckb_verification::{BlockVerifier, InvalidParentError, NonContextualBlockTxsVerifier};
use ckb_verification_contextual::{ContextualBlockVerifier, VerifyContext};
use ckb_verification_traits::{Switch, Verifier};
#[cfg(debug_assertions)]
use is_sorted::IsSorted;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Instant;
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
}

#[cfg_attr(feature = "mock", faux::methods)]
impl ChainController {
    pub fn new(
        process_block_sender: Sender<ProcessBlockRequest>,
        truncate_sender: Sender<TruncateRequest>,
    ) -> Self {
        ChainController {
            process_block_sender,
            truncate_sender,
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

    pub fn during_hardfork(&self, hardfork_switch: &HardForks) -> bool {
        let hardfork_during_detach =
            self.check_if_hardfork_during_blocks(hardfork_switch, &self.detached_blocks);
        let hardfork_during_attach =
            self.check_if_hardfork_during_blocks(hardfork_switch, &self.attached_blocks);

        hardfork_during_detach || hardfork_during_attach
    }

    fn check_if_hardfork_during_blocks(
        &self,
        hardfork: &HardForks,
        blocks: &VecDeque<BlockView>,
    ) -> bool {
        if blocks.is_empty() {
            false
        } else {
            // This method assumes that the input blocks are sorted and unique.
            let rfc_0049 = hardfork.ckb2023.rfc_0049();
            let epoch_first = blocks.front().unwrap().epoch().number();
            let epoch_next = blocks
                .back()
                .unwrap()
                .epoch()
                .minimum_epoch_number_after_n_blocks(1);
            epoch_first < rfc_0049 && rfc_0049 <= epoch_next
        }
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
        let signal_receiver = new_crossbeam_exit_rx();
        let (process_block_sender, process_block_receiver) = channel::bounded(0);
        let (truncate_sender, truncate_receiver) = channel::bounded(0);

        // Mainly for test: give an empty thread_name
        let mut thread_builder = thread::Builder::new();
        if let Some(name) = thread_name {
            thread_builder = thread_builder.name(name.to_string());
        }
        let tx_control = self.shared.tx_pool_controller().clone();

        let chain_jh = thread_builder
            .spawn(move || loop {
                select! {
                    recv(process_block_receiver) -> msg => match msg {
                        Ok(Request { responder, arguments: (block, verify) }) => {
                            let instant = Instant::now();

                            let _ = tx_control.suspend_chunk_process();
                            let _ = responder.send(self.process_block(block, verify));
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
                        break;
                    }
                }
            })
            .expect("Start ChainService failed");

        register_thread("ChainService", chain_jh);

        ChainController::new(process_block_sender, truncate_sender)
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
    // Use for testing only, can only truncate less than 50000 blocks each time
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

        let block_count = snapshot
            .tip_header()
            .number()
            .saturating_sub(target_tip_header.number());

        if block_count > 5_0000 {
            let err = format!(
                "trying to truncate too many blocks: {}, exceed 50000",
                block_count
            );
            return Err(InternalErrorKind::Database.other(err).into());
        }
        let mut fork = self.make_fork_for_truncate(&target_tip_header, snapshot.tip_header());

        let db_txn = self.shared.store().begin_transaction();
        self.rollback(&fork, &db_txn)?;

        db_txn.insert_tip_header(&target_tip_header)?;
        db_txn.insert_current_epoch_ext(&target_epoch_ext)?;

        // Currently, we only move the target tip header here, we don't delete the block for performance
        // TODO: delete the blocks if we need in the future

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
            db_txn.insert_block_ext(block.header().num_hash(), &ext)?;
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

            debug!("Reload_proposal_table [{}, {}]", proposal_start, common);
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
                    let resolved = self.resolve_block_transactions(&txn, b, &verify_context);
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
                                contextual_block_verifier.verify(&resolved, b)
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
                                        b.header().num_hash(),
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
                                    self.insert_failure_ext(
                                        &txn,
                                        b.header().num_hash(),
                                        ext.clone(),
                                    )?;
                                }
                            }
                        }
                        Err(err) => {
                            found_error = Some(err);
                            self.insert_failure_ext(&txn, b.header().num_hash(), ext.clone())?;
                        }
                    }
                } else {
                    self.insert_failure_ext(&txn, b.header().num_hash(), ext.clone())?;
                }
            } else {
                txn.attach_block(b)?;
                attach_block_cell(&txn, b)?;
                mmr.push(b.digest())
                    .map_err(|e| InternalErrorKind::MMR.other(e))?;
                self.insert_ok_ext(&txn, b.header().num_hash(), ext.clone(), None, None)?;
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
        num_hash: BlockNumberAndHash,
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
        txn.insert_block_ext(num_hash, &ext)
    }

    fn insert_failure_ext(
        &self,
        txn: &StoreTransaction,
        num_hash: BlockNumberAndHash,
        mut ext: BlockExt,
    ) -> Result<(), Error> {
        ext.verified = Some(false);
        txn.insert_block_ext(num_hash, &ext)
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
            trace!("Block {}", b.data());
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
