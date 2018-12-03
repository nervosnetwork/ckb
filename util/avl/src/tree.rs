#![allow(clippy::op_ref)]

use super::Result;
use ckb_core::transaction_meta::TransactionMeta;
use node::{get, insert, DBNode};
use numext_fixed_hash::H256;

use ckb_db::batch::{Batch, Col};
use ckb_db::kvdb::KeyValueDB;

use std::sync::Arc;

#[derive(Debug)]
pub struct NodeEntry {
    hash: Option<H256>,
    node: Option<Node>,
}

impl NodeEntry {
    pub fn new(hash: Option<H256>, node: Option<Node>) -> Self {
        NodeEntry { hash, node }
    }

    pub fn set_node(&mut self, node: Node) {
        self.node = Some(node);
    }

    pub fn set_hash(&mut self, hash: H256) {
        self.hash = Some(hash);
    }

    pub fn clear_hash(&mut self) {
        self.hash = None;
    }

    pub fn node(&self) -> &Option<Node> {
        &self.node
    }

    pub fn hash(&self) -> Option<&H256> {
        self.hash.as_ref()
    }
}

#[derive(Debug)]
pub struct Node {
    height: u32,
    key: H256,
    meta: Option<TransactionMeta>,
    children: [Option<Box<NodeEntry>>; 2],
}

impl Node {
    pub fn new(
        height: u32,
        key: H256,
        meta: Option<TransactionMeta>,
        children: [Option<Box<NodeEntry>>; 2],
    ) -> Self {
        Node {
            height,
            key,
            meta,
            children,
        }
    }

    pub fn height(&self) -> u32 {
        self.height
    }
}

pub struct AvlTree {
    root: Option<Box<NodeEntry>>,
    db: Arc<KeyValueDB>,
    col: Col,
}

impl AvlTree {
    pub fn new(db: Arc<KeyValueDB>, col: Col, r: H256) -> Self {
        let root = Some(Box::new(NodeEntry::new(Some(r), None)));
        AvlTree { root, db, col }
    }

    pub fn get_node(&self, h: &H256) -> Result<Node> {
        let data = get(&*self.db, self.col, h)?;

        match data {
            DBNode::Leaf(key, meta) => Ok(Node::new(1, key, Some(meta), [None, None])),
            DBNode::Branch(height, key, children) => {
                let left = Some(Box::new(NodeEntry::new(Some(children[0].clone()), None)));
                let right = Some(Box::new(NodeEntry::new(Some(children[1].clone()), None)));
                Ok(Node::new(height, key, None, [left, right]))
            }
        }
    }

    pub fn fetch_entry(&self, entry: &mut Option<Box<NodeEntry>>) -> Result<()> {
        if let Some(ref mut entry) = entry {
            if entry.node.is_none() {
                let hash = entry.hash().cloned().unwrap();
                let node = self.get_node(&hash)?;
                entry.set_node(node);
            }
        }

        Ok(())
    }

    // i1=0: left, i1=1: right
    fn rotate_successor(
        &mut self,
        mut k1: Option<Box<NodeEntry>>,
        i1: usize,
    ) -> Result<Option<Box<NodeEntry>>> {
        let i2 = i1 ^ 1;
        let mut k2 = self.take_child(&mut k1, i1)?;
        let h1 = self.child_height(&mut k2, i1)?;
        let h2 = self.child_height(&mut k2, i2)?;
        if h1 < h2 {
            let k3 = self.take_child(&mut k2, i2)?;
            k2 = self.rotate(k2, k3, i2)?;
        }
        Ok(self.rotate(k1, k2, i1)?)
    }

    // i1=0: left, i1=1: right
    fn rotate(
        &mut self,
        mut k1: Option<Box<NodeEntry>>,
        mut k2: Option<Box<NodeEntry>>,
        i1: usize,
    ) -> Result<Option<Box<NodeEntry>>> {
        let i2 = i1 ^ 1;
        {
            self.fetch_entry(&mut k1)?;
            self.fetch_entry(&mut k2)?;
            let entry2 = &mut k2.as_mut().unwrap();
            if let Some(ref mut node2) = entry2.node {
                {
                    let entry1 = &mut k1.as_mut().unwrap();
                    if let Some(ref mut node1) = entry1.node {
                        node2.height = node1.height;
                        node1.height -= 1;
                        node1.children[i1] = node2.children[i2].take();
                    } else {
                        unreachable!()
                    }
                }
                node2.children[i2] = k1;
            } else {
                unreachable!()
            }
        }
        Ok(k2)
    }

