use ckb_memory_tracker::collections::{TracedHashMap, TracedTag};
use ckb_types::{core, packed};
use ckb_util::RwLock;
use std::collections::{HashMap, VecDeque};

pub type ParentHash = packed::Byte32;

// NOTE: Never use `LruCache` as container. We have to ensure synchronizing between
// orphan_block_pool and block_status_map, but `LruCache` would prune old items implicitly.
#[derive(Default)]
pub struct OrphanBlockPool {
    blocks: RwLock<TracedHashMap<ParentHash, HashMap<packed::Byte32, core::BlockView>>>,
    parents: RwLock<TracedHashMap<packed::Byte32, ParentHash>>,
}

impl OrphanBlockPool {
    pub fn with_capacity(capacity: usize) -> Self {
        TracedTag::push("blocks");
        let blocks = RwLock::new(TracedHashMap::with_capacity_and_hasher(
            capacity,
            Default::default(),
        ));
        TracedTag::replace_last("parents");
        let parents = RwLock::new(Default::default());
        TracedTag::pop();
        OrphanBlockPool { blocks, parents }
    }

    /// Insert orphaned block, for which we have already requested its parent block
    pub fn insert(&self, block: core::BlockView) {
        let hash = block.header().hash();
        let parent_hash = block.data().header().raw().parent_hash();
        self.blocks
            .write()
            .entry(parent_hash.clone())
            .or_insert_with(HashMap::default)
            .insert(hash.clone(), block);
        self.parents.write().insert(hash, parent_hash);
    }

    pub fn remove_blocks_by_parent(&self, hash: &packed::Byte32) -> Vec<core::BlockView> {
        let mut guard = self.blocks.write();
        let mut queue: VecDeque<packed::Byte32> = VecDeque::new();
        queue.push_back(hash.to_owned());

        let mut removed: Vec<core::BlockView> = Vec::new();
        while let Some(parent_hash) = queue.pop_front() {
            if let Some(orphaned) = guard.remove(&parent_hash) {
                let (hashes, blocks): (Vec<_>, Vec<_>) = orphaned.into_iter().unzip();
                let mut parents = self.parents.write();
                for hash in hashes.iter() {
                    parents.remove(hash);
                }
                queue.extend(hashes);
                removed.extend(blocks);
            }
        }
        removed
    }

    pub fn get_block(&self, hash: &packed::Byte32) -> Option<core::BlockView> {
        self.parents
            .write()
            .get(hash)
            .map(|parent_hash| {
                self.blocks
                    .write()
                    .get(parent_hash)
                    .map(|value| value.get(hash).cloned())
                    .unwrap_or(None)
            })
            .unwrap_or(None)
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
    use std::iter::FromIterator;

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
        let orphan: HashSet<BlockView> = HashSet::from_iter(orphan.into_iter());
        let block: HashSet<BlockView> = HashSet::from_iter(blocks.into_iter());
        assert_eq!(orphan, block)
    }
}
