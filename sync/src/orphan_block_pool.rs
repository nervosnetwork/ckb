use std::collections::{HashMap, VecDeque};
use std::ops::Deref;

use ckb_error::{Error, InternalErrorKind};
use ckb_types::prelude::ShouldBeOk;
use ckb_types::{core, packed};
use ckb_util::parking_lot::RwLock;
use ckb_util::shrink_to_fit;

/// alias name
pub type ParentHash = packed::Byte32;
/// alias name
pub type Hash = packed::Byte32;

const SHRINK_THRESHOLD: usize = 100;
const MAX_SUB_NODES_UPDATE_IN_ROOTOF: usize = 16;

/// RwLock of wrapper for OrphanBlockPool
pub struct OrphanBlockPool {
    inner: RwLock<InnerOrphanBlockPool>,
}

impl OrphanBlockPool {
    ///  init orphan block pool, reserve maps with specific capacity
    pub fn with_capacity(capacity: usize) -> Self {
        OrphanBlockPool {
            inner: RwLock::new(InnerOrphanBlockPool::with_capacity(capacity)),
        }
    }
    /// insert the orphan block into pool
    ///
    /// Return Ok() normally
    ///
    /// Return Error when inner data not sync
    pub fn insert(&self, block: core::BlockView) -> Result<(), Error> {
        self.inner.write().insert(block)
    }
    /// return vector of sub-node blocks beneath to the root hash
    ///
    /// return None if none beneath the root
    pub fn remove_blocks_by_parent(&self, root: &Hash) -> Vec<core::BlockView> {
        self.inner
            .write()
            .remove_blocks_by_parent(root)
            .unwrap_or_default()
    }
    /// return block stored in pool
    ///
    /// return None if not found in pool
    pub fn get_block(&self, hash: &Hash) -> Option<core::BlockView> {
        self.inner.read().get_block(hash)
    }
    /// is trees map is empty
    pub fn is_empty(&self) -> bool {
        self.inner.read().is_trees_empty()
    }
    /// the leaders count in trees
    pub fn len(&self) -> usize {
        self.inner.read().trees_len()
    }

    /// get leaders(key) in trees
    pub fn get_leaders(&self) -> Vec<Hash> {
        self.inner.read().get_leaders()
    }

    #[allow(dead_code)]
    /// debug tool to get length of trees
    fn trees_len(&self) -> usize {
        self.inner.read().trees_len()
    }
    #[allow(dead_code)]
    /// debug tool to get length of rootof
    fn root_of_len(&self) -> usize {
        self.inner.read().rootof_len()
    }
}

struct InnerOrphanBlockPool {
    // store root and sub-node block
    trees: HashMap<ParentHash, VecDeque<(Hash, core::BlockView)>>,
    // CP(child-parent) relationship between sub-nodes
    root_of: HashMap<Hash, ParentHash>,
}

fn internal_error(reason: String) -> Error {
    InternalErrorKind::System.other(reason).into()
}

impl InnerOrphanBlockPool {
    fn with_capacity(capacity: usize) -> Self {
        InnerOrphanBlockPool {
            trees: HashMap::with_capacity(capacity),
            root_of: HashMap::new(),
        }
    }

    /// root_of table contains direct child-parent relationship
    ///
    /// but we need to find the root recursively when given any child
    ///
    /// Return hash of the root hash
    fn find_root(&self, hash: &Hash) -> Hash {
        let mut p = hash.clone();
        loop {
            if let Some(v) = self.root_of.get(&p) {
                p = v.clone();
            } else {
                return p;
            }
        }
    }

    /// update specific sub-node's parent to root(hash)
    /// if sub-nodes' count less than LIMIT, we update their parent
    ///
    /// params:
    ///
    /// key: which tree's sub-nodes needs be be upgrade (in trees)
    ///
    /// root: update root
    fn update_root_in_rootof(&mut self, key: &Hash, root: &Hash) {
        let tree = self.trees.get(key).should_be_ok();
        if tree.iter().size_hint().0 < MAX_SUB_NODES_UPDATE_IN_ROOTOF {
            for (hash, _) in tree.iter() {
                *self.root_of.get_mut(hash).unwrap() = root.clone();
            }
        }
    }