    fn rotate_if_necessary(
        &mut self,
        mut entry: Option<Box<NodeEntry>>,
        i1: usize,
        h1: u32,
    ) -> Result<Option<Box<NodeEntry>>> {
        let i2 = i1 ^ 1;
        let h2 = self.child_height(&mut entry, i2)?;

        if h1 == h2 + 2 {
            self.rotate_successor(entry, i1)
        } else {
            Ok(entry)
        }
    }

    // i=0: left, i=1: right
    fn take_child(
        &mut self,
        entry: &mut Option<Box<NodeEntry>>,
        i: usize,
    ) -> Result<Option<Box<NodeEntry>>> {
        self.fetch_entry(entry)?;
        if let Some(ref mut entry) = entry {
            if let Some(ref mut node) = entry.node {
                Ok(node.children[i].take())
            } else {
                unreachable!()
            }
        } else {
            unreachable!()
        }
    }

    fn child_height(&mut self, entry: &mut Option<Box<NodeEntry>>, i: usize) -> Result<u32> {
        self.fetch_entry(entry)?;
        if let Some(ref mut entry) = entry {
            if let Some(ref mut node) = entry.node {
                let child = &mut node.children[i];
                if child.is_none() {
                    Ok(0)
                } else {
                    self.fetch_entry(child)?;
                    if let Some(ref entry) = child {
                        if let Some(ref node) = entry.node {
                            Ok(node.height)
                        } else {
                            unreachable!()
                        }
                    } else {
                        unreachable!()
                    }
                }
            } else {
                unreachable!()
            }
        } else {
            unreachable!()
        }
    }

    fn insert_at(
        &mut self,
        mut entry: Option<Box<NodeEntry>>,
        key: H256,
        value: TransactionMeta,
    ) -> Result<(Option<Box<NodeEntry>>, u32, Option<TransactionMeta>)> {
        self.fetch_entry(&mut entry)?;
        let entry = entry.unwrap();
        let hash = entry.hash().cloned();
        let mut node = entry.node.unwrap();
        if node.meta.is_some() {
            if node.key == key {
                let old_val = node.meta;
                let node = Node::new(1, key, Some(value), [None, None]);
                let entry = Some(Box::new(NodeEntry::new(None, Some(node))));
                Ok((entry, 0, old_val))
            } else {
                let node1 = Node::new(1, key.clone(), Some(value), [None, None]);
                let entry1 = Some(Box::new(NodeEntry::new(None, Some(node1))));
                let node2 = Node::new(1, node.key.clone(), node.meta, [None, None]);
                let entry2 = Some(Box::new(NodeEntry::new(hash, Some(node2))));

                let (k0, left, right) = if &node.key < &key {
                    (key, entry2, entry1)
                } else {
                    (node.key, entry1, entry2)
                };

                let node3 = Node::new(2, k0, None, [left, right]);
                let entry3 = Some(Box::new(NodeEntry::new(None, Some(node3))));

                Ok((entry3, 2, None))
            }
        } else {
            let i = if &key < &node.key { 0 } else { 1 };
            let child = node.children[i].take();
            let (entry, h2, old_val) = self.insert_at(child, key, value)?;
            node.children[i] = entry;

            if h2 == node.height {
                let height = node.height;
                node.height += 1;
                let entry = Some(Box::new(NodeEntry::new(None, Some(node))));

                Ok((
                    self.rotate_if_necessary(entry, i, height)?,
                    height + 1,
                    old_val,
                ))
            } else {
                let height = node.height;
                let entry = Some(Box::new(NodeEntry::new(None, Some(node))));
                Ok((entry, height, old_val))
            }
        }
    }

    pub fn root_hash(&self) -> Option<H256> {
        if let Some(ref entry) = self.root {
            entry.hash().cloned()
        } else {
            None
        }
    }

    pub fn is_empty(&self) -> bool {
        self.root.is_none() || self.root_hash() == Some(H256::zero())
    }

