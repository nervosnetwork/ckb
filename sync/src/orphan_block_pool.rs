use ckb_logger::debug;
use ckb_types::core::EpochNumber;
use ckb_types::{core, packed};
use ckb_util::{parking_lot::RwLock, shrink_to_fit};
use std::collections::{HashMap, HashSet, VecDeque};

#[doc(hidden)]
pub type ParentHash = packed::Byte32;
const SHRINK_THRESHOLD: usize = 100;
const EXPIRED_EPOCH: u64 = 6;

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

    /// cleanup expired blocks(epoch + EXPIRED_EPOCH < tip_epoch)
    pub fn clean_expired_blocks(&mut self, tip_epoch: EpochNumber) -> Vec<packed::Byte32> {
        let mut result = vec![];

        for hash in self.leaders.clone().iter() {
            if self.need_clean(hash, tip_epoch) {
                // remove items in orphan pool and return hash to callee(clean header map)
                let descendants = self.remove_blocks_by_parent(&hash);
                result.extend(descendants.iter().map(|block| block.hash()));
            }
        }
        result
    }

    /// get 1st block belongs to that parent and check if it's expired block
    fn need_clean(&self, parent_hash: &packed::Byte32, tip_epoch: EpochNumber) -> bool {
        self.blocks
            .get(parent_hash)
            .and_then(|map| {
                map.iter()
                    .next()
                    .map(|(_, block)| block.header().epoch().number() + EXPIRED_EPOCH < tip_epoch)
            })
            .unwrap_or_default()
    }
}

// NOTE: Never use `LruCache` as container. We have to ensure synchronizing between
// orphan_block_pool and block_status_map, but `LruCache` would prune old items implicitly.
// RwLock ensures the consistency between maps. Using multiple concurrent maps does not work here.
/// Orphan Block Pool manages all orphan blocks, primarily support insert and remove orphan blocks
#[derive(Default)]
pub struct OrphanBlockPool {
    inner: RwLock<InnerPool>,
}

impl OrphanBlockPool {
    /// initialization api
    pub fn with_capacity(capacity: usize) -> Self {
        OrphanBlockPool {
            inner: RwLock::new(InnerPool::with_capacity(capacity)),
        }
    }

    /// Insert orphaned block, for which we have already requested its parent block
    pub fn insert(&self, block: core::BlockView) {
        self.inner.write().insert(block);
    }

    /// collect all children(orphan) blocks of parent_hash and return
    pub fn remove_blocks_by_parent(&self, parent_hash: &ParentHash) -> Vec<core::BlockView> {
        self.inner.write().remove_blocks_by_parent(parent_hash)
    }

    /// get block of input hash
    pub fn get_block(&self, hash: &packed::Byte32) -> Option<core::BlockView> {
        self.inner.read().get_block(hash)
    }

    /// clean epoch expired blocks
    pub fn clean_expired_blocks(&self, epoch: EpochNumber) -> Vec<packed::Byte32> {
        self.inner.write().clean_expired_blocks(epoch)
    }

    /// parents map length
    pub fn len(&self) -> usize {
        self.inner.read().parents.len()
    }

    /// is parents map empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// clone all leaders hash in leader map
    pub fn clone_leaders(&self) -> Vec<ParentHash> {
        self.inner.read().leaders.iter().cloned().collect()
    }

    #[cfg(test)]
    pub(crate) fn leaders_len(&self) -> usize {
        self.inner.read().leaders.len()
    }
}
