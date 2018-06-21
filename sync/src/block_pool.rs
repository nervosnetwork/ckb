use bigint::H256;
use core::block::{Block, IndexedBlock};
use core::header::IndexedHeader;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};
use util::RwLock;

pub type BlockHash = H256;
pub type ParentHash = H256;

#[derive(Default)]
pub struct OrphanBlockPool {
    blocks: RwLock<HashMap<ParentHash, HashMap<BlockHash, Block>>>,
}

impl OrphanBlockPool {
    pub fn with_capacity(capacity: usize) -> Self {
        OrphanBlockPool {
            blocks: RwLock::new(HashMap::with_capacity(capacity)),
        }
    }

    /// Insert orphaned block, for which we have already requested its parent block
    pub fn insert(&self, block: IndexedBlock) {
        self.blocks
            .write()
            .entry(block.header.parent_hash)
            .or_insert_with(HashMap::new)
            .insert(block.hash(), block.into());
    }

    pub fn remove_blocks_by_parent(&self, hash: &H256) -> VecDeque<IndexedBlock> {
        let mut queue: VecDeque<H256> = VecDeque::new();
        queue.push_back(*hash);

        let mut removed: VecDeque<IndexedBlock> = VecDeque::new();
        while let Some(parent_hash) = queue.pop_front() {
            if let Entry::Occupied(entry) = self.blocks.write().entry(parent_hash) {
                let (_, orphaned) = entry.remove_entry();
                queue.extend(orphaned.keys().cloned());
                removed.extend(orphaned.into_iter().map(|(h, b)| {
                    let Block {
                        header,
                        transactions,
                    } = b;
                    let header = IndexedHeader::new(header, h);

                    IndexedBlock {
                        header,
                        transactions,
                    }
                }));
            }
        }
        removed
    }

    pub fn len(&self) -> usize {
        self.blocks.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