    /// Insert transaction meta using txid as key.
    pub fn insert(&mut self, key: H256, value: TransactionMeta) -> Result<Option<TransactionMeta>> {
        if self.is_empty() {
            let node = Node::new(1, key, Some(value), [None, None]);
            self.root = Some(Box::new(NodeEntry::new(None, Some(node))));
            Ok(None)
        } else {
            let hash = self.root_hash();
            let root = self.root.take();
            match self.insert_at(root, key, value) {
                Ok((root, _, old)) => {
                    self.root = root;
                    Ok(old)
                }
                Err(x) => {
                    self.root = Some(Box::new(NodeEntry::new(hash, None)));
                    Err(x)
                }
            }
        }
    }

    pub fn update(&mut self, key: H256, index: usize) -> Result<bool> {
        if self.is_empty() {
            Ok(false)
        } else {
            let hash = self.root_hash();
            let root = self.root.take();

            match self.update_at(root, key, index) {
                Ok((root, change)) => {
                    self.root = root;
                    Ok(change)
                }
                Err(x) => {
                    self.root = Some(Box::new(NodeEntry::new(hash, None)));
                    Err(x)
                }
            }
        }
    }

    fn update_at(
        &mut self,
        mut entry: Option<Box<NodeEntry>>,
        key: H256,
        index: usize,
    ) -> Result<(Option<Box<NodeEntry>>, bool)> {
        self.fetch_entry(&mut entry)?;
        let entry = entry.unwrap();
        let hash = entry.hash().cloned();
        let mut node = entry.node.unwrap();

        if node.meta.is_some() {
            if node.key == key {
                let changed = if let Some(ref mut meta) = node.meta {
                    if meta.is_spent(index) {
                        false
                    } else {
                        meta.set_spent(index);
                        true
                    }
                } else {
                    unreachable!();
                };

                if changed {
                    Ok((Some(Box::new(NodeEntry::new(None, Some(node)))), true))
                } else {
                    Ok((Some(Box::new(NodeEntry::new(hash, Some(node)))), false))
                }
            } else {
                Ok((Some(Box::new(NodeEntry::new(hash, Some(node)))), false))
            }
        } else {
            let i = if key < node.key { 0 } else { 1 };
            let child = node.children[i].take();
            let (new_child, changed) = self.update_at(child, key, index)?;
            node.children[i] = new_child;

            if changed {
                Ok((Some(Box::new(NodeEntry::new(None, Some(node)))), true))
            } else {
                Ok((Some(Box::new(NodeEntry::new(hash, Some(node)))), false))
            }
        }
    }

    pub fn get(&mut self, key: &H256) -> Result<Option<TransactionMeta>> {
        if self.is_empty() {
            Ok(None)
        } else {
            let hash = self.root_hash();
            let root = self.root.take();

            match self.lookup(key, root) {
                Ok((root, value)) => {
                    self.root = root;
                    Ok(value)
                }
                Err(x) => {
                    self.root = Some(Box::new(NodeEntry::new(hash, None)));
                    Err(x)
                }
            }
        }
    }

    fn lookup(
        &mut self,
        key: &H256,
        mut entry: Option<Box<NodeEntry>>,
    ) -> Result<(Option<Box<NodeEntry>>, Option<TransactionMeta>)> {
        self.fetch_entry(&mut entry)?;

        let entry = entry.unwrap();
        let hash = entry.hash().cloned();
        let mut node = entry.node.unwrap();

        if node.meta.is_some() {
            if &node.key == key {
                let meta = node.meta.clone();
                let entry = Some(Box::new(NodeEntry::new(hash, Some(node))));
                Ok((entry, meta))
            } else {
                let entry = Some(Box::new(NodeEntry::new(hash, Some(node))));
                Ok((entry, None))
            }
        } else {
            let i = if key < &node.key { 0 } else { 1 };
            let child = node.children[i].take();
            let (new_child, meta) = self.lookup(key, child)?;
            node.children[i] = new_child;
            let entry = Some(Box::new(NodeEntry::new(hash, Some(node))));
            Ok((entry, meta))
        }
    }

    pub fn commit(&mut self, batch: &mut Batch) -> H256 {
        let root = self.root.take();
        let (hash, root) = self.commit_node(batch, root);
        self.root = root;
        hash
    }

