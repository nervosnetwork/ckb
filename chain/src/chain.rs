use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::Block;
use ckb_core::cell::{
    resolve_transaction, BlockCellProvider, BlockHeadersProvider, OverlayCellProvider,
    OverlayHeaderProvider, ResolvedTransaction,
};
use ckb_core::extras::{BlockExt, DaoStats};
use ckb_core::service::{Request, DEFAULT_CHANNEL_SIZE, SIGNAL_CHANNEL_SIZE};
use ckb_core::transaction::{CellOutput, ProposalShortId};
use ckb_core::{header::Header, BlockNumber, Cycle};
use ckb_notify::NotifyController;
use ckb_shared::cell_set::CellSetDiff;
use ckb_shared::chain_state::ChainState;
use ckb_shared::error::SharedError;
use ckb_shared::shared::Shared;
use ckb_store::{ChainStore, StoreBatch};
use ckb_traits::{BlockMedianTimeContext, ChainProvider};
use ckb_verification::{BlockVerifier, TransactionsVerifier, Verifier};
use crossbeam_channel::{self, select, Receiver, Sender};
use dao::calculate_dao_data;
use failure::Error as FailureError;
use faketime::unix_time_as_millis;
use fnv::{FnvHashMap, FnvHashSet};
use log::{self, debug, error, info, log_enabled, warn};
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use serde_derive::{Deserialize, Serialize};
use std::sync::Arc;
use std::{cmp, mem, thread};
use stop_handler::{SignalSender, StopHandler};

#[derive(Clone)]
pub struct ChainController {
    process_block_sender: Sender<Request<Arc<Block>, Result<(), FailureError>>>,
    stop: StopHandler<()>,
}

impl Drop for ChainController {
    fn drop(&mut self) {
        self.stop.try_send();
    }
}

impl ChainController {
    pub fn process_block(&self, block: Arc<Block>) -> Result<(), FailureError> {
        Request::call(&self.process_block_sender, block).expect("process_block() failed")
    }
}

struct ChainReceivers {
    process_block_receiver: Receiver<Request<Arc<Block>, Result<(), FailureError>>>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ForkChanges {
    // blocks attached to index after forks
    pub(crate) attached_blocks: Vec<Block>,
    // blocks detached from index after forks
    pub(crate) detached_blocks: Vec<Block>,
    // proposal_id detached to index after forks
    pub(crate) detached_proposal_id: FnvHashSet<ProposalShortId>,
    // to be updated exts
    pub(crate) dirty_exts: Vec<BlockExt>,
}

impl ForkChanges {
    pub fn attached_blocks(&self) -> &[Block] {
        &self.attached_blocks
    }

    pub fn detached_blocks(&self) -> &[Block] {
        &self.detached_blocks
    }

    pub fn detached_proposal_id(&self) -> &FnvHashSet<ProposalShortId> {
        &self.detached_proposal_id
    }

    pub fn has_detached(&self) -> bool {
        !self.detached_blocks.is_empty()
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

// Verification context for fork
struct ForkContext<'a, CS> {
    pub fork_blocks: &'a [Block],
    pub store: Arc<CS>,
    pub consensus: &'a Consensus,
}

impl<'a, CS: ChainStore> ForkContext<'a, CS> {
    fn get_header(&self, number: BlockNumber) -> Option<Header> {
        match self
            .fork_blocks
            .iter()
            .find(|b| b.header().number() == number)
        {
            Some(block) => Some(block.header().to_owned()),
            None => self
                .store
                .get_block_hash(number)
                .and_then(|hash| self.store.get_header(&hash)),
        }
    }
}

impl<'a, CS: ChainStore> BlockMedianTimeContext for ForkContext<'a, CS> {
    fn median_block_count(&self) -> u64 {
        self.consensus.median_time_block_count() as u64
    }

