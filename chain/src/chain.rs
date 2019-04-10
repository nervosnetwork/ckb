use ckb_core::block::Block;
use ckb_core::cell::{
    resolve_transaction, BlockCellProvider, OverlayCellProvider, ResolvedTransaction,
};
use ckb_core::extras::BlockExt;
use ckb_core::service::{Request, DEFAULT_CHANNEL_SIZE, SIGNAL_CHANNEL_SIZE};
use ckb_core::transaction::ProposalShortId;
use ckb_core::BlockNumber;
use ckb_notify::NotifyController;
use ckb_shared::cell_set::CellSetDiff;
use ckb_shared::chain_state::ChainState;
use ckb_shared::error::SharedError;
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::Shared;
use ckb_shared::store::StoreBatch;
use ckb_traits::ChainProvider;
use ckb_verification::{BlockVerifier, TransactionsVerifier, Verifier};
use crossbeam_channel::{self, select, Receiver, Sender};
use failure::Error as FailureError;
use faketime::unix_time_as_millis;
use fnv::FnvHashSet;
use log::{self, debug, error, log_enabled};
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::cmp;
use std::sync::Arc;
use std::thread;
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

#[derive(Debug, Default)]
pub struct ForkChanges {
    // blocks attached to index after forks
    pub(crate) attached_blocks: Vec<Block>,
    // blocks detached from index after forks
    pub(crate) detached_blocks: Vec<Block>,
    // proposal_id detached to index after forks
    pub(crate) detached_proposal_id: Vec<ProposalShortId>,
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

