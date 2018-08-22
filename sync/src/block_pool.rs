use bigint::H256;
use core::block::IndexedBlock;
use fnv::{FnvHashMap, FnvHashSet};
use std::collections::hash_map::Entry;
use std::collections::VecDeque;
use util::RwLock;

pub type BlockHash = H256;
pub type ParentHash = H256;

#[derive(Default)]
pub struct OrphanBlockPool {
    blocks: RwLock<FnvHashMap<ParentHash, FnvHashSet<IndexedBlock>>>,
}

impl OrphanBlockPool {
    pub fn with_capacity(capacity: usize) -> Self {
        OrphanBlockPool {
            blocks: RwLock::new(FnvHashMap::with_capacity_and_hasher(
                capacity,
                Default::default(),
            )),
        }
    }

    /// Insert orphaned block, for which we have already requested its parent block
    pub fn insert(&self, block: IndexedBlock) {
        self.blocks
            .write()
            .entry(block.header.parent_hash)
            .or_insert_with(FnvHashSet::default)
            .insert(block);
    }

    pub fn remove_blocks_by_parent(&self, hash: &H256) -> VecDeque<IndexedBlock> {
        let mut guard = self.blocks.write();
        let mut queue: VecDeque<H256> = VecDeque::new();
        queue.push_back(*hash);

        let mut removed: VecDeque<IndexedBlock> = VecDeque::new();
        while let Some(parent_hash) = queue.pop_front() {
            if let Entry::Occupied(entry) = guard.entry(parent_hash) {
                let (_, orphaned) = entry.remove_entry();
                queue.extend(orphaned.iter().map(|b| b.hash()));
                removed.extend(orphaned.into_iter());
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

#[cfg(test)]
mod tests {
    use super::*;
    use bigint::{H256, U256};
    use ckb_chain::consensus::Consensus;
    use ckb_time::now_ms;
    use core::header::Seal;
    use core::header::{Header, IndexedHeader, RawHeader};
    use std::collections::HashSet;
    use std::iter::FromIterator;

    fn gen_block(parent_header: IndexedHeader) -> IndexedBlock {
        let time = now_ms();
        let nonce = parent_header.seal.nonce + 1;
        let header = Header {
            raw: RawHeader {
                number: parent_header.number + 1,
                version: 0,
                parent_hash: parent_header.hash(),
                timestamp: time,
                txs_commit: H256::zero(),
                txs_proposal: H256::zero(),
                difficulty: U256::zero(),
                cellbase_id: H256::zero(),
                uncles_hash: H256::zero(),
            },
            seal: Seal {
                nonce,
                mix_hash: H256::from(nonce),
            },
        };

        IndexedBlock {
            header: header.into(),
            uncles: vec![],
            commit_transactions: vec![],
            proposal_transactions: vec![],
        }
    }

    #[test]
    fn test_remove_blocks_by_parent() {
        let consensus = Consensus::default();
        let block_number = 200;
        let mut blocks: Vec<IndexedBlock> = Vec::new();
        let mut parent = consensus.genesis_block().header.clone();
        let pool = OrphanBlockPool::with_capacity(200);
        for _ in 1..block_number {
            let new_block = gen_block(parent);
            blocks.push(new_block.clone());
            pool.insert(new_block.clone());
            parent = new_block.header;
        }

        let orphan = pool.remove_blocks_by_parent(&consensus.genesis_block().hash());
        let orphan: HashSet<IndexedBlock> = HashSet::from_iter(orphan.into_iter());
        let block: HashSet<IndexedBlock> = HashSet::from_iter(blocks.into_iter());
        assert_eq!(orphan, block)
    }
}
