use crate::{
    tell_synchronizer_to_punish_the_bad_peer, utils::forkchanges::ForkChanges, GlobalIndex,
    LonelyBlock, LonelyBlockWithCallback, UnverifiedBlock, VerifiedBlockStatus, VerifyResult,
};
use ckb_channel::{select, Receiver};
use ckb_error::{Error, InternalErrorKind};
use ckb_logger::internal::{log_enabled, trace};
use ckb_logger::Level::Trace;
use ckb_logger::{debug, error, info, log_enabled_target, trace_target};
use ckb_merkle_mountain_range::leaf_index_to_mmr_size;
use ckb_proposal_table::ProposalTable;
use ckb_shared::block_status::BlockStatus;
use ckb_shared::types::VerifyFailedBlockInfo;
use ckb_shared::Shared;
use ckb_store::{attach_block_cell, detach_block_cell, ChainStore, StoreTransaction};
use ckb_systemtime::unix_time_as_millis;
use ckb_types::core::cell::{
    resolve_transaction, BlockCellProvider, HeaderChecker, OverlayCellProvider, ResolvedTransaction,
};
use ckb_types::core::{BlockExt, BlockNumber, BlockView, Cycle, HeaderView};
use ckb_types::packed::Byte32;
use ckb_types::utilities::merkle_mountain_range::ChainRootMMR;
use ckb_types::H256;
use ckb_verification::cache::Completed;
use ckb_verification::InvalidParentError;
use ckb_verification_contextual::{ContextualBlockVerifier, VerifyContext};
use ckb_verification_traits::Switch;
use std::cmp;
use std::collections::HashSet;
use std::sync::Arc;

pub(crate) struct ConsumeUnverifiedBlockProcessor {
    pub(crate) shared: Shared,
    pub(crate) proposal_table: ProposalTable,
    pub(crate) verify_failed_blocks_tx: tokio::sync::mpsc::UnboundedSender<VerifyFailedBlockInfo>,
}

pub(crate) struct ConsumeUnverifiedBlocks {
    unverified_block_rx: Receiver<UnverifiedBlock>,
    stop_rx: Receiver<()>,
    processor: ConsumeUnverifiedBlockProcessor,
}

impl ConsumeUnverifiedBlocks {
    pub(crate) fn new(
        shared: Shared,
        unverified_blocks_rx: Receiver<UnverifiedBlock>,
        proposal_table: ProposalTable,
        verify_failed_blocks_tx: tokio::sync::mpsc::UnboundedSender<VerifyFailedBlockInfo>,
        stop_rx: Receiver<()>,
    ) -> Self {
        ConsumeUnverifiedBlocks {
            unverified_block_rx: unverified_blocks_rx,
            stop_rx,
            processor: ConsumeUnverifiedBlockProcessor {
                shared,
                proposal_table,
                verify_failed_blocks_tx,
            },
        }
    }
    pub(crate) fn start(mut self) {
        let mut begin_loop = std::time::Instant::now();
        loop {
            begin_loop = std::time::Instant::now();
            select! {
                recv(self.unverified_block_rx) -> msg => match msg {
                    Ok(unverified_task) => {
                        // process this unverified block
                        trace!("got an unverified block, wait cost: {:?}", begin_loop.elapsed());
                        self.processor.consume_unverified_blocks(unverified_task);
                        trace!("consume_unverified_blocks cost: {:?}", begin_loop.elapsed());
                    },
                    Err(err) => {
                        error!("unverified_block_rx err: {}", err);
                        return;
                    },
                },
                recv(self.stop_rx) -> _ => {
                    info!("consume_unverified_blocks thread received exit signal, exit now");
                    break;
                }

            }
        }
    }
}