    fn commit_node(
        &mut self,
        batch: &mut Batch,
        entry: Option<Box<NodeEntry>>,
    ) -> (H256, Option<Box<NodeEntry>>) {
        let entry = entry.unwrap();
        let hash = entry.hash().cloned();
        if let Some(h) = hash {
            (h, Some(entry))
        } else {
            let mut node = entry.node.unwrap();
            if node.meta.is_some() {
                let hash = insert(
                    self.col,
                    batch,
                    &DBNode::Leaf(node.key.clone(), node.meta.clone().unwrap()),
                );
                let entry = Some(Box::new(NodeEntry::new(Some(hash.clone()), Some(node))));
                (hash, entry)
            } else {
                let left = node.children[0].take();
                let right = node.children[1].take();
                let (hash1, entry1) = self.commit_node(batch, left);
                let (hash2, entry2) = self.commit_node(batch, right);
                let hash = insert(
                    self.col,
                    batch,
                    &DBNode::Branch(
                        node.height,
                        node.key.clone(),
                        [hash1.clone(), hash2.clone()],
                    ),
                );
                node.children[0] = entry1;
                node.children[1] = entry2;
                let entry = Some(Box::new(NodeEntry::new(Some(hash.clone()), Some(node))));
                (hash, entry)
            }
        }
    }

