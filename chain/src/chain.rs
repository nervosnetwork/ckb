use crate::error::ProcessBlockError;
use ckb_core::block::Block;
use ckb_core::cell::CellProvider;
use ckb_core::extras::BlockExt;
use ckb_core::header::BlockNumber;
use ckb_core::service::{Request, DEFAULT_CHANNEL_SIZE, SIGNAL_CHANNEL_SIZE};
use ckb_core::transaction::OutPoint;
use ckb_db::batch::Batch;
use ckb_notify::{ForkBlocks, NotifyController};
use ckb_shared::error::SharedError;
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::{ChainProvider, ChainState, Shared};
use ckb_shared::txo_set::TxoSetDiff;
use ckb_verification::{BlockVerifier, TransactionsVerifier, Verifier};
use crossbeam_channel::{self, select, Receiver, Sender};
use faketime::unix_time_as_millis;
use fnv::{FnvHashMap, FnvHashSet};
use log::{self, debug, error, log_enabled};
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::cmp;
use std::sync::Arc;
use std::thread;
use stop_handler::{SignalSender, StopHandler};

#[derive(Clone)]
pub struct ChainController {
    process_block_sender: Sender<Request<Arc<Block>, Result<(), ProcessBlockError>>>,
    stop: StopHandler<()>,
}

impl Drop for ChainController {
    fn drop(&mut self) {
        self.stop.try_send();
    }
}

impl ChainController {
    pub fn process_block(&self, block: Arc<Block>) -> Result<(), ProcessBlockError> {
        Request::call(&self.process_block_sender, block).expect("process_block() failed")
    }
}

struct ChainReceivers {
    process_block_receiver: Receiver<Request<Arc<Block>, Result<(), ProcessBlockError>>>,
}

