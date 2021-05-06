use ckb_types::{core, packed};
use ckb_util::shrink_to_fit;
use dashmap::DashMap;
use std::collections::{HashMap, VecDeque};

pub type ParentHash = packed::Byte32;
const SHRINK_THRESHOLD: usize = 100;

// NOTE: Never use `LruCache` as container. We have to ensure synchronizing between
// orphan_block_pool and block_status_map, but `LruCache` would prune old items implicitly.
#[derive(Default)]
pub struct OrphanBlockPool {
    blocks: DashMap<ParentHash, HashMap<packed::Byte32, core::BlockView>>,
    parents: DashMap<packed::Byte32, ParentHash>,
}

impl OrphanBlockPool {
    pub fn with_capacity(capacity: usize) -> Self {
        OrphanBlockPool {
            blocks: DashMap::with_capacity(capacity),
            parents: DashMap::new(),
        }
    }

    /// Insert orphaned block, for which we have already requested its parent block
    pub fn insert(&self, block: core::BlockView) {
        let hash = block.header().hash();
        let parent_hash = block.data().header().raw().parent_hash();
        self.blocks
            .entry(parent_hash.clone())
            .or_insert_with(HashMap::default)
            .insert(hash.clone(), block);
        self.parents.insert(hash, parent_hash);
    }

    pub fn remove_blocks_by_parent(&self, hash: &packed::Byte32) -> Vec<core::BlockView> {
        let mut queue: VecDeque<packed::Byte32> = VecDeque::new();
        queue.push_back(hash.to_owned());

        let mut removed: Vec<core::BlockView> = Vec::new();
        while let Some(parent_hash) = queue.pop_front() {
            if let Some((_, orphaned)) = self.blocks.remove(&parent_hash) {
                let (hashes, blocks): (Vec<_>, Vec<_>) = orphaned.into_iter().unzip();
                for hash in hashes.iter() {
                    self.parents.remove(hash);
                }
                queue.extend(hashes);
                removed.extend(blocks);
            }
        }

        shrink_to_fit!(self.blocks, SHRINK_THRESHOLD);
        shrink_to_fit!(self.parents, SHRINK_THRESHOLD);
        removed
    }

    pub fn get_block(&self, hash: &packed::Byte32) -> Option<core::BlockView> {
        self.parents.get(hash).and_then(|parent_hash| {
            self.blocks
                .get(&parent_hash)
                .and_then(|blocks| blocks.get(hash).cloned())
        })
    }

    pub fn len(&self) -> usize {
        self.parents.len()
    }
}

#[cfg(test)]
mod tests {
    use super::OrphanBlockPool;
    use ckb_chain_spec::consensus::ConsensusBuilder;
    use ckb_types::core::{BlockBuilder, BlockView, HeaderView};
    use ckb_types::prelude::*;
    use faketime::unix_time_as_millis;
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::thread;

    fn gen_block(parent_header: &HeaderView) -> BlockView {
        BlockBuilder::default()
            .parent_hash(parent_header.hash())
            .timestamp(unix_time_as_millis().pack())
            .number((parent_header.number() + 1).pack())
            .nonce((parent_header.nonce() + 1).pack())
            .build()
    }

    #[test]
    fn test_remove_blocks_by_parent() {
        let consensus = ConsensusBuilder::default().build();
        let block_number = 200;
        let mut blocks = Vec::new();
        let mut parent = consensus.genesis_block().header();
        let pool = OrphanBlockPool::with_capacity(200);
        for _ in 1..block_number {
            let new_block = gen_block(&parent);
            blocks.push(new_block.clone());
            pool.insert(new_block.clone());
            parent = new_block.header();
        }

        let orphan = pool.remove_blocks_by_parent(&consensus.genesis_block().hash());
        let orphan: HashSet<BlockView> = orphan.into_iter().collect();
        let block: HashSet<BlockView> = blocks.into_iter().collect();
        assert_eq!(orphan, block)
    }

    #[test]
    fn test_remove_blocks_by_parent_and_get_block_should_not_deadlock() {
        let consensus = ConsensusBuilder::default().build();
        let pool = OrphanBlockPool::with_capacity(1024);
        let mut header = consensus.genesis_block().header();
        let mut hashes = Vec::new();
        for _ in 1..1024 {
            let new_block = gen_block(&header);
            pool.insert(new_block.clone());
            header = new_block.header();
            hashes.push(header.hash());
        }

        let pool_arc1 = Arc::new(pool);
        let pool_arc2 = Arc::clone(&pool_arc1);

        let thread1 = thread::spawn(move || {
            pool_arc1.remove_blocks_by_parent(&consensus.genesis_block().hash());
        });

        for hash in hashes.iter().rev() {
            pool_arc2.get_block(hash);
        }

        thread1.join().unwrap();
    }
}