    pub fn reconstruct(&mut self, h: &H256) {
        if self.root_hash().as_ref() != Some(h) {
            self.root = Some(Box::new(NodeEntry::new(Some(h.clone()), None)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_db::memorydb::MemoryKeyValueDB;

    const TEST_COL: Col = Some(0);

    fn open_db() -> MemoryKeyValueDB {
        MemoryKeyValueDB::open(1)
    }

    fn build_tree(db: Arc<MemoryKeyValueDB>) -> AvlTree {
        let mut t = AvlTree::new(db, TEST_COL, H256::zero());

        let k1 = [1; 32].into();
        let k2 = [2; 32].into();
        let k3 = [3; 32].into();
        let k4 = [4; 32].into();

        t.insert(k1, TransactionMeta::new(1)).unwrap();
        t.insert(k2, TransactionMeta::new(2)).unwrap();
        t.insert(k3, TransactionMeta::new(3)).unwrap();
        t.insert(k4, TransactionMeta::new(4)).unwrap();

        t
    }

    fn build_and_commit_tree(db: Arc<MemoryKeyValueDB>) -> AvlTree {
        let mut tree = build_tree(db);
        let mut batch = Batch::default();
        tree.commit(&mut batch);
        tree.db.write(batch).expect("DB Error!");
        tree
    }

    #[test]
    fn init() {
        let db = Arc::new(open_db());
        let t = AvlTree::new(db, TEST_COL, H256::zero());
        assert_eq!(t.root_hash(), Some(H256::zero()));
    }

    #[test]
    fn test_insert_and_get() {
        let k4: H256 = [4; 32].into();
        let k5: H256 = [5; 32].into();

        let db = Arc::new(open_db());
        let mut t = build_tree(db);

        assert_eq!(t.get(&k4), Ok(Some(TransactionMeta::new(4))));
        assert_eq!(t.get(&k5), Ok(None));

        assert_eq!(t.root_hash(), None);

        let mut batch = Batch::default();
        t.commit(&mut batch);
        t.db.write(batch).expect("DB Error!");

        t.root_hash().unwrap();
    }

    #[test]
    fn test_commit_and_get() {
        let k4: H256 = [4; 32].into();
        let k5: H256 = [5; 32].into();

        let db = Arc::new(open_db());

        let root_hash = build_and_commit_tree(db.clone()).root_hash().unwrap();

        let mut t = AvlTree::new(db, TEST_COL, root_hash);
        // after serialization, the bitvec is aligned to bytes
        assert_eq!(t.get(&k4), Ok(Some(TransactionMeta::new(8))));
        assert_eq!(t.get(&k5), Ok(None));
    }

    #[test]
    fn test_update() {
        let k3: H256 = [3; 32].into();
        let k4: H256 = [4; 32].into();
        let k5: H256 = [5; 32].into();

        let db = Arc::new(open_db());

        let root_hash = {
            let mut t = build_tree(db.clone());
            assert_eq!(Ok(false), t.update(k5.clone(), 0));
            assert_eq!(Ok(true), t.update(k4.clone(), 0));
            assert_eq!(Ok(true), t.update(k4.clone(), 2));
            assert_eq!(Ok(false), t.update(k4.clone(), 0));
            assert_eq!(Ok(false), t.update(k5.clone(), 0));

            let mut batch = Batch::default();
            t.commit(&mut batch);
            t.db.write(batch).expect("DB Error!");

            assert_eq!(Ok(true), t.update(k3, 2));
            assert_eq!(Ok(false), t.update(k4.clone(), 0));
            assert_eq!(Ok(false), t.update(k5, 0));

            let mut batch = Batch::default();
            t.commit(&mut batch);
            t.db.write(batch).expect("DB Error!");
            t.root_hash().unwrap()
        };

        {
            let mut t = AvlTree::new(db, TEST_COL, root_hash);
            // after serialization, the bitvec is aligned to bytes
            let t4 = t.get(&k4).unwrap().unwrap();
            assert_eq!(t4.output_spent.to_bytes(), vec![0b1010_0000u8]);
        }
    }

    #[test]
    fn test_multiple_commits() {
        let k1: H256 = [1; 32].into();
        let k2: H256 = [2; 32].into();
        let k3: H256 = [3; 32].into();
        let k4: H256 = [4; 32].into();
        let k5: H256 = [5; 32].into();
        let k6: H256 = [6; 32].into();

        let db = Arc::new(open_db());

        let mut root_hash = build_and_commit_tree(db.clone()).root_hash().unwrap();

        root_hash = {
            let mut tree = AvlTree::new(db.clone(), TEST_COL, root_hash);
            tree.update(k4.clone(), 2).unwrap();
            tree.update(k3.clone(), 2).unwrap();
            tree.insert(k5.clone(), TransactionMeta::new(5)).unwrap();
            let mut batch = Batch::default();
            tree.commit(&mut batch);
            tree.db.write(batch).expect("DB Error!");
            tree.root_hash().unwrap()
        };

        root_hash = {
            let mut tree = AvlTree::new(db.clone(), TEST_COL, root_hash);
            tree.update(k2.clone(), 1).unwrap();
            tree.update(k5.clone(), 0).unwrap();
            tree.update(k5.clone(), 2).unwrap();
            tree.update(k5.clone(), 3).unwrap();
            tree.insert(k6.clone(), TransactionMeta::new(6)).unwrap();
            let mut batch = Batch::default();
            tree.commit(&mut batch);
            tree.db.write(batch).expect("DB Error!");
            tree.root_hash().unwrap()
        };

        root_hash = {
            let mut tree = AvlTree::new(db.clone(), TEST_COL, root_hash);
            tree.update(k6.clone(), 3).unwrap();
            tree.update(k3.clone(), 0).unwrap();
            tree.update(k3.clone(), 1).unwrap();
            let mut batch = Batch::default();
            tree.commit(&mut batch);
            tree.db.write(batch).expect("DB Error!");
            tree.root_hash().unwrap()
        };

        let mut tree = AvlTree::new(db, TEST_COL, root_hash);

        let t1 = tree.get(&k1).unwrap().unwrap();
        assert_eq!(t1.output_spent.to_bytes(), vec![0b0000_0000u8]);
        let t2 = tree.get(&k2).unwrap().unwrap();
        assert_eq!(t2.output_spent.to_bytes(), vec![0b0100_0000u8]);
        let t3 = tree.get(&k3).unwrap().unwrap();
        assert_eq!(t3.output_spent.to_bytes(), vec![0b1110_0000u8]);
        let t4 = tree.get(&k4).unwrap().unwrap();
        assert_eq!(t4.output_spent.to_bytes(), vec![0b0010_0000u8]);
        let t5 = tree.get(&k5).unwrap().unwrap();
        assert_eq!(t5.output_spent.to_bytes(), vec![0b1011_0000u8]);
        let t6 = tree.get(&k6).unwrap().unwrap();
        assert_eq!(t6.output_spent.to_bytes(), vec![0b0001_0000u8]);
    }
}
