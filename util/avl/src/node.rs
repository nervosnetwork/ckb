use super::{AVLError, Result};
use bigint::H256;
use bincode::{deserialize, serialize};
use core::transaction_meta::TransactionMeta;
use db::batch::{Batch, Col};
use db::kvdb::KeyValueDB;
use hash::sha3_256;

// DB node in the avl
#[derive(Eq, PartialEq, Debug, Serialize, Deserialize, Clone)]
pub enum DBNode {
    // Leaf
    Leaf(H256, TransactionMeta),
    // Branch
    Branch(u32, H256, [H256; 2]),
}

pub fn get(db: &KeyValueDB, col: Col, h: H256) -> Result<DBNode> {
    if let Some(v) = db.read(col, &h).expect("DB Error!") {
        return Ok(deserialize(&v).unwrap());
    } else {
        return Err(Box::new(AVLError::DatabaseError(h)));
    }
}

pub fn insert(col: Col, batch: &mut Batch, node: &DBNode) -> H256 {
    let raw = serialize(node).unwrap();

    let h: H256 = sha3_256(&raw).into();

    batch.insert(col, h.to_vec(), raw);

    h
}

pub fn search(
    db: &KeyValueDB,
    col: Col,
    mut hash: H256,
    key: H256,
) -> Result<Option<TransactionMeta>> {
    loop {
        let node = get(db, col, hash)?;

        match node {
            DBNode::Leaf(k, value) => {
                if k == key {
                    return Ok(Some(value));
                } else {
                    return Ok(None);
                }
            }
            DBNode::Branch(_, k, children) => {
                let idx = if key < k { 0 } else { 1 };
                hash = children[idx as usize];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    const TEST_COL: Col = Some(0);
    use super::*;
    use db::memorydb::MemoryKeyValueDB;

    fn get_meta(node: &DBNode) -> Option<TransactionMeta> {
        match node {
            DBNode::Leaf(_, meta) => Some(meta.clone()),
            _ => None,
        }
    }

    fn leaf_eq(a: &DBNode, b: &DBNode) -> bool {
        match (a, b) {
            (DBNode::Leaf(k1, m1), DBNode::Leaf(k2, m2)) if k1 == k2 => meta_eq(m1, m2),
            _ => false,
        }
    }

    fn meta_eq(a: &TransactionMeta, b: &TransactionMeta) -> bool {
        a.output_spent.to_bytes() == b.output_spent.to_bytes()
    }

    struct TestContext {
        db: MemoryKeyValueDB,
        root: DBNode,
        root_hash: H256,
        left: DBNode,
        left_hash: H256,
        left_key: H256,
        right: DBNode,
        right_hash: H256,
        right_key: H256,
    }

    fn build_context() -> TestContext {
        let db = MemoryKeyValueDB::open(1);

        let mut left_meta = TransactionMeta::new(2);
        left_meta.set_spent(1);
        let mut right_meta = TransactionMeta::new(9);
        right_meta.set_spent(2);

        let mut batch = Batch::new();

        let left_key = H256::from(1);
        let left = DBNode::Leaf(left_key.clone(), left_meta);
        let right_key = H256::from(2);
        let right = DBNode::Leaf(right_key.clone(), right_meta);

        let left_hash = insert(TEST_COL, &mut batch, &left);
        let right_hash = insert(TEST_COL, &mut batch, &right);

        let root = DBNode::Branch(0, right_key, [left_hash, right_hash]);
        let root_hash = insert(TEST_COL, &mut batch, &root);
        db.write(batch).expect("db.write(batch)");

        TestContext {
            db,
            root,
            root_hash,
            left,
            left_hash,
            left_key,
            right,
            right_hash,
            right_key,
        }
    }

    #[test]
    fn test_get() {
        let context = build_context();
        let left_found = get(&context.db, TEST_COL, context.left_hash);
        let right_found = get(&context.db, TEST_COL, context.right_hash);
        let root_found = get(&context.db, TEST_COL, context.root_hash);
        let not_found = get(&context.db, TEST_COL, H256::from(123));

        assert!(left_found.is_ok());
        assert!(leaf_eq(&context.left, &left_found.unwrap()));
        assert!(right_found.is_ok());
        assert!(leaf_eq(&context.right, &right_found.unwrap()));
        assert!(root_found.is_ok());
        assert_eq!(context.root, root_found.unwrap());
        assert!(not_found.is_err());
    }

    #[test]
    fn test_search() {
        let context = build_context();
        let left_found = search(&context.db, TEST_COL, context.root_hash, context.left_key);
        let right_found = search(&context.db, TEST_COL, context.root_hash, context.right_key);
        let branch_not_found = search(&context.db, TEST_COL, H256::from(123), context.left_key);
        let leaf_not_found = search(&context.db, TEST_COL, context.root_hash, H256::from(123));

        assert!(meta_eq(
            &left_found.unwrap().unwrap(),
            &get_meta(&context.left).unwrap()
        ));
        assert!(meta_eq(
            &right_found.unwrap().unwrap(),
            &get_meta(&context.right).unwrap()
        ));
        assert!(branch_not_found.is_err());
        assert!(leaf_not_found.is_ok() && leaf_not_found.unwrap().is_none());
    }
}
