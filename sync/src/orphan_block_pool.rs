use ckb_types::{core, packed};
use ckb_util::shrink_to_fit;
use ckb_util::RwLock;
use std::collections::{HashMap, HashSet, VecDeque};

pub type ParentHash = packed::Byte32;
const SHRINK_THRESHOLD: usize = 100;

// NOTE: Never use `LruCache` as container. We have to ensure synchronizing between
// orphan_block_pool and block_status_map, but `LruCache` would prune old items implicitly.
#[derive(Default)]
pub struct OrphanBlockPool {
    // Group by blocks in the pool by the parent hash.
    blocks: RwLock<HashMap<ParentHash, HashMap<packed::Byte32, core::BlockView>>>,
    // The map tells the parent hash when given the hash of a block in the pool.
    //
    // The block is in the orphan pool if and only if the block hash exists as a key in this map.
    parents: RwLock<HashMap<packed::Byte32, ParentHash>>,
    // Leaders are blocks not in the orphan pool but having at least a child in the pool.
    leaders: RwLock<HashSet<ParentHash>>,
}

impl OrphanBlockPool {
    pub fn with_capacity(capacity: usize) -> Self {
        OrphanBlockPool {
            blocks: RwLock::new(HashMap::with_capacity_and_hasher(
                capacity,
                Default::default(),
            )),
            parents: RwLock::new(Default::default()),
            leaders: RwLock::new(Default::default()),
        }
    }

    /// Insert orphaned block, for which we have already requested its parent block
    pub fn insert(&self, block: core::BlockView) {
        let mut blocks_map = self.blocks.write();
        let mut parents_map = self.parents.write();
        let mut leaders_set = self.leaders.write();

        let hash = block.header().hash();
        let parent_hash = block.data().header().raw().parent_hash();
        blocks_map
            .entry(parent_hash.clone())
            .or_insert_with(HashMap::default)
            .insert(hash.clone(), block);
        // Out-of-order insertion needs to be deduplicated
        leaders_set.remove(&hash);
        // It is a possible optimization to make the judgment in advance,
        // because the parent of the block must not be equal to its own hash,
        // so we can judge first, which may reduce one arc clone
        if !parents_map.contains_key(&parent_hash) {
            // Block referenced by `parent_hash` is not in the pool,
            // and it has at least one child, the new inserted block, so add it to leaders.
            leaders_set.insert(parent_hash.clone());
        }
        parents_map.insert(hash, parent_hash);
    }

    pub fn remove_blocks_by_parent(&self, parent_hash: &ParentHash) -> Vec<core::BlockView> {
        let mut blocks_map = self.blocks.write();
        let mut parents_map = self.parents.write();
        let mut leaders_set = self.leaders.write();

        // try remove leaders first
        if !leaders_set.remove(parent_hash) {
            return Vec::new();
        }

        let mut queue: VecDeque<packed::Byte32> = VecDeque::new();
        queue.push_back(parent_hash.to_owned());

        let mut removed: Vec<core::BlockView> = Vec::new();
        while let Some(parent_hash) = queue.pop_front() {
            if let Some(orphaned) = blocks_map.remove(&parent_hash) {
                let (hashes, blocks): (Vec<_>, Vec<_>) = orphaned.into_iter().unzip();
                for hash in hashes.iter() {
                    parents_map.remove(hash);
                }
                queue.extend(hashes);
                removed.extend(blocks);
            }
        }

        shrink_to_fit!(blocks_map, SHRINK_THRESHOLD);
        shrink_to_fit!(parents_map, SHRINK_THRESHOLD);
        shrink_to_fit!(leaders_set, SHRINK_THRESHOLD);
        removed
    }

    pub fn get_block(&self, hash: &packed::Byte32) -> Option<core::BlockView> {
        // acquire the `blocks` read lock first, guarantee ordering of acquisition is same as `remove_blocks_by_parent`, avoids deadlocking
        let guard = self.blocks.read();
        self.parents.read().get(hash).and_then(|parent_hash| {
            guard
                .get(parent_hash)
                .and_then(|value| value.get(hash).cloned())
        })
    }

    pub fn len(&self) -> usize {
        self.parents.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clone_leaders(&self) -> Vec<ParentHash> {
        self.leaders.read().iter().cloned().collect::<Vec<_>>()
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
        let orphan_set: HashSet<BlockView> = orphan.into_iter().collect();
        let blocks_set: HashSet<BlockView> = blocks.into_iter().collect();
        assert_eq!(orphan_set, blocks_set)
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

    #[test]
    fn test_leaders() {
        let consensus = ConsensusBuilder::default().build();
        let block_number = 20;
        let mut blocks = Vec::new();
        let mut parent = consensus.genesis_block().header();
        let pool = OrphanBlockPool::with_capacity(20);
        for i in 0..block_number - 1 {
            let new_block = gen_block(&parent);
            blocks.push(new_block.clone());
            parent = new_block.header();
            if i % 5 != 0 {
                pool.insert(new_block.clone());
            }
        }

        {
            assert_eq!(pool.len(), 15);
            assert_eq!(pool.leaders.read().len(), 4);
        }

        {
            pool.insert(blocks[5].clone());
            assert_eq!(pool.len(), 16);
            assert_eq!(pool.leaders.read().len(), 3);
        }

        {
            pool.insert(blocks[10].clone());
            assert_eq!(pool.len(), 17);
            assert_eq!(pool.leaders.read().len(), 2);
        }

        {
            // index 0 doesn't in the orphan pool, so do nothing
            let orphan = pool.remove_blocks_by_parent(&consensus.genesis_block().hash());
            assert!(orphan.is_empty());
            assert_eq!(pool.len(), 17);
            assert_eq!(pool.leaders.read().len(), 2);
        }

        {
            pool.insert(blocks[0].clone());
            assert_eq!(pool.len(), 18);
            assert_eq!(pool.leaders.read().len(), 2);
        }

        let orphan = {
            let orphan = pool.remove_blocks_by_parent(&consensus.genesis_block().hash());
            assert_eq!(pool.len(), 3);
            assert_eq!(pool.leaders.read().len(), 1);
            orphan
        };

        {
            pool.insert(blocks[15].clone());
            assert_eq!(pool.len(), 4);
            assert_eq!(pool.leaders.read().len(), 1);
        }

        let orphan_1 = pool.remove_blocks_by_parent(&blocks[14].hash());

        let orphan_set: HashSet<BlockView> =
            orphan.into_iter().chain(orphan_1.into_iter()).collect();
        let blocks_set: HashSet<BlockView> = blocks.into_iter().collect();
        assert_eq!(orphan_set, blocks_set);
        assert_eq!(pool.len(), 0);
        assert_eq!(pool.leaders.read().len(), 0);
    }
}