    pub fn detached_proposal_id(&self) -> &[ProposalShortId] {
        &self.detached_proposal_id
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

pub struct ChainService<CI> {
    shared: Shared<CI>,
    notify: NotifyController,
    verification: bool,
}

impl<CI: ChainIndex + 'static> ChainService<CI> {
    pub fn new(
        shared: Shared<CI>,
        notify: NotifyController,
        verification: bool,
    ) -> ChainService<CI> {
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
        debug!(target: "chain", "begin processing block: {}", block.header().hash());
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

    #[allow(clippy::op_ref)]
    pub(crate) fn insert_block(&self, block: Arc<Block>) -> Result<(), FailureError> {
        let mut new_best_block = false;
        let mut total_difficulty = U256::zero();

        let mut cell_set_diff = CellSetDiff::default();
        let mut fork = ForkChanges::default();
        let mut chain_state = self.shared.chain_state().lock();
        let tip_number = chain_state.tip_number();
        let tip_hash = chain_state.tip_hash();
        let parent_ext = self
            .shared
            .block_ext(&block.header().parent_hash())
            .expect("parent already store");

        let cannon_total_difficulty = parent_ext.total_difficulty + block.header().difficulty();
        let current_total_difficulty = chain_state.total_difficulty().clone();

        debug!(
            target: "chain",
            "difficulty current = {}, cannon = {}",
            current_total_difficulty,
            cannon_total_difficulty,
        );

        if parent_ext.txs_verified == Some(false) {
            Err(SharedError::InvalidParentBlock)?;
        }

        let ext = BlockExt {
            received_at: unix_time_as_millis(),
            total_difficulty: cannon_total_difficulty.clone(),
            total_uncles_count: parent_ext.total_uncles_count + block.uncles().len() as u64,
            txs_verified: None,
        };

        let mut batch = self.shared.store().new_batch();
        batch.insert_block(&block);

        if (cannon_total_difficulty > current_total_difficulty)
            || ((current_total_difficulty == cannon_total_difficulty)
                && (block.header().hash() < tip_hash))
        {
            debug!(
                target: "chain",
                "new best block found: {} => {}, difficulty diff = {}",
                block.header().number(), block.header().hash(),
                &cannon_total_difficulty - &current_total_difficulty
            );

            self.find_fork(&mut fork, tip_number, &block, ext);
            cell_set_diff = self.reconcile_main_chain(&mut batch, &mut fork, &mut chain_state)?;
            self.update_index(&mut batch, &fork.detached_blocks, &fork.attached_blocks);
            self.update_proposal_ids(&mut chain_state, &fork);
            batch.insert_tip_header(&block.header());
            new_best_block = true;

            total_difficulty = cannon_total_difficulty;
        } else {
            batch.insert_block_ext(&block.header().hash(), &ext);
        }

        batch.commit();

        // self.shared.store().save_with_batch(|batch| {
        //     self.shared.store().insert_block(batch, &block);

        //     if (cannon_total_difficulty > current_total_difficulty)
        //         || ((current_total_difficulty == cannon_total_difficulty)
        //             && (block.header().hash() < tip_hash))
        //     {
        //         debug!(
        //             target: "chain",
        //             "new best block found: {} => {}, difficulty diff = {}",
        //             block.header().number(), block.header().hash(),
        //             &cannon_total_difficulty - &current_total_difficulty
        //         );

        //         self.find_fork(&mut fork, tip_number, &block, ext);
        //         cell_set_diff = self.reconcile_main_chain(batch, &mut fork, &mut chain_state)?;
        //         self.update_index(batch, &fork.detached_blocks, &fork.attached_blocks);
        //         self.update_proposal_ids(&mut chain_state, &fork);
        //         self.shared
        //             .store()
        //             .insert_tip_header(batch, &block.header());

        //         new_best_block = true;

        //         total_difficulty = cannon_total_difficulty;
        //     } else {
        //         self.shared
        //             .store()
        //             .insert_block_ext(batch, &block.header().hash(), &ext);
        //     }
        //     Ok(())
        // })?;

        if new_best_block {
            let tip_header = block.header().clone();
            let detached_proposal_id = chain_state.proposal_ids_finalize(tip_header.number());
            fork.detached_proposal_id = detached_proposal_id;
            chain_state.update_tip(tip_header, total_difficulty, cell_set_diff);
            chain_state.update_tx_pool_for_reorg(
                fork.detached_blocks(),
                fork.attached_blocks(),
                fork.detached_proposal_id(),
                self.shared.consensus().max_block_cycles(),
            );
            if log_enabled!(target: "chain", log::Level::Debug) {
                self.print_chain(&chain_state, 10);
            }
        } else {
            self.notify.notify_new_uncle(block);
        }

        Ok(())
    }

    pub(crate) fn update_proposal_ids(&self, chain_state: &mut ChainState<CI>, fork: &ForkChanges) {
        for blk in fork.attached_blocks() {
            chain_state.update_proposal_ids(&blk);
        }
    }

    pub(crate) fn update_index(
        &self,
        batch: &mut StoreBatch,
        detached_blocks: &[Block],
        attached_blocks: &[Block],
    ) {
        let old_number = match detached_blocks.get(0) {
            Some(b) => b.header().number(),
            None => 0,
        };

        let new_number = attached_blocks[0].header().number();

        for block in detached_blocks {
            batch.delete_block_number(&block.header().hash());
            batch.delete_transaction_address(block.commit_transactions());
        }

        for block in attached_blocks {
            let number = block.header().number();
            let hash = block.header().hash();
            batch.insert_block_hash(number, &hash);
            batch.insert_block_number(&hash, number);
            batch.insert_transaction_address(&hash, block.commit_transactions());
        }

        for n in new_number..old_number {
            batch.delete_block_hash(n + 1);
        }
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
                index.forward(new_block.header().parent_hash().clone());
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
            index.forward(attached_block.header().parent_hash().clone());
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
            new_tip_block.header().parent_hash().clone(),
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
        chain_state: &mut ChainState<CI>,
    ) -> Result<CellSetDiff, FailureError> {
        let mut cell_set_diff = CellSetDiff::default();

        let attached_blocks_iter = fork.attached_blocks().iter().rev();
        let detached_blocks_iter = fork.detached_blocks().iter().rev();

        let attached_blocks_len = fork.attached_blocks.len();
        let verified_len = attached_blocks_len - fork.dirty_exts.len();

        for b in detached_blocks_iter {
            cell_set_diff.push_old(b);
        }

        for b in attached_blocks_iter.take(verified_len) {
            cell_set_diff.push_new(b);
        }

        // The verify function
        let txs_verifier = TransactionsVerifier::new(self.shared.consensus().max_block_cycles());

        let mut found_error = None;
        // verify transaction
        for (ext, b) in fork
            .dirty_exts
            .iter_mut()
            .zip(fork.attached_blocks.iter())
            .rev()
        {
            if self.verification {
                if found_error.is_none() {
                    let mut seen_inputs = FnvHashSet::default();

                    let cell_set_diff_cp = OverlayCellProvider::new(&cell_set_diff, chain_state);
                    let block_cp = BlockCellProvider::new(b);
                    let cell_provider = OverlayCellProvider::new(&block_cp, &cell_set_diff_cp);

                    let resolved: Vec<ResolvedTransaction> = b
                        .commit_transactions()
                        .iter()
                        .map(|x| resolve_transaction(x, &mut seen_inputs, &cell_provider))
                        .collect();

                    match txs_verifier.verify(
                        chain_state.mut_txs_verify_cache(),
                        &resolved,
                        self.shared.block_reward(b.header().number()),
                    ) {
                        Ok(_) => {
                            cell_set_diff.push_new(b);
                            ext.txs_verified = Some(true);
                        }
                        Err(err) => {
                            found_error = Some(err);
                            ext.txs_verified = Some(false);
                        }
                    }
                } else {
                    ext.txs_verified = Some(false);
                }
            } else {
                cell_set_diff.push_new(b);
                ext.txs_verified = Some(true);
            }
        }

        // update exts
        for (ext, b) in fork
            .dirty_exts
            .iter()
            .zip(fork.attached_blocks().iter())
            .rev()
        {
            batch.insert_block_ext(&b.header().hash(), ext);
        }

        if let Some(err) = found_error {
            Err(SharedError::InvalidTransaction(err.to_string()))?
        } else {
            Ok(cell_set_diff)
        }
    }

    // TODO: beatify
    fn print_chain(&self, chain_state: &ChainState<CI>, len: u64) {
        debug!(target: "chain", "Chain {{");

        let tip = chain_state.tip_number();
        let bottom = tip - cmp::min(tip, len);

        for number in (bottom..=tip).rev() {
            let hash = self.shared.block_hash(number).unwrap_or_else(|| {
                panic!(format!("invaild block number({}), tip={}", number, tip))
            });
            debug!(target: "chain", "   {} => {}", number, hash);
        }

        debug!(target: "chain", "}}");

        // TODO: remove me when block explorer is available
        debug!(target: "chain", "Tx in Head Block {{");
        for transaction in self
            .shared
            .block_hash(tip)
            .and_then(|hash| self.shared.store().get_block_body(&hash))
            .expect("invalid block number")
        {
            debug!(target: "chain", "   {} => {:?}", transaction.hash(), transaction);
        }

        debug!(target: "chain", "}}");

        debug!(target: "chain", "Proposal in Head Block {{");
        debug!(target: "chain",  "   {:?}", self
            .shared
            .block_hash(tip)
            .and_then(|hash| self.shared.store().get_block_proposal_txs_ids(&hash))
            .expect("invalid block number"));

        debug!(target: "chain", "}}");

        debug!(target: "chain", "Uncle block {{");
        for (index, uncle) in self
            .shared
            .block_hash(tip)
            .and_then(|hash| self.shared.store().get_block_uncles(&hash))
            .expect("invalid block number")
            .iter()
            .enumerate()
        {
            debug!(target: "chain", "   {} => {:?}", index, uncle);
        }
        debug!(target: "chain", "}}");
    }
}

pub struct ChainBuilder<CI> {
    shared: Shared<CI>,
    notify: NotifyController,
    verification: bool,
}

impl<CI: ChainIndex + 'static> ChainBuilder<CI> {
    pub fn new(shared: Shared<CI>, notify: NotifyController) -> ChainBuilder<CI> {
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

    pub fn build(self) -> ChainService<CI> {
        ChainService::new(self.shared, self.notify, self.verification)
    }
}