impl ConsumeUnverifiedBlockProcessor {
    pub(crate) fn consume_unverified_blocks(&mut self, unverified_block: UnverifiedBlock) {
        // process this unverified block
        let verify_result = self.verify_block(&unverified_block);
        match &verify_result {
            Ok(_) => {
                let log_now = std::time::Instant::now();
                self.shared
                    .remove_block_status(&unverified_block.block().hash());
                let log_elapsed_remove_block_status = log_now.elapsed();
                self.shared
                    .remove_header_view(&unverified_block.block().hash());
                debug!(
                    "block {} remove_block_status cost: {:?}, and header_view cost: {:?}",
                    unverified_block.block().hash(),
                    log_elapsed_remove_block_status,
                    log_now.elapsed()
                );
            }
            Err(err) => {
                error!(
                    "verify [{:?}]'s block {} failed: {}",
                    unverified_block.peer_id(),
                    unverified_block.block().hash(),
                    err
                );

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

                self.shared.insert_block_status(
                    unverified_block.block().hash(),
                    BlockStatus::BLOCK_INVALID,
                );
                error!(
                    "set_unverified tip to {}-{}, because verify {} failed: {}",
                    tip.number(),
                    tip.hash(),
                    unverified_block.block().hash(),
                    err
                );

                tell_synchronizer_to_punish_the_bad_peer(
                    self.verify_failed_blocks_tx.clone(),
                    unverified_block.peer_id(),
                    unverified_block.block().hash(),
                    err,
                );
            }
        }

        unverified_block.execute_callback(verify_result);
    }

    fn verify_block(&mut self, unverified_block: &UnverifiedBlock) -> VerifyResult {
        let UnverifiedBlock {
            unverified_block:
                LonelyBlockWithCallback {
                    lonely_block:
                        LonelyBlock {
                            block,
                            peer_id: _peer_id,
                            switch,
                        },
                    verify_callback: _verify_callback,
                },
            parent_header,
        } = unverified_block;

        let switch: Switch = switch.unwrap_or_else(|| {
            let mut assume_valid_target = self.shared.assume_valid_target();
            match *assume_valid_target {
                Some(ref target) => {
                    // if the target has been reached, delete it
                    if target
                        == &ckb_types::prelude::Unpack::<H256>::unpack(&BlockView::hash(&block))
                    {
                        assume_valid_target.take();
                        Switch::NONE
                    } else {
                        Switch::DISABLE_SCRIPT
                    }
                }
                None => Switch::NONE,
            }
        });

        let parent_ext = self
            .shared
            .store()
            .get_block_ext(&block.data().header().raw().parent_hash())
            .expect("parent should be stored already");

        if let Some(ext) = self.shared.store().get_block_ext(&block.hash()) {
            match ext.verified {
                Some(verified) => {
                    debug!(
                        "block {}-{} has been verified, previously verified result: {}",
                        block.number(),
                        block.hash(),
                        verified
                    );
                    return if verified {
                        Ok(VerifiedBlockStatus::PreviouslySeenAndVerified)
                    } else {
                        Err(InternalErrorKind::Other
                            .other("block previously verified failed")
                            .into())
                    };
                }
                _ => {
                    // we didn't verify this block, going on verify now
                }
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
            self.reconcile_main_chain(Arc::clone(&db_txn), &mut fork, switch)?;
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

            Ok(VerifiedBlockStatus::FirstSeenAndVerified)
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
            Ok(VerifiedBlockStatus::UncleBlockNotVerified)
        }
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
            "block verify error, block number: {}, hash: {}, error: {:?}",
            b.header().number(),
            b.header().hash(),
            err
        );
        if log_enabled!(ckb_logger::Level::Trace) {
            trace!("block {}", b);
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
    pub(crate) fn truncate(
        &mut self,
        proposal_table: &mut ProposalTable,
        target_tip_hash: &Byte32,
    ) -> Result<(), Error> {
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
        let (detached_proposal_id, new_proposals) =
            proposal_table.finalize(origin_proposals, target_tip_header.number());
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
}

#[cfg(debug_assertions)]
fn is_sorted_assert(fork: &ForkChanges) {
    assert!(fork.is_sorted())
}

#[cfg(not(debug_assertions))]
fn is_sorted_assert(_fork: &ForkChanges) {}
