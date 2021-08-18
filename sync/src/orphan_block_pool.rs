use ckb_logger::debug;
use ckb_types::{core, packed};
use ckb_util::{parking_lot::RwLock, shrink_to_fit};
use std::collections::{HashMap, HashSet, VecDeque};

pub type ParentHash = packed::Byte32;
const SHRINK_THRESHOLD: usize = 100;

#[derive(Default)]
struct InnerPool {
    // Group by blocks in the pool by the parent hash.
    blocks: HashMap<ParentHash, HashMap<packed::Byte32, core::BlockView>>,
    // The map tells the parent hash when given the hash of a block in the pool.
    //
    // The block is in the orphan pool if and only if the block hash exists as a key in this map.
    parents: HashMap<packed::Byte32, ParentHash>,
    // Leaders are blocks not in the orphan pool but having at least a child in the pool.
    leaders: HashSet<ParentHash>,
}

impl InnerPool {
    fn with_capacity(capacity: usize) -> Self {
        InnerPool {
            blocks: HashMap::with_capacity(capacity),
            parents: HashMap::new(),
            leaders: HashSet::new(),
        }
    }

    fn insert(&mut self, block: core::BlockView) {
        let hash = block.header().hash();
        let parent_hash = block.data().header().raw().parent_hash();
        self.blocks
            .entry(parent_hash.clone())
            .or_insert_with(HashMap::default)
            .insert(hash.clone(), block);
        // Out-of-order insertion needs to be deduplicated
        self.leaders.remove(&hash);
        // It is a possible optimization to make the judgment in advance,
        // because the parent of the block must not be equal to its own hash,
        // so we can judge first, which may reduce one arc clone
        if !self.parents.contains_key(&parent_hash) {
            // Block referenced by `parent_hash` is not in the pool,
            // and it has at least one child, the new inserted block, so add it to leaders.
            self.leaders.insert(parent_hash.clone());
        }
        self.parents.insert(hash, parent_hash);
    }

    pub fn remove_blocks_by_parent(&mut self, parent_hash: &ParentHash) -> Vec<core::BlockView> {
        // try remove leaders first
        if !self.leaders.remove(parent_hash) {
            return Vec::new();
        }

        let mut queue: VecDeque<packed::Byte32> = VecDeque::new();
        queue.push_back(parent_hash.to_owned());

        let mut removed: Vec<core::BlockView> = Vec::new();
        while let Some(parent_hash) = queue.pop_front() {
            if let Some(orphaned) = self.blocks.remove(&parent_hash) {
                let (hashes, blocks): (Vec<_>, Vec<_>) = orphaned.into_iter().unzip();
                for hash in hashes.iter() {
                    self.parents.remove(hash);
                }
                queue.extend(hashes);
                removed.extend(blocks);
            }
        }

        debug!("orphan pool pop chain len: {}", removed.len());
        debug_assert_ne!(
            removed.len(),
            0,
            "orphan pool removed list must not be zero"
        );

        shrink_to_fit!(self.blocks, SHRINK_THRESHOLD);
        shrink_to_fit!(self.parents, SHRINK_THRESHOLD);
        shrink_to_fit!(self.leaders, SHRINK_THRESHOLD);
        removed
    }

    pub fn get_block(&self, hash: &packed::Byte32) -> Option<core::BlockView> {
        self.parents.get(hash).and_then(|parent_hash| {
            self.blocks
                .get(&parent_hash)
                .and_then(|blocks| blocks.get(hash).cloned())
        })
    }
}

// NOTE: Never use `LruCache` as container. We have to ensure synchronizing between
// orphan_block_pool and block_status_map, but `LruCache` would prune old items implicitly.
// RwLock ensures the consistency between maps. Using multiple concurrent maps does not work here.
#[derive(Default)]
pub struct OrphanBlockPool {
    inner: RwLock<InnerPool>,
}

impl OrphanBlockPool {
    pub fn with_capacity(capacity: usize) -> Self {
        OrphanBlockPool {
            inner: RwLock::new(InnerPool::with_capacity(capacity)),
        }
    }

    /// Insert orphaned block, for which we have already requested its parent block
    pub fn insert(&self, block: core::BlockView) {
        self.inner.write().insert(block);
    }

    pub fn remove_blocks_by_parent(&self, parent_hash: &ParentHash) -> Vec<core::BlockView> {
        self.inner.write().remove_blocks_by_parent(parent_hash)
    }

    pub fn get_block(&self, hash: &packed::Byte32) -> Option<core::BlockView> {
        self.inner.read().get_block(hash)
    }

    pub fn len(&self) -> usize {
        self.inner.read().parents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clone_leaders(&self) -> Vec<ParentHash> {
        self.inner.read().leaders.iter().cloned().collect()
    }

    #[cfg(test)]
    fn leaders_len(&self) -> usize {
        self.inner.read().leaders.len()
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

        assert_eq!(pool.len(), 15);
        assert_eq!(pool.leaders_len(), 4);

        pool.insert(blocks[5].clone());
        assert_eq!(pool.len(), 16);
        assert_eq!(pool.leaders_len(), 3);

        pool.insert(blocks[10].clone());
        assert_eq!(pool.len(), 17);
        assert_eq!(pool.leaders_len(), 2);

        // index 0 doesn't in the orphan pool, so do nothing
        let orphan = pool.remove_blocks_by_parent(&consensus.genesis_block().hash());
        assert!(orphan.is_empty());
        assert_eq!(pool.len(), 17);
        assert_eq!(pool.leaders_len(), 2);

        pool.insert(blocks[0].clone());
        assert_eq!(pool.len(), 18);
        assert_eq!(pool.leaders_len(), 2);

        let orphan = pool.remove_blocks_by_parent(&consensus.genesis_block().hash());
        assert_eq!(pool.len(), 3);
        assert_eq!(pool.leaders_len(), 1);

        pool.insert(blocks[15].clone());
        assert_eq!(pool.len(), 4);
        assert_eq!(pool.leaders_len(), 1);

        let orphan_1 = pool.remove_blocks_by_parent(&blocks[14].hash());

        let orphan_set: HashSet<BlockView> =
            orphan.into_iter().chain(orphan_1.into_iter()).collect();
        let blocks_set: HashSet<BlockView> = blocks.into_iter().collect();
        assert_eq!(orphan_set, blocks_set);
        assert_eq!(pool.len(), 0);
        assert_eq!(pool.leaders_len(), 0);
    }
}