    /// insert node block into orphan block pool
    ///
    /// there are 4 cases related with character of insert node
    ///
    /// 1. add leaf node
    ///
    /// 2. add separated root node
    ///
    /// 3. join two trees
    ///
    /// 4. update root
    ///
    /// params: block - block inserted
    ///
    /// Returns OK if none error occurs
    ///
    /// Returns Error or Panic if internal data not sync
    fn insert(&mut self, block: core::BlockView) -> Result<(), Error> {
        let hash = block.header().hash();
        let parent_hash = block.data().header().raw().parent_hash();

        // ignore duplicated block
        if self.root_of.contains_key(&hash) {
            return Ok(());
        }

        if self.root_of.contains_key(&parent_hash) {
            if self.trees.contains_key(&hash) {
                // case3: join two trees
                self.join_two_trees(hash, parent_hash, block);
            } else if !self.root_of.contains_key(&hash) {
                // case1: add leaf node
                self.add_leaf_node(hash, parent_hash, block);
            } else {
                return Err(internal_error(String::from(
                    "insert node inner state error!",
                )));
            }
        } else if self.trees.contains_key(&hash) {
            // case4: update root
            self.update_root(hash, parent_hash, block);
        } else if !self.trees.contains_key(&hash) && !self.trees.contains_key(&parent_hash) {
            // case2: add separated root node, add new entry in root_of and trees
            self.add_separated_root_node(hash, parent_hash, block);
        } else if !self.trees.contains_key(&hash) && self.trees.contains_key(&parent_hash) {
            // that means duplicate block occurs, for now we can't tell which is valid
            // so we append that block at end of deque
            self.trees
                .get_mut(&parent_hash)
                .should_be_ok()
                .push_back((hash, block));
        } else {
            return Err(internal_error(String::from(
                "insert node inner state error!",
            )));
        }

        Ok(())
    }

    /// Note: without input check, this function only used in insert()
    fn join_two_trees(&mut self, hash: Hash, parent_hash: ParentHash, block: core::BlockView) {
        let root = self.find_root(&parent_hash);
        // update root_of
        self.root_of.insert(hash.clone(), parent_hash);
        self.update_root_in_rootof(&hash, &root);

        let mut new_vec = VecDeque::new();
        let tree2 = self.trees.get_mut(&hash).should_be_ok();
        new_vec.push_front((hash.clone(), block));
        new_vec.append(tree2);
        let tree1 = self.trees.get_mut(&root).should_be_ok();
        tree1.append(&mut new_vec);

        self.trees.remove(&hash);
    }

    /// Note: without input check, this function only used in insert()
    fn add_leaf_node(&mut self, hash: Hash, parent_hash: ParentHash, block: core::BlockView) {
        let root = self.find_root(&parent_hash);
        if let Some(deq) = self.trees.get_mut(&root) {
            self.root_of.insert(hash.clone(), root.clone());
            deq.push_back((hash, block));
        } else {
            // TODO: re-construct record in trees instead of panic
            panic!("find the root in root_of, but trees lack of root record!");
        }
    }

    /// Note: without input check, this function only used in insert()
    fn update_root(&mut self, hash: Hash, parent_hash: ParentHash, block: core::BlockView) {
        self.root_of.insert(hash.clone(), parent_hash.clone());
        self.update_root_in_rootof(&hash, &parent_hash);
        let mut v = VecDeque::new();
        v.append(self.trees.get_mut(&hash).unwrap());
        self.trees.remove(&hash);
        v.push_front((hash, block));
        self.trees.insert(parent_hash, v);
    }

    /// Note: without input check, this function only used in insert()
    fn add_separated_root_node(
        &mut self,
        hash: Hash,
        parent_hash: ParentHash,
        block: core::BlockView,
    ) {
        self.root_of.insert(hash.clone(), parent_hash.clone());
        let mut v = VecDeque::new();
        v.push_back((hash, block));
        self.trees.insert(parent_hash, v);
    }

    /// collect all blocks of sub-nodes via input root-node hash
    /// remove all sub-nodes info in trees and root_of
    ///
    /// params: parent_hash: root node hash
    ///
    /// Returns Some(Vec) all sub-nodes' blocks
    ///
    /// Return None if input root-node hash not exists in trees(not real root node?)
    fn remove_blocks_by_parent(&mut self, root: &Hash) -> Option<Vec<core::BlockView>> {
        if let Some(v) = self.trees.get_mut(root) {
            let mut vec = Vec::with_capacity(v.len());
            for (hash, block) in v.iter() {
                vec.push(block.clone());
                self.root_of.remove(hash);
            }

            self.trees.remove(root);
            shrink_to_fit!(self.trees, SHRINK_THRESHOLD);
            shrink_to_fit!(self.root_of, SHRINK_THRESHOLD);
            Some(vec)
        } else {
            None
        }
    }

    /// get block by input node hash
    ///
    /// params: hash - node hash
    ///
    /// Returns Some(block) if the node is found
    ///
    /// Returns None if the node is not found
    fn get_block(&self, hash: &Hash) -> Option<core::BlockView> {
        self.root_of.get(hash).and_then(|relationship| {
            let parent_hash = self.find_root(relationship);
            self.trees.get(&parent_hash).and_then(|v| {
                for (h, b) in v.iter() {
                    if *h.deref() == (*hash) {
                        return Some(b.clone());
                    }
                }
                None
            })
        })
    }

    /// get leaders(key) from trees
    pub fn get_leaders(&self) -> Vec<Hash> {
        let mut result = vec![];
        for key in self.trees.keys() {
            result.push(key.clone());
        }
        result
    }