    fn timestamp(&self, number: BlockNumber) -> Option<u64> {
        self.get_header(number).map(|header| header.timestamp())
    }
}

pub struct ChainService<CS> {
    shared: Shared<CS>,
    notify: NotifyController,
    verification: bool,
}

impl<CS: ChainStore + 'static> ChainService<CS> {
    pub fn new(
        shared: Shared<CS>,
        notify: NotifyController,
        verification: bool,
    ) -> ChainService<CS> {
        ChainService {
            shared,
            notify,
            verification,
        }
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
                        Ok(Request { responder, arguments: block }) => {
                            let _ = responder.send(self.process_block(block));
                        },
                        _ => {
                            error!(target: "chain", "process_block_receiver closed");
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
    pub(crate) fn process_block(&mut self, block: Arc<Block>) -> Result<(), FailureError> {
        debug!(target: "chain", "begin processing block: {:x}", block.header().hash());
        if block.header().number() < 1 {
            warn!(target: "chain", "receive 0 number block: {}-{:x}", block.header().number(), block.header().hash());
        }
        if self.verification {
            let block_verifier = BlockVerifier::new(self.shared.clone());
            block_verifier.verify(&block).map_err(|e| {
                debug!(target: "chain", "[process_block] verification error {:?}", e);
                e
            })?
        }
        self.insert_block(block)?;
        debug!(target: "chain", "finish processing block");
        Ok(())
    }

    pub(crate) fn insert_block(&self, block: Arc<Block>) -> Result<(), FailureError> {
        let mut new_best_block = false;
        let mut total_difficulty = U256::zero();

        let mut cell_set_diff = CellSetDiff::default();
        let mut fork = ForkChanges::default();
        let mut chain_state = self.shared.chain_state().lock();
        let mut txs_verify_cache = self.shared.txs_verify_cache().lock();

        let parent_ext = self
            .shared
            .block_ext(&block.header().parent_hash())
            .expect("parent already store");

        let parent_header = self
            .shared
            .block_header(&block.header().parent_hash())
            .expect("parent already store");

        let cannon_total_difficulty =
            parent_ext.total_difficulty.to_owned() + block.header().difficulty();
        let current_total_difficulty = chain_state.total_difficulty().to_owned();

        debug!(
            target: "chain",
            "difficulty current = {}, cannon = {}",
            current_total_difficulty,
            cannon_total_difficulty,
        );

        if parent_ext.txs_verified == Some(false) {
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

        let epoch = next_epoch_ext.unwrap_or(parent_header_epoch);

        let (ar, c) = calculate_dao_data(
            block.header().number(),
            &parent_ext.dao_stats,
            &epoch,
            self.shared.consensus().secondary_epoch_reward(),
        )?;

        let ext = BlockExt {
            received_at: unix_time_as_millis(),
            total_difficulty: cannon_total_difficulty.clone(),
            total_uncles_count: parent_ext.total_uncles_count + block.uncles().len() as u64,
            txs_verified: None,
            dao_stats: DaoStats {
                accumulated_rate: ar,
                accumulated_capacity: c.as_u64(),
            },
        };

        batch.insert_block_epoch_index(
            &block.header().hash(),
            epoch.last_block_hash_in_previous_epoch(),
        )?;
        batch.insert_epoch_ext(epoch.last_block_hash_in_previous_epoch(), &epoch)?;

        if (cannon_total_difficulty > current_total_difficulty)
            || ((current_total_difficulty == cannon_total_difficulty)
                && (block.header().hash() < chain_state.tip_hash()))
        {
            debug!(
                target: "chain",
                "new best block found: {} => {}, difficulty diff = {}",
                block.header().number(), block.header().hash(),
                &cannon_total_difficulty - &current_total_difficulty
            );
            self.find_fork(&mut fork, chain_state.tip_number(), &block, ext);
            self.update_index(&mut batch, &fork.detached_blocks, &fork.attached_blocks)?;
            // MUST update index before reconcile_main_chain
            cell_set_diff = self.reconcile_main_chain(
                &mut batch,
                &mut fork,
                &mut chain_state,
                &mut txs_verify_cache,
            )?;
            self.update_proposal_ids(&mut chain_state, &fork);
            batch.insert_tip_header(&block.header())?;
            if new_epoch || fork.has_detached() {
                batch.insert_current_epoch_ext(&epoch)?;
            }
            new_best_block = true;

            total_difficulty = cannon_total_difficulty.clone();
        } else {
            batch.insert_block_ext(&block.header().hash(), &ext)?;
        }
        batch.commit()?;

        if new_best_block {
            let tip_header = block.header().to_owned();
            info!(
                target: "chain",
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
            chain_state.update_tip(tip_header, total_difficulty, cell_set_diff);
            chain_state.update_tx_pool_for_reorg(
                fork.detached_blocks().iter(),
                fork.attached_blocks().iter(),
                fork.detached_proposal_id().iter(),
                &mut txs_verify_cache,
            );
            if log_enabled!(target: "chain", log::Level::Debug) {
                self.print_chain(&chain_state, 10);
            }
        } else {
            info!(
                target: "chain",
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

    pub(crate) fn update_index(
        &self,
        batch: &mut StoreBatch,
        detached_blocks: &[Block],
        attached_blocks: &[Block],
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
                    .block_hash(bn)
                    .expect("block hash stored before alignment_fork");
                let old_block = self
                    .shared
                    .block(&hash)
                    .expect("block data stored before alignment_fork");
                fork.detached_blocks.push(old_block);
            }
        } else {
            while index.number > current_tip_number {
                if index.unseen {
                    let ext = self
                        .shared
                        .block_ext(&index.hash)
                        .expect("block ext stored before alignment_fork");
                    if ext.txs_verified.is_none() {
                        fork.dirty_exts.push(ext)
                    } else {
                        index.unseen = false;
                    }
                }
                let new_block = self
                    .shared
                    .block(&index.hash)
                    .expect("block data stored before alignment_fork");
                index.forward(new_block.header().parent_hash().to_owned());
                fork.attached_blocks.push(new_block);
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
                .block_hash(index.number)
                .expect("detached hash stored before find_fork_until_latest_common");
            if detached_hash == index.hash {
                break;
            }
            let detached_blocks = self
                .shared
                .block(&detached_hash)
                .expect("detached block stored before find_fork_until_latest_common");
            fork.detached_blocks.push(detached_blocks);

            if index.unseen {
                let ext = self
                    .shared
                    .block_ext(&index.hash)
                    .expect("block ext stored before find_fork_until_latest_common");
                if ext.txs_verified.is_none() {
                    fork.dirty_exts.push(ext)
                } else {
                    index.unseen = false;
                }
            }

            let attached_block = self
                .shared
                .block(&index.hash)
                .expect("attached block stored before find_fork_until_latest_common");
            index.forward(attached_block.header().parent_hash().to_owned());
            fork.attached_blocks.push(attached_block);
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
        fork.dirty_exts.push(new_tip_ext);

        // attached_blocks = forks[latest_common + 1 .. new_tip]
        // detached_blocks = chain[latest_common + 1 .. old_tip]
        fork.attached_blocks.push(new_tip_block.clone());

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

    // we found new best_block total_difficulty > old_chain.total_difficulty
    pub(crate) fn reconcile_main_chain(
        &self,
        batch: &mut StoreBatch,
        fork: &mut ForkChanges,
        chain_state: &mut ChainState<CS>,
        txs_verify_cache: &mut LruCache<H256, Cycle>,
    ) -> Result<CellSetDiff, FailureError> {
        let mut cell_set_diff = CellSetDiff::default();
        let mut outputs: FnvHashMap<H256, &[CellOutput]> = FnvHashMap::default();
        let mut block_headers_provider = BlockHeadersProvider::default();

        let mut dirty_exts = Vec::new();
        // cause we need borrow outputs from fork, swap `dirty_exts` out to evade from borrow check
        mem::swap(&mut fork.dirty_exts, &mut dirty_exts);

        let attached_blocks_iter = fork.attached_blocks().iter().rev();
        let detached_blocks_iter = fork.detached_blocks().iter().rev();

        let unverified_len = fork.attached_blocks.len() - dirty_exts.len();

        for b in detached_blocks_iter {
            cell_set_diff.push_old(b);
            block_headers_provider.push_detached(b);
        }

        for b in attached_blocks_iter.take(unverified_len) {
            cell_set_diff.push_new(b);
            outputs.extend(
                b.transactions()
                    .iter()
                    .map(|tx| (tx.hash().to_owned(), tx.outputs())),
            );
            block_headers_provider.push_attached(b);
        }

        // The verify function
        let txs_verifier = TransactionsVerifier::new(
            self.shared.clone(),
            self.shared.consensus().max_block_cycles(),
            self.shared.script_config(),
        );

        let mut found_error = None;
        // verify transaction
        for (ext, b) in dirty_exts.iter_mut().zip(fork.attached_blocks.iter()).rev() {
            if self.verification {
                if found_error.is_none() {
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
                            let cellbase_maturity = self.shared.consensus().cellbase_maturity();

                            let parent_hash = b.header().parent_hash();
                            let parent_ext = self
                                .shared
                                .get_block_epoch(parent_hash)
                                .expect("parent header verified");
                            let parent = self
                                .shared
                                .block_header(parent_hash)
                                .expect("parent header verified");
                            let epoch = self
                                .shared
                                .next_epoch_ext(&parent_ext, &parent)
                                .unwrap_or(parent_ext);

                            match txs_verifier.verify(
                                &resolved,
                                Arc::clone(self.shared.store()),
                                &epoch,
                                ForkContext {
                                    fork_blocks: &fork.attached_blocks,
                                    store: Arc::clone(self.shared.store()),
                                    consensus: self.shared.consensus(),
                                },
                                b.header(),
                                cellbase_maturity,
                                txs_verify_cache,
                            ) {
                                Ok(_) => {
                                    cell_set_diff.push_new(b);
                                    outputs.extend(
                                        b.transactions()
                                            .iter()
                                            .map(|tx| (tx.hash().to_owned(), tx.outputs())),
                                    );
                                    ext.txs_verified = Some(true);
                                }
                                Err(err) => {
                                    error!(target: "chain", "cell_set_diff {}", serde_json::to_string(&cell_set_diff).unwrap());
                                    error!(target: "chain", "block {}", serde_json::to_string(b).unwrap());
                                    found_error =
                                        Some(SharedError::InvalidTransaction(err.to_string()));
                                    ext.txs_verified = Some(false);
                                }
                            }
                        }
                        Err(err) => {
                            found_error = Some(SharedError::UnresolvableTransaction(err));
                            ext.txs_verified = Some(false);
                        }
                    }
                } else {
                    ext.txs_verified = Some(false);
                }

                if found_error.is_some() {
                    error!(target: "chain", "cell_set {}", serde_json::to_string(&chain_state.cell_set()).unwrap());
                }
            } else {
                cell_set_diff.push_new(b);
                outputs.extend(
                    b.transactions()
                        .iter()
                        .map(|tx| (tx.hash().to_owned(), tx.outputs())),
                );
                ext.txs_verified = Some(true);
            }
        }
        mem::replace(&mut fork.dirty_exts, dirty_exts);

        // update exts
        for (ext, b) in fork
            .dirty_exts
            .iter()
            .zip(fork.attached_blocks().iter())
            .rev()
        {
            batch.insert_block_ext(&b.header().hash(), ext)?;
        }

        if let Some(err) = found_error {
            error!(target: "chain", "fork {}", serde_json::to_string(&fork).unwrap());
            Err(err)?
        } else {
            Ok(cell_set_diff)
        }
    }

    // TODO: beatify
    fn print_chain(&self, chain_state: &ChainState<CS>, len: u64) {
        debug!(target: "chain", "Chain {{");

        let tip = chain_state.tip_number();
        let bottom = tip - cmp::min(tip, len);

        for number in (bottom..=tip).rev() {
            let hash = self.shared.block_hash(number).unwrap_or_else(|| {
                panic!(format!("invaild block number({}), tip={}", number, tip))
            });
            debug!(target: "chain", "   {} => {:x}", number, hash);
        }

        debug!(target: "chain", "}}");
    }
}

pub struct ChainBuilder<CS> {
    shared: Shared<CS>,
    notify: NotifyController,
    verification: bool,
}

impl<CS: ChainStore + 'static> ChainBuilder<CS> {
    pub fn new(shared: Shared<CS>, notify: NotifyController) -> ChainBuilder<CS> {
        ChainBuilder {
            shared,
            notify,
            verification: true,
        }
    }

    pub fn notify(mut self, value: NotifyController) -> Self {
        self.notify = value;
        self
    }

    pub fn verification(mut self, verification: bool) -> Self {
        self.verification = verification;
        self
    }

    pub fn build(self) -> ChainService<CS> {
        ChainService::new(self.shared, self.notify, self.verification)
    }
}
