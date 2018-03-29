use super::queue::{BlockQueue, BlockQueueState, HeaderQueue, Position};
use bigint::H256;
use core::block::{Block, Header};
use core::cell::{CellProvider, CellState};
use core::transaction::OutPoint;
use nervos_chain::chain::ChainClient;
use std::sync::Arc;
use util::RwLock;

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum BlockState {
    Unknown,
    Scheduled,
    Requested,
    Verifying,
    Stored,
    DeadEnd,
}

impl From<BlockQueueState> for BlockState {
    fn from(state: BlockQueueState) -> Self {
        match state {
            BlockQueueState::Scheduled => BlockState::Scheduled,
            BlockQueueState::Requested => BlockState::Requested,
            BlockQueueState::Verifying => BlockState::Verifying,
        }
    }
}

pub struct Chain {
    pub chain_store: Arc<ChainClient>,
    pub block_queue: RwLock<BlockQueue>,
    pub header_queue: RwLock<HeaderQueue>,
}

impl CellProvider for Chain {
    fn cell(&self, out_point: &OutPoint) -> CellState {
        let index = out_point.index as usize;
        if let Some(meta) = self.chain_store.get_transaction_meta(&out_point.hash) {
            if index < meta.spent_at.len() {
                if !meta.is_spent(index) {
                    let mut transaction = self.chain_store
                        .get_transaction(&out_point.hash)
                        .expect("transaction must exist");
                    return CellState::Head(transaction.outputs.swap_remove(index));
                } else {
                    return CellState::Tail;
                }
            }
        }
        CellState::Unknown
    }
}

impl Chain {
    pub fn new(store: &Arc<ChainClient>) -> Chain {
        let head = store.head_header();
        Chain {
            chain_store: Arc::clone(store),
            block_queue: RwLock::new(BlockQueue::new()),
            header_queue: RwLock::new(HeaderQueue::new(head.hash())),
        }
    }

    pub fn block_state(&self, hash: &H256) -> BlockState {
        let guard = self.block_queue.read();
        match guard.contains(hash) {
            Some(queue_state) => BlockState::from(queue_state),
            None => if self.chain_store.block_header(hash).is_some() {
                BlockState::Stored
            // } else if self.dead_end_blocks.contains(hash) {
            //     BlockState::DeadEnd
            } else {
                BlockState::Unknown
            },
        }
    }

    pub fn block_hash(&self, height: u64) -> Option<H256> {
        let best_storage_height = self.chain_store.head_header().height;
        if height <= best_storage_height {
            self.chain_store.block_hash(height)
        } else {
            // we try to keep these in order, but they are probably not
            self.block_queue
                .read()
                .at((height - best_storage_height) as usize)
        }
    }

    pub fn block_header(&self, height: u64) -> Option<Header> {
        let best_storage_height = self.chain_store.head_header().height;
        if height <= best_storage_height {
            self.chain_store
                .block_hash(height)
                .and_then(|hash| self.chain_store.block_header(&hash))
        } else {
            self.header_queue.read().at(height - best_storage_height)
        }
    }

    pub fn schedule_blocks_headers(&self, headers: Vec<Header>) {
        let mut write_guard = self.block_queue.write();
        write_guard
            .scheduled
            .push_back_n(headers.iter().map(|h| h.hash).collect());
        self.header_queue.write().insert_n(headers);
    }

    pub fn request_blocks_hashes(&self, n: u32) -> Vec<H256> {
        let mut write_guard = self.block_queue.write();
        let scheduled = write_guard.scheduled.pop_front_n(n);
        write_guard.requested.push_back_n(scheduled.clone());
        scheduled
    }

    pub fn verify_header(&self, header: Header) {
        let mut write_guard = self.block_queue.write();
        write_guard.verifying.push_back(header.hash);
        self.header_queue.write().insert(header);
    }

    pub fn scheduled_len(&self) -> usize {
        self.block_queue.read().scheduled.len()
    }

    pub fn get_locator(&self) -> Vec<H256> {
        self.chain_store.get_locator()
    }

    pub fn forget_block_leave_header(&self, hash: &H256) -> Position {
        let mut write_guard = self.block_queue.write();
        match write_guard.verifying.remove(hash) {
            Position::None => match write_guard.requested.remove(hash) {
                Position::None => write_guard.scheduled.remove(hash),
                position => position,
            },
            position => position,
        }
    }

    /// Forget in-memory blocks, but leave their headers in the headers_chain (orphan queue)
    pub fn forget_blocks_leave_header(&self, hashes: &[H256]) {
        for hash in hashes {
            self.forget_block_leave_header(hash);
        }
    }

    /// Forget in-memory block
    pub fn forget_block(&self, hash: &H256) -> Position {
        self.header_queue.write().remove(hash);
        self.forget_block_leave_header(hash)
    }

    /// Forget in-memory blocks
    pub fn forget_blocks(&self, hashes: &[H256]) {
        for hash in hashes {
            self.forget_block(hash);
        }
    }

    pub fn insert_block(&self, block: &Block) {
        let hash = block.hash();
        if self.chain_store.process_block(block).is_ok() {
            self.header_queue.write().block_inserted_to_storage(&hash);
            self.forget_block_leave_header(&hash);
        }
        //TODO: remove dup transaction from pool
    }
}
