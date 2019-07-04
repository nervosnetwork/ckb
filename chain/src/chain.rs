use ckb_core::block::Block;
use ckb_core::cell::{
    resolve_transaction, BlockCellProvider, BlockHeadersProvider, OverlayCellProvider,
    OverlayHeaderProvider, ResolvedTransaction,
};
use ckb_core::extras::BlockExt;
use ckb_core::service::{Request, DEFAULT_CHANNEL_SIZE, SIGNAL_CHANNEL_SIZE};
use ckb_core::transaction::{CellOutput, ProposalShortId};
use ckb_core::{BlockNumber, Cycle};
use ckb_logger::{self, debug, error, info, log_enabled, warn};
use ckb_notify::NotifyController;
use ckb_shared::cell_set::CellSetDiff;
use ckb_shared::chain_state::ChainState;
use ckb_shared::error::SharedError;
use ckb_shared::shared::Shared;
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_store::{ChainStore, StoreBatch};
use ckb_traits::ChainProvider;
use ckb_verification::{BlockVerifier, ContextualBlockVerifier, ForkContext, Verifier};
use crossbeam_channel::{self, select, Receiver, Sender};
use failure::Error as FailureError;
use faketime::unix_time_as_millis;
use fnv::{FnvHashMap, FnvHashSet};
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use serde_derive::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use std::{cmp, thread};

type ProcessBlockRequest = Request<(Arc<Block>, bool), Result<(), FailureError>>;

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
    pub fn process_block(&self, block: Arc<Block>, need_verify: bool) -> Result<(), FailureError> {
        Request::call(&self.process_block_sender, (block, need_verify))
            .expect("process_block() failed")
    }
}

struct ChainReceivers {
    process_block_receiver: Receiver<ProcessBlockRequest>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ForkChanges {
    // blocks attached to index after forks
    pub(crate) attached_blocks: VecDeque<Block>,
    // blocks detached from index after forks
    pub(crate) detached_blocks: VecDeque<Block>,
    // proposal_id detached to index after forks
    pub(crate) detached_proposal_id: FnvHashSet<ProposalShortId>,
    // to be updated exts
    pub(crate) dirty_exts: VecDeque<BlockExt>,
}

type VerifyContext<'a, CS> = (
    CellSetDiff,
    BlockHeadersProvider,
    FnvHashMap<H256, &'a [CellOutput]>,
    ForkContext<'a, Shared<CS>>,
);

impl ForkChanges {
    pub fn attached_blocks(&self) -> &VecDeque<Block> {
        &self.attached_blocks
    }

    pub fn detached_blocks(&self) -> &VecDeque<Block> {
        &self.detached_blocks
    }

    pub fn detached_proposal_id(&self) -> &FnvHashSet<ProposalShortId> {
        &self.detached_proposal_id
    }

    pub fn has_detached(&self) -> bool {
        !self.detached_blocks.is_empty()
    }

    pub fn verified_len(&self) -> usize {
        self.attached_blocks.len() - self.dirty_exts.len()
    }

    pub fn build_verify_context<'a, CS: ChainStore>(
        &'a self,
        shared: &Shared<CS>,
    ) -> VerifyContext<'a, CS> {
        let mut cell_set_diff = CellSetDiff::default();
        let mut outputs: FnvHashMap<H256, &[CellOutput]> = FnvHashMap::default();
        let mut block_headers_provider = BlockHeadersProvider::default();

        let mut context = ForkContext {
            attached_blocks: Vec::with_capacity(self.attached_blocks().len()),
            detached_blocks: self.detached_blocks().iter().collect(),
            provider: shared.clone(),
        };

        for b in self.detached_blocks() {
            cell_set_diff.push_old(b);
            block_headers_provider.push_detached(b);
        }

        for b in self.attached_blocks().iter().take(self.verified_len()) {
            cell_set_diff.push_new(b);
            outputs.extend(
                b.transactions()
                    .iter()
                    .map(|tx| (tx.hash().to_owned(), tx.outputs())),
            );
            block_headers_provider.push_attached(b);
            context.attached_blocks.push(b);
        }

        (cell_set_diff, block_headers_provider, outputs, context)
    }
}

pub(crate) struct GlobalIndex {
    pub(crate) number: BlockNumber,
    pub(crate) hash: H256,
    pub(crate) unseen: bool,
}

