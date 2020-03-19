use crate::cell::{attach_block_cell, detach_block_cell};
use crate::switch::Switch;
use ckb_error::{Error, InternalErrorKind};
use ckb_logger::{self, debug, error, info, log_enabled, metric, trace, warn};
use ckb_proposal_table::ProposalTable;
use ckb_shared::shared::Shared;
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_store::{ChainStore, StoreTransaction};
use ckb_types::{
    core::{
        cell::{
            resolve_transaction, BlockCellProvider, CellProvider, CellStatus, OverlayCellProvider,
            ResolvedTransaction,
        },
        service::{Request, DEFAULT_CHANNEL_SIZE, SIGNAL_CHANNEL_SIZE},
        BlockExt, BlockNumber, BlockView, TransactionMeta,
    },
    packed::{Byte32, OutPoint, ProposalShortId},
    prelude::*,
    U256,
};
use ckb_verification::InvalidParentError;
use ckb_verification::{BlockVerifier, ContextualBlockVerifier, Verifier, VerifyContext};
use crossbeam_channel::{self, select, Receiver, Sender};
use faketime::unix_time_as_millis;
use im::hashmap::HashMap as HamtMap;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::{cmp, thread};

type ProcessBlockRequest = Request<(Arc<BlockView>, Switch), Result<bool, Error>>;

#[derive(Clone)]
pub struct ChainController {
    process_block_sender: Sender<ProcessBlockRequest>,
    stop: StopHandler<()>,
}

impl Drop for ChainController {
    fn drop(&mut self) {
        self.stop.try_send();
    }
}

impl ChainController {
    pub fn process_block(&self, block: Arc<BlockView>) -> Result<bool, Error> {
        self.internal_process_block(block, Switch::NONE)
    }

    pub fn internal_process_block(
        &self,
        block: Arc<BlockView>,
        switch: Switch,
    ) -> Result<bool, Error> {
        Request::call(&self.process_block_sender, (block, switch)).unwrap_or_else(|| {
            Err(InternalErrorKind::System
                .reason("Chain service has gone")
                .into())
        })
    }
}

struct ChainReceivers {
    process_block_receiver: Receiver<ProcessBlockRequest>,
}

#[derive(Debug, Default)]
pub struct ForkChanges {
    // blocks attached to index after forks
    pub(crate) attached_blocks: VecDeque<BlockView>,
    // blocks detached from index after forks
    pub(crate) detached_blocks: VecDeque<BlockView>,
    // proposal_id detached to index after forks
    pub(crate) detached_proposal_id: HashSet<ProposalShortId>,
    // to be updated exts
    pub(crate) dirty_exts: VecDeque<BlockExt>,
}

impl ForkChanges {
    pub fn attached_blocks(&self) -> &VecDeque<BlockView> {
        &self.attached_blocks
    }

    pub fn detached_blocks(&self) -> &VecDeque<BlockView> {
        &self.detached_blocks
    }

    pub fn detached_proposal_id(&self) -> &HashSet<ProposalShortId> {
        &self.detached_proposal_id
    }

    pub fn has_detached(&self) -> bool {
        !self.detached_blocks.is_empty()
    }

    pub fn verified_len(&self) -> usize {
        self.attached_blocks.len() - self.dirty_exts.len()
    }
}

struct CellSetWrapper<'a> {
    pub cell_set: &'a HamtMap<Byte32, TransactionMeta>,
    pub txn: &'a StoreTransaction,
}

impl<'a> CellSetWrapper<'a> {
    pub fn new(cell_set: &'a HamtMap<Byte32, TransactionMeta>, txn: &'a StoreTransaction) -> Self {
        CellSetWrapper { cell_set, txn }
    }
}