    fn trees_len(&self) -> usize {
        self.trees.len()
    }
    fn rootof_len(&self) -> usize {
        self.root_of.len()
    }
    fn is_trees_empty(&self) -> bool {
        self.trees.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::thread;

    use faketime::unix_time_as_millis;

    use ckb_chain_spec::consensus::ConsensusBuilder;
    use ckb_types::core::{BlockBuilder, BlockView, HeaderView};
    use ckb_types::prelude::*;

    use super::OrphanBlockPool;

    fn gen_block(parent_header: &HeaderView) -> BlockView {
        BlockBuilder::default()
            .parent_hash(parent_header.hash())
            .timestamp(unix_time_as_millis().pack())
            .number((parent_header.number() + 1).pack())
            .nonce((parent_header.nonce() + 1).pack())
            .build()
    }

    #[test]
    /// generate 200 blocks, and get all blocks by input genesis block.
    fn test_remove_blocks_by_parent() {
        let consensus = ConsensusBuilder::default().build();
        let block_number = 200;
        let mut blocks = Vec::new();
        let mut parent = consensus.genesis_block().header();
        let pool = OrphanBlockPool::with_capacity(200);
        for _ in 1..block_number {
            let new_block = gen_block(&parent);
            blocks.push(new_block.clone());
            pool.insert(new_block.clone()).expect("insert error");
            parent = new_block.header();
        }

        let orphan = pool.remove_blocks_by_parent(&consensus.genesis_block().hash());
        let orphan_set: HashSet<BlockView> = orphan.into_iter().collect();
        let blocks_set: HashSet<BlockView> = blocks.into_iter().collect();
        assert_eq!(orphan_set, blocks_set)
    }

    #[test]
    /// generate 2 blocks(valid and invalid), their parents all are genesis.
    /// test if get both blocks retrieved by input genesis block.
    fn test_duplicated_blocks() {
        let consensus = ConsensusBuilder::default().build();
        let parent = consensus.genesis_block().header();
        let pool = OrphanBlockPool::with_capacity(200);

        let valid_block = gen_block(&parent);
        pool.insert(valid_block).expect("insert error");

        let invalid_block = BlockBuilder::default()
            .parent_hash(parent.hash())
            .timestamp(unix_time_as_millis().pack())
            .number((1000).pack())
            .nonce((parent.nonce() + 1).pack())
            .build();

        pool.insert(invalid_block).expect("insert error");

        let orphan = pool.remove_blocks_by_parent(&consensus.genesis_block().hash());
        assert_eq!(orphan.len(), 2);
    }

    #[test]
    /// generate 1024 blocks and do remove and insert concurrently
    fn test_remove_blocks_by_parent_and_get_block_should_not_deadlock() {
        let consensus = ConsensusBuilder::default().build();
        let pool = OrphanBlockPool::with_capacity(1024);
        let mut header = consensus.genesis_block().header();
        let mut hashes = Vec::new();
        for _ in 1..1024 {
            let new_block = gen_block(&header);
            pool.insert(new_block.clone()).expect("insert error");
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
    ///generate 19 blocks(0..=18) and put 4 blocks in leader, 15 blocks in pool
    fn test_trees() {
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
                pool.insert(new_block.clone()).expect("insert error");
            }
        }

        assert_eq!(pool.trees_len(), 4);
        assert_eq!(pool.root_of_len(), 15);

        assert!(pool.insert(blocks[5].clone()).is_ok());
        assert_eq!(pool.root_of_len(), 16);
        assert_eq!(pool.trees_len(), 3);

        assert!(pool.insert(blocks[10].clone()).is_ok());
        assert_eq!(pool.root_of_len(), 17);
        assert_eq!(pool.trees_len(), 2);

        // block 0 doesn't in the pool, so do nothing
        assert_eq!(
            pool.remove_blocks_by_parent(&consensus.genesis_block().hash())
                .len(),
            0
        );
        assert_eq!(pool.root_of_len(), 17);
        assert_eq!(pool.trees_len(), 2);

        assert!(pool.insert(blocks[0].clone()).is_ok());
        assert_eq!(pool.root_of_len(), 18);
        assert_eq!(pool.trees_len(), 2);

        assert_eq!(
            pool.remove_blocks_by_parent(&consensus.genesis_block().hash())
                .len(),
            15
        );
        assert_eq!(pool.root_of_len(), 3);
        assert_eq!(pool.trees_len(), 1);

        assert!(pool.insert(blocks[15].clone()).is_ok());
        assert_eq!(pool.root_of_len(), 4);
        assert_eq!(pool.trees_len(), 1);

        assert_eq!(pool.remove_blocks_by_parent(&blocks[14].hash()).len(), 4);
        assert_eq!(pool.root_of_len(), 0);
        assert_eq!(pool.trees_len(), 0);
    }
}