impl GlobalIndex {
    pub(crate) fn new(number: BlockNumber, hash: H256, unseen: bool) -> GlobalIndex {
        GlobalIndex {
            number,
            hash,
            unseen,
        }
    }

    pub(crate) fn forward(&mut self, hash: H256) {
        self.number -= 1;
        self.hash = hash;
    }
}

pub struct ChainService<CS> {
    shared: Shared<CS>,
    notify: NotifyController,
}

impl<CS: ChainStore + 'static> ChainService<CS> {
    pub fn new(shared: Shared<CS>, notify: NotifyController) -> ChainService<CS> {
        ChainService { shared, notify }
    }

    pub fn start<S: ToString>(mut self, thread_name: Option<S>) -> ChainController {
        let (signal_sender, signal_receiver) =
            crossbeam_channel::bounded::<()>(SIGNAL_CHANNEL_SIZE);
        let (process_block_sender, process_block_receiver) =
            crossbeam_channel::bounded(DEFAULT_CHANNEL_SIZE);

        // Mainly for test: give a empty thread_name
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
        block: Arc<Block>,
        need_verify: bool,
    ) -> Result<(), FailureError> {
        debug!("begin processing block: {:x}", block.header().hash());
        if block.header().number() < 1 {
            warn!(
                "receive 0 number block: {}-{:x}",
                block.header().number(),
                block.header().hash()
            );
        }
        self.insert_block(block, need_verify)?;
        debug!("finish processing block");
        Ok(())
    }

    fn insert_block(&self, block: Arc<Block>, need_verify: bool) -> Result<(), FailureError> {
        // skip stored block
        if self
            .shared
            .store()
            .get_block_header(block.header().hash())
            .is_some()
        {
            return Ok(());
        }

        if need_verify {
            let block_verifier = BlockVerifier::new(self.shared.clone());
            block_verifier.verify(&block).map_err(|e| {
                debug!("[process_block] verification error {:?}", e);
                e
            })?
        }

        let mut total_difficulty = U256::zero();

        let mut fork = ForkChanges::default();
        let mut chain_state = self.shared.lock_chain_state();
        let mut txs_verify_cache = self.shared.lock_txs_verify_cache();

        let parent_ext = self
            .shared
            .store()
            .get_block_ext(&block.header().parent_hash())
            .expect("parent already store");

        let parent_header = self
            .shared
            .store()
            .get_block_header(&block.header().parent_hash())
            .expect("parent already store");

        let cannon_total_difficulty =
            parent_ext.total_difficulty.to_owned() + block.header().difficulty();
        let current_total_difficulty = chain_state.total_difficulty().to_owned();

        debug!(
            "difficulty current = {:#x}, cannon = {:#x}",
            current_total_difficulty, cannon_total_difficulty,
        );

        if parent_ext.verified == Some(false) {
            Err(SharedError::InvalidParentBlock)?;
        }

        let mut batch = self.shared.store().new_batch()?;
        batch.insert_block(&block)?;

        let parent_header_epoch = self
            .shared
            .get_block_epoch(&parent_header.hash())
            .expect("parent epoch already store");

        let next_epoch_ext = self
            .shared
            .next_epoch_ext(&parent_header_epoch, &parent_header);
        let new_epoch = next_epoch_ext.is_some();

        let epoch = next_epoch_ext.unwrap_or_else(|| parent_header_epoch.to_owned());

        let ext = BlockExt {
            received_at: unix_time_as_millis(),
            total_difficulty: cannon_total_difficulty.clone(),
            total_uncles_count: parent_ext.total_uncles_count + block.uncles().len() as u64,
            verified: None,
            txs_fees: vec![],
        };

        batch.insert_block_epoch_index(
            &block.header().hash(),
            epoch.last_block_hash_in_previous_epoch(),
        )?;
        batch.insert_epoch_ext(epoch.last_block_hash_in_previous_epoch(), &epoch)?;

        let new_best_block = (cannon_total_difficulty > current_total_difficulty)
            || ((current_total_difficulty == cannon_total_difficulty)
                && (block.header().hash() < chain_state.tip_hash()));

        if new_best_block {
            debug!(
                "new best block found: {} => {:#x}, difficulty diff = {:#x}",
                block.header().number(),
                block.header().hash(),
                &cannon_total_difficulty - &current_total_difficulty
            );
            self.find_fork(&mut fork, chain_state.tip_number(), &block, ext);
            self.update_index(
                &mut batch,
                fork.detached_blocks.iter(),
                fork.attached_blocks.iter(),
            )?;
            // MUST update index before reconcile_main_chain
            let cell_set_diff = self.reconcile_main_chain(
                &mut batch,
                &mut fork,
                &mut chain_state,
                &mut txs_verify_cache,
                need_verify,
            )?;
            self.update_proposal_ids(&mut chain_state, &fork);
            chain_state.update_cell_set(cell_set_diff, &mut batch)?;
            batch.insert_tip_header(&block.header())?;
            if new_epoch || fork.has_detached() {
                batch.insert_current_epoch_ext(&epoch)?;
            }

            total_difficulty = cannon_total_difficulty.clone();
        } else {
            batch.insert_block_ext(&block.header().hash(), &ext)?;
        }
        batch.commit()?;

        if new_best_block {
            let tip_header = block.header().to_owned();
            info!(
                "block: {}, hash: {:#x}, total_diff: {:#x}, txs: {}",
                tip_header.number(),
                tip_header.hash(),
                total_difficulty,
                block.transactions().len()
            );
            // finalize proposal_id table change
            // then, update tx_pool
            let detached_proposal_id = chain_state.proposal_ids_finalize(tip_header.number());
            fork.detached_proposal_id = detached_proposal_id;
            if new_epoch || fork.has_detached() {
                chain_state.update_current_epoch_ext(epoch);
            }
            chain_state.update_tip(tip_header, total_difficulty)?;
            chain_state.update_tx_pool_for_reorg(
                fork.detached_blocks().iter(),
                fork.attached_blocks().iter(),
                fork.detached_proposal_id().iter(),
                &mut txs_verify_cache,
            );
            for detached_block in fork.detached_blocks() {
                self.notify
                    .notify_new_uncle(Arc::new(detached_block.clone()));
            }
            if log_enabled!(ckb_logger::Level::Debug) {
                self.print_chain(&chain_state, 10);
            }
        } else {
            info!(
                "uncle: {}, hash: {:#x}, total_diff: {:#x}, txs: {}",
                block.header().number(),
                block.header().hash(),
                cannon_total_difficulty,
                block.transactions().len()
            );
            self.notify.notify_new_uncle(block);
        }

        Ok(())
    }

    pub(crate) fn update_proposal_ids(&self, chain_state: &mut ChainState<CS>, fork: &ForkChanges) {
        for blk in fork.detached_blocks() {
            chain_state.remove_proposal_ids(&blk);
        }
        for blk in fork.attached_blocks() {
            chain_state.insert_proposal_ids(&blk);
        }
    }

    pub(crate) fn update_index<'a>(
        &'a self,
        batch: &mut StoreBatch,
        detached_blocks: impl Iterator<Item = &'a Block>,
        attached_blocks: impl Iterator<Item = &'a Block>,
    ) -> Result<(), FailureError> {
        for block in detached_blocks {
            batch.detach_block(block)?;
        }

        for block in attached_blocks {
            batch.attach_block(block)?;
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
                index.forward(new_block.header().parent_hash().to_owned());
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
            index.forward(attached_block.header().parent_hash().to_owned());
            fork.attached_blocks.push_front(attached_block);
        }
    }

    pub(crate) fn find_fork(
        &self,
        fork: &mut ForkChanges,
        current_tip_number: BlockNumber,
        new_tip_block: &Block,
        new_tip_ext: BlockExt,
    ) {
        let new_tip_number = new_tip_block.header().number();
        fork.dirty_exts.push_front(new_tip_ext);

        // attached_blocks = forks[latest_common + 1 .. new_tip]
        // detached_blocks = chain[latest_common + 1 .. old_tip]
        fork.attached_blocks.push_front(new_tip_block.clone());

        let mut index = GlobalIndex::new(
            new_tip_number - 1,
            new_tip_block.header().parent_hash().to_owned(),
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
        batch: &mut StoreBatch,
        fork: &mut ForkChanges,
        chain_state: &mut ChainState<CS>,
        txs_verify_cache: &mut LruCache<H256, Cycle>,
        need_verify: bool,
    ) -> Result<CellSetDiff, FailureError> {
        let verified_len = fork.verified_len();

        let (mut cell_set_diff, mut block_headers_provider, mut outputs, mut verify_context) =
            fork.build_verify_context(&self.shared);

        let mut verify_results = fork
            .dirty_exts
            .iter()
            .zip(fork.attached_blocks().iter().skip(verified_len))
            .map(|(ext, b)| (b.header().hash().to_owned(), ext.verified, vec![]))
            .collect::<Vec<_>>();

        let mut found_error = None;
        // verify transaction
        for ((_, verified, l_txs_fees), b) in verify_results
            .iter_mut()
            .zip(fork.attached_blocks.iter().skip(verified_len))
        {
            if need_verify {
                if found_error.is_none() {
                    let contextual_block_verifier = ContextualBlockVerifier::new(&verify_context);
                    let mut seen_inputs = FnvHashSet::default();
                    let cell_set_overlay =
                        chain_state.new_cell_set_overlay(&cell_set_diff, &outputs);
                    let block_cp = match BlockCellProvider::new(b) {
                        Ok(block_cp) => block_cp,
                        Err(err) => {
                            found_error = Some(SharedError::UnresolvableTransaction(err));
                            continue;
                        }
                    };

                    let cell_provider = OverlayCellProvider::new(&block_cp, &cell_set_overlay);
                    block_headers_provider.push_attached(b);
                    let header_provider =
                        OverlayHeaderProvider::new(&block_headers_provider, &*chain_state);

                    match b
                        .transactions()
                        .iter()
                        .map(|x| {
                            resolve_transaction(
                                x,
                                &mut seen_inputs,
                                &cell_provider,
                                &header_provider,
                            )
                        })
                        .collect::<Result<Vec<ResolvedTransaction>, _>>()
                    {
                        Ok(resolved) => {
                            match contextual_block_verifier.verify(&resolved, b, txs_verify_cache) {
                                Ok((cycles, txs_fees)) => {
                                    cell_set_diff.push_new(b);
                                    outputs.extend(
                                        b.transactions()
                                            .iter()
                                            .map(|tx| (tx.hash().to_owned(), tx.outputs())),
                                    );
                                    *verified = Some(true);
                                    l_txs_fees.extend(txs_fees);
                                    let proof_size =
                                        self.shared.consensus().pow_engine().proof_size();
                                    if b.transactions().len() > 1 {
                                        info!(
                                            "[block_verifier] block number: {}, hash: {:#x}, size:{}/{}, cycles: {}/{}",
                                            b.header().number(),
                                            b.header().hash(),
                                            b.serialized_size(proof_size),
                                            self.shared.consensus().max_block_bytes(),
                                            cycles,
                                            self.shared.consensus().max_block_cycles()
                                        );
                                    }
                                }
                                Err(err) => {
                                    error!("block {:?} verify error{:?}", b, err);
                                    found_error =
                                        Some(SharedError::InvalidTransaction(err.to_string()));
                                    *verified = Some(false);
                                }
                            }
                        }
                        Err(err) => {
                            found_error = Some(SharedError::UnresolvableTransaction(err));
                            *verified = Some(false);
                        }
                    }
                } else {
                    *verified = Some(false);
                }
            } else {
                cell_set_diff.push_new(b);
                outputs.extend(
                    b.transactions()
                        .iter()
                        .map(|tx| (tx.hash().to_owned(), tx.outputs())),
                );
                *verified = Some(true);
            }
            verify_context.attached_blocks.push(b);
        }

        // update exts
        for (ext, (hash, verified, txs_fees)) in fork.dirty_exts.iter_mut().zip(verify_results) {
            ext.verified = verified;
            ext.txs_fees = txs_fees;
            batch.insert_block_ext(&hash, ext)?;
        }

        if let Some(err) = found_error {
            error!("fork {}", serde_json::to_string(&fork).unwrap());
            Err(err)?
        } else {
            Ok(cell_set_diff)
        }
    }

    // TODO: beatify
    fn print_chain(&self, chain_state: &ChainState<CS>, len: u64) {
        debug!("Chain {{");

        let tip = chain_state.tip_number();
        let bottom = tip - cmp::min(tip, len);

        for number in (bottom..=tip).rev() {
            let hash = self
                .shared
                .store()
                .get_block_hash(number)
                .unwrap_or_else(|| {
                    panic!(format!("invaild block number({}), tip={}", number, tip))
                });
            debug!("   {} => {:x}", number, hash);
        }

        debug!("}}");
    }
}