impl<'a> CellProvider for CellSetWrapper<'a> {
    fn cell(&self, out_point: &OutPoint, with_data: bool) -> CellStatus {
        let tx_hash = out_point.tx_hash();
        let index = out_point.index().unpack();
        match self.cell_set.get(&tx_hash) {
            Some(tx_meta) => match tx_meta.is_dead(index as usize) {
                Some(false) => {
                    let mut cell_meta = self
                        .txn
                        .get_cell_meta(&tx_hash, index)
                        .expect("store should be consistent with cell_set");
                    if with_data {
                        cell_meta.mem_cell_data = self.txn.get_cell_data(&tx_hash, index);
                    }
                    CellStatus::live_cell(cell_meta)
                }
                Some(true) => CellStatus::Dead,
                None => CellStatus::Unknown,
            },
            None => CellStatus::Unknown,
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

pub struct ChainService {
    shared: Shared,
    proposal_table: ProposalTable,
}

impl ChainService {
    pub fn new(shared: Shared, proposal_table: ProposalTable) -> ChainService {
        ChainService {
            shared,
            proposal_table,
        }
    }

    // remove `allow` tag when https://github.com/crossbeam-rs/crossbeam/issues/404 is solved
    #[allow(clippy::zero_ptr, clippy::drop_copy)]
    pub fn start<S: ToString>(mut self, thread_name: Option<S>) -> ChainController {
        let (signal_sender, signal_receiver) =
            crossbeam_channel::bounded::<()>(SIGNAL_CHANNEL_SIZE);
        let (process_block_sender, process_block_receiver) =
            crossbeam_channel::bounded(DEFAULT_CHANNEL_SIZE);

        // Mainly for test: give an empty thread_name
        let mut thread_builder = thread::Builder::new();
        if let Some(name) = thread_name {
            thread_builder = thread_builder.name(name.to_string());
        }

        let receivers = ChainReceivers {
            process_block_receiver,
        };
        let thread = thread_builder
            .spawn(move || loop {
                select! {
                    recv(signal_receiver) -> _ => {
                        break;
                    },
                    recv(receivers.process_block_receiver) -> msg => match msg {
                        Ok(Request { responder, arguments: (block, verify) }) => {
                            let _ = responder.send(self.process_block(block, verify));
                        },
                        _ => {
                            error!("process_block_receiver closed");
                            break;
                        },
                    }
                }
            })
            .expect("Start ChainService failed");
        let stop = StopHandler::new(SignalSender::Crossbeam(signal_sender), thread);

        ChainController {
            process_block_sender,
            stop,
        }
    }

    // process_block will do block verify
    // but invoker should guarantee block header be verified
    pub(crate) fn process_block(
        &mut self,
        block: Arc<BlockView>,
        switch: Switch,
    ) -> Result<bool, Error> {
        debug!("begin processing block: {}", block.header().hash());
        if block.header().number() < 1 {
            warn!(
                "receive 0 number block: {}-{}",
                block.header().number(),
                block.header().hash()
            );
        }
        self.insert_block(block, switch).map(|ret| {
            debug!("finish processing block");
            ret
        })
    }

    fn non_contextual_verify(&self, block: &BlockView) -> Result<(), Error> {
        let block_verifier = BlockVerifier::new(self.shared.consensus());
        block_verifier.verify(&block).map_err(|e| {
            debug!("[process_block] verification error {:?}", e);
            e
        })
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

        let parent_header_epoch = txn_snapshot
            .get_block_epoch(&parent_header.hash())
            .expect("parent epoch already store");

        let next_epoch_ext = txn_snapshot.next_epoch_ext(
            self.shared.consensus(),
            &parent_header_epoch,
            &parent_header,
        );
        let new_epoch = next_epoch_ext.is_some();

        let epoch = next_epoch_ext.unwrap_or_else(|| parent_header_epoch.to_owned());

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
        db_txn.insert_epoch_ext(&epoch.last_block_hash_in_previous_epoch(), &epoch)?;

        let shared_snapshot = Arc::clone(&self.shared.snapshot());
        let mut cell_set = shared_snapshot.cell_set().clone();
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
            if !fork.detached_blocks.is_empty() {
                metric!({
                    "topic": "reorg",
                    "tags": {},
                    "fields": { "attached": fork.attached_blocks.len(), "detached": fork.detached_blocks.len(), },
                });
            }

            self.rollback(&fork, &db_txn, &mut cell_set)?;
            // update and verify chain root
            // MUST update index before reconcile_main_chain
            self.reconcile_main_chain(&db_txn, &mut fork, switch, &mut cell_set)?;

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

            let new_snapshot = self.shared.new_snapshot(
                tip_header,
                total_difficulty,
                epoch,
                cell_set,
                new_proposals,
            );

            self.shared.store_snapshot(Arc::clone(&new_snapshot));

            if let Err(e) = self.shared.tx_pool_controller().update_tx_pool_for_reorg(
                fork.detached_blocks().clone(),
                fork.attached_blocks().clone(),
                fork.detached_proposal_id().clone(),
                new_snapshot,
            ) {
                error!("notify update_tx_pool_for_reorg error {}", e);
            }
            for detached_block in fork.detached_blocks() {
                if let Err(e) = self
                    .shared
                    .tx_pool_controller()
                    .notify_new_uncle(detached_block.as_uncle())
                {
                    error!("notify new_uncle error {}", e);
                }
            }
            let block_ref: &BlockView = &block;
            self.shared
                .notify_controller()
                .notify_new_block(block_ref.clone());
            if log_enabled!(ckb_logger::Level::Debug) {
                self.print_chain(10);
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
            let block_ref: &BlockView = &block;
            if let Err(e) = self
                .shared
                .tx_pool_controller()
                .notify_new_uncle(block_ref.as_uncle())
            {
                error!("notify new_uncle error {}", e);
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
    }

    pub(crate) fn rollback(
        &self,
        fork: &ForkChanges,
        txn: &StoreTransaction,
        cell_set: &mut HamtMap<Byte32, TransactionMeta>,
    ) -> Result<(), Error> {
        for block in fork.detached_blocks().iter().rev() {
            txn.detach_block(block)?;
            detach_block_cell(txn, block, cell_set)?;
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
                fork.detached_blocks.push_front(old_block);
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
    }

    // we found new best_block
    pub(crate) fn reconcile_main_chain(
        &self,
        txn: &StoreTransaction,
        fork: &mut ForkChanges,
        switch: Switch,
        cell_set: &mut HamtMap<Byte32, TransactionMeta>,
    ) -> Result<(), Error> {
        let txs_verify_cache = self.shared.txs_verify_cache();

        let verified_len = fork.verified_len();
        for b in fork.attached_blocks().iter().take(verified_len) {
            txn.attach_block(b)?;
            attach_block_cell(txn, b, cell_set)?;
        }

        let verify_context = VerifyContext::new(txn, self.shared.consensus());
        let future_executor = self.shared.tx_pool_controller().executor();

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
                        let wrapper = CellSetWrapper::new(cell_set, txn);
                        let cell_provider = OverlayCellProvider::new(&block_cp, &wrapper);
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
                                txs_verify_cache.clone(),
                                &future_executor,
                                switch,
                            ) {
                                Ok((cycles, cache_entries)) => {
                                    let txs_fees = cache_entries
                                        .into_iter()
                                        .skip(1)
                                        .map(|entry| entry.fee)
                                        .collect();
                                    txn.attach_block(b)?;
                                    attach_block_cell(txn, b, cell_set)?;
                                    let mut mut_ext = ext.clone();
                                    mut_ext.verified = Some(true);
                                    mut_ext.txs_fees = txs_fees;
                                    txn.insert_block_ext(&b.header().hash(), &mut_ext)?;
                                    if b.transactions().len() > 1 {
                                        info!(
                                            "[block_verifier] block number: {}, hash: {}, size:{}/{}, cycles: {}/{}",
                                            b.number(),
                                            b.hash(),
                                            b.data().serialized_size_without_uncle_proposals(),
                                            self.shared.consensus().max_block_bytes(),
                                            cycles,
                                            self.shared.consensus().max_block_cycles()
                                        );
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
                            found_error = Some(err);
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
                attach_block_cell(txn, b, cell_set)?;
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
            let hash = snapshot.get_block_hash(number).unwrap_or_else(|| {
                panic!(format!(
                    "invaild block number({}), tip={}",
                    number, tip_number
                ))
            });
            debug!("   {} => {}", number, hash);
        }

        debug!("}}");
    }
}