#[derive(Debug, Clone)]
pub struct BlockInsertionResult {
    pub fork_blks: ForkBlocks,
    pub new_best_block: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Fork {
    pub(crate) new_blocks: Vec<Block>,
    pub(crate) old_blocks: Vec<Block>,
    pub(crate) open_exts: Vec<BlockExt>,
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
    pub(crate) fn process_block(&mut self, block: Arc<Block>) -> Result<(), ProcessBlockError> {
        debug!(target: "chain", "begin processing block: {}", block.header().hash());
        if self.verification {
            let block_verifier = BlockVerifier::new(self.shared.clone());
            block_verifier
                .verify(&block)
                .map_err(ProcessBlockError::Verification)?
        }
        let insert_result = self
            .insert_block(&block)
            .map_err(ProcessBlockError::Shared)?;
        self.post_insert_result(block, insert_result);
        debug!(target: "chain", "finish processing block");
        Ok(())
    }

    #[allow(clippy::op_ref)]
    pub(crate) fn insert_block(&self, block: &Block) -> Result<BlockInsertionResult, SharedError> {
        let mut new_best_block = false;
        let mut total_difficulty = U256::zero();

        let mut txo_set_diff = TxoSetDiff::default();
        let mut fork = Fork::default();

        let mut chain_state = self.shared.chain_state().write();
        let tip_number = chain_state.tip_number();
        let parent_ext = self
            .shared
            .block_ext(&block.header().parent_hash())
            .expect("parent already store");

        let cannon_total_difficulty = parent_ext.total_difficulty + block.header().difficulty();
        let current_total_difficulty = chain_state.total_difficulty();

        debug!(
            target: "chain",
            "difficulty current = {}, cannon = {}",
            current_total_difficulty,
            cannon_total_difficulty,
        );

        let ext = BlockExt {
            received_at: unix_time_as_millis(),
            total_difficulty: cannon_total_difficulty.clone(),
            total_uncles_count: parent_ext.total_uncles_count + block.uncles().len() as u64,
            // if txs in parent is invalid, txs in block is also invalid
            valid: if parent_ext.valid == Some(false) {
                Some(false)
            } else {
                None
            },
        };

        self.shared.store().save_with_batch(|batch| {
            self.shared.store().insert_block(batch, block);

            if &cannon_total_difficulty > current_total_difficulty
                || (current_total_difficulty == &cannon_total_difficulty
                    && block.header().hash() < chain_state.tip_hash())
            {
                debug!(
                    target: "chain",
                    "new best block found: {} => {}, difficulty diff = {}",
                    block.header().number(), block.header().hash(),
                    &cannon_total_difficulty - current_total_difficulty
                );

                let (di, fo) =
                    self.reconcile_main_chain(batch, tip_number, block, ext, &*chain_state)?;

                txo_set_diff = di;
                fork = fo;

                self.shared
                    .store()
                    .insert_tip_header(batch, &block.header());

                new_best_block = true;

                total_difficulty = cannon_total_difficulty;
            } else {
                self.shared
                    .store()
                    .insert_block_ext(batch, &block.header().hash(), &ext);
            }
            Ok(())
        })?;

        if new_best_block {
            debug!(target: "chain", "update index");

            chain_state.update_header(block.header().clone());
            chain_state.update_difficulty(total_difficulty);
            chain_state.update_txo_set(txo_set_diff);

            debug!(target: "chain", "update index release");
        }

        Ok(BlockInsertionResult {
            new_best_block,
            fork_blks: ForkBlocks::new(fork.old_blocks, fork.new_blocks),
        })
    }

    pub(crate) fn post_insert_result(&mut self, block: Arc<Block>, result: BlockInsertionResult) {
        let BlockInsertionResult {
            new_best_block,
            fork_blks,
        } = result;

        if new_best_block {
            self.notify.notify_switch_fork(Arc::new(fork_blks));
            if log_enabled!(target: "chain", log::Level::Debug) {
                self.print_chain(10);
            }
        } else {
            self.notify.notify_new_uncle(block);
        }
    }

    pub(crate) fn update_index(
        &self,
        batch: &mut Batch,
        old_blocks: &[Block],
        new_blocks: &[Block],
    ) {
        let old_number = match old_blocks.get(0) {
            Some(b) => b.header().number(),
            None => 0,
        };

        let new_number = new_blocks[0].header().number();

        for block in old_blocks {
            self.shared
                .store()
                .delete_block_number(batch, &block.header().hash());
            self.shared
                .store()
                .delete_transaction_address(batch, block.commit_transactions());
        }

        for block in new_blocks {
            let number = block.header().number();
            let hash = block.header().hash();
            self.shared.store().insert_block_hash(batch, number, &hash);
            self.shared
                .store()
                .insert_block_number(batch, &hash, number);
            self.shared.store().insert_transaction_address(
                batch,
                &hash,
                block.commit_transactions(),
            );
        }

        for n in new_number..old_number {
            self.shared.store().delete_block_hash(batch, n + 1);
        }
    }

    fn alignment_fork(
        &self,
        fork: &mut Fork,
        index: &mut GlobalIndex,
        new_tip_number: BlockNumber,
        current_tip_number: BlockNumber,
    ) {
        if new_tip_number <= current_tip_number {
            for bn in new_tip_number..=current_tip_number {
                let hash = self.shared.block_hash(bn).unwrap();
                let old_block = self.shared.block(&hash).unwrap();
                fork.old_blocks.push(old_block);
            }
        } else {
            while index.number > current_tip_number {
                if index.unseen {
                    let ext = self.shared.block_ext(&index.hash).unwrap();
                    if ext.valid.is_none() {
                        fork.open_exts.push(ext)
                    } else {
                        index.unseen = false;
                    }
                }
                let new_block = self.shared.block(&index.hash).unwrap();
                index.forward(new_block.header().parent_hash().clone());
                fork.new_blocks.push(new_block);
            }
        }
    }

    fn find_fork_until_latest_common(&self, fork: &mut Fork, index: &mut GlobalIndex) {
        loop {
            if index.number == 0 {
                break;
            }
            let old_hash = self.shared.block_hash(index.number).unwrap();
            if old_hash == index.hash {
                break;
            }
            let old_block = self.shared.block(&old_hash).unwrap();
            fork.old_blocks.push(old_block);

            if index.unseen {
                let ext = self.shared.block_ext(&index.hash).unwrap();
                if ext.valid.is_none() {
                    fork.open_exts.push(ext)
                } else {
                    index.unseen = false;
                }
            }

            let new_block = self.shared.block(&index.hash).unwrap();
            index.forward(new_block.header().parent_hash().clone());
            fork.new_blocks.push(new_block);
        }
    }

    pub(crate) fn find_fork(
        &self,
        current_tip_number: BlockNumber,
        new_tip_block: &Block,
        new_tip_ext: BlockExt,
    ) -> Option<Fork> {
        let new_tip_number = new_tip_block.header().number();
        let mut fork = Fork::default();

        if new_tip_ext.valid.is_none() {
            fork.open_exts.push(new_tip_ext);
        } else {
            // txs in block are invalid
            return None;
        }

        // new_blocks = forks[latest_common + 1 .. new_tip]
        // old_blocks = chain[latest_common + 1 .. old_tip]
        fork.new_blocks.push(new_tip_block.clone());

        let mut index = GlobalIndex::new(
            new_tip_number - 1,
            new_tip_block.header().parent_hash().clone(),
            true,
        );

        // if new_tip_number <= current_tip_number
        // then old_blocks.extend(chain[new_tip_number .. =current_tip_number])
        // if new_tip_number > current_tip_number
        // then new_blocks.extend(forks[current_tip_number + 1 .. =new_tip_number])
        self.alignment_fork(&mut fork, &mut index, new_tip_number, current_tip_number);

        // find latest common ancestor
        self.find_fork_until_latest_common(&mut fork, &mut index);

        Some(fork)
    }

    // we found new best_block total_difficulty > old_chain.total_difficulty
    pub(crate) fn reconcile_main_chain(
        &self,
        batch: &mut Batch,
        tip_number: BlockNumber,
        block: &Block,
        ext: BlockExt,
        chain_state: &ChainState,
    ) -> Result<(TxoSetDiff, Fork), SharedError> {
        let skip_verify = !self.verification;

        let mut fork = self
            .find_fork(tip_number, block, ext)
            .ok_or(SharedError::InvalidTransaction)?;

        let mut old_inputs = FnvHashSet::default();
        let mut old_outputs = FnvHashSet::default();
        let mut new_inputs = FnvHashSet::default();
        let mut new_outputs = FnvHashMap::default();

        let push_new = |b: &Block,
                        new_inputs: &mut FnvHashSet<OutPoint>,
                        new_outputs: &mut FnvHashMap<H256, usize>| {
            for tx in b.commit_transactions() {
                let input_pts = tx.input_pts();
                let tx_hash = tx.hash();
                let output_len = tx.outputs().len();
                for pt in input_pts {
                    new_inputs.insert(pt);
                }

                new_outputs.insert(tx_hash, output_len);
            }
        };

        let new_blocks_iter = fork.new_blocks.iter().rev();
        let old_blocks_iter = fork.old_blocks.iter().rev();

        let new_blocks_len = fork.new_blocks.len();
        let verified_len = new_blocks_len - fork.open_exts.len();

        for b in old_blocks_iter {
            for tx in b.commit_transactions() {
                let input_pts = tx.input_pts();
                let tx_hash = tx.hash();

                for pt in input_pts {
                    old_inputs.insert(pt);
                }

                old_outputs.insert(tx_hash);
            }
        }

        for b in new_blocks_iter.clone().take(verified_len) {
            push_new(b, &mut new_inputs, &mut new_outputs);
        }

        let mut txs_cache = self.shared.txs_verify_cache().write();
        // The verify function
        let txs_verifier = TransactionsVerifier::new(self.shared.consensus().max_block_cycles());

        let mut found_error = false;
        // verify transaction
        for (ext, b) in fork.open_exts.iter_mut().zip(fork.new_blocks.iter()).rev() {
            if !found_error
                || skip_verify
                || txs_verifier
                    .verify(&mut *txs_cache, b, |op: &OutPoint| {
                        self.shared.cell_at(op, |op| {
                            if new_inputs.contains(op) {
                                Some(true)
                            } else if let Some(x) = new_outputs.get(&op.hash) {
                                if op.index < (*x as u32) {
                                    Some(false)
                                } else {
                                    Some(true)
                                }
                            } else if old_outputs.contains(&op.hash) {
                                None
                            } else {
                                chain_state
                                    .is_spent(op)
                                    .map(|x| x && !old_inputs.contains(op))
                            }
                        })
                    })
                    .is_ok()
            {
                push_new(b, &mut new_inputs, &mut new_outputs);
                ext.valid = Some(true);
            } else {
                found_error = true;
                ext.valid = Some(false);
            }
        }

        // update exts
        for (ext, b) in fork.open_exts.iter().zip(fork.new_blocks.iter()).rev() {
            self.shared
                .store()
                .insert_block_ext(batch, &b.header().hash(), ext);
        }

        if found_error {
            return Err(SharedError::InvalidTransaction);
        }

        self.update_index(batch, &fork.old_blocks, &fork.new_blocks);

        let old_inputs: Vec<OutPoint> = old_inputs.into_iter().collect();
        let old_outputs: Vec<H256> = old_outputs.into_iter().collect();
        let new_inputs: Vec<OutPoint> = new_inputs.into_iter().collect();
        let new_outputs: Vec<(H256, usize)> = new_outputs.into_iter().collect();

        Ok((
            TxoSetDiff {
                old_inputs,
                old_outputs,
                new_inputs,
                new_outputs,
            },
            fork,
        ))
    }

    fn print_chain(&self, len: u64) {
        debug!(target: "chain", "Chain {{");

        let tip = self.shared.chain_state().read().tip_number();
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
