use batch::{Batch, Key, KeyValue, Operation, Value};
use bigint::H256;
use core::block::Header;
use core::transaction::Transaction;
use kvdb::{KeyValueDB, Result};
use std::collections::HashMap;
use transaction_meta::TransactionMeta;
use util::RwLock;

#[derive(Default, Debug)]
struct Inner {
    meta: HashMap<&'static str, Vec<u8>>,
    block_hash: HashMap<u64, H256>,
    block_header: HashMap<H256, Box<Header>>,
    block_transactions: HashMap<H256, Vec<H256>>,
    transaction: HashMap<H256, Box<Transaction>>,
    transaction_meta: HashMap<H256, Box<TransactionMeta>>,
}

#[derive(Default, Debug)]
pub struct MemoryKeyValueDB {
    db: RwLock<Inner>,
}

impl KeyValueDB for MemoryKeyValueDB {
    fn write(&self, batch: Batch) -> Result<()> {
        let mut db = self.db.write();
        batch.operations.into_iter().for_each(|op| match op {
            Operation::Insert(insert) => match insert {
                KeyValue::BlockHash(key, value) => {
                    db.block_hash.insert(key, value);
                }
                KeyValue::BlockHeader(key, value) => {
                    db.block_header.insert(key, value);
                }
                KeyValue::BlockTransactions(key, value) => {
                    db.block_transactions.insert(key, value);
                }
                KeyValue::Meta(key, value) => {
                    db.meta.insert(key, value);
                }
                KeyValue::Transaction(key, value) => {
                    db.transaction.insert(key, value);
                }
                KeyValue::TransactionMeta(key, value) => {
                    db.transaction_meta.insert(key, value);
                }
            },
            Operation::Delete(delete) => match delete {
                Key::BlockHash(key) => {
                    db.block_hash.remove(&key);
                }
                Key::BlockHeader(key) => {
                    db.block_header.remove(&key);
                }
                Key::BlockTransactions(key) => {
                    db.block_transactions.remove(&key);
                }
                Key::Meta(key) => {
                    db.meta.remove(&key);
                }
                Key::Transaction(key) => {
                    db.transaction.remove(&key);
                }
                Key::TransactionMeta(key) => {
                    db.transaction_meta.remove(&key);
                }
            },
        });
        Ok(())
    }

    fn read(&self, key: &Key) -> Result<Option<Value>> {
        let db = self.db.read();
        let result = match *key {
            Key::BlockHash(ref key) => db.block_hash
                .get(key)
                .and_then(|v| Some(Value::BlockHash(*v))),
            Key::BlockHeader(ref key) => db.block_header
                .get(key)
                .and_then(|v| Some(Value::BlockHeader(v.clone()))),
            Key::BlockTransactions(ref key) => db.block_transactions
                .get(key)
                .and_then(|v| Some(Value::BlockTransactions(v.clone()))),
            Key::Meta(key) => db.meta.get(key).and_then(|v| Some(Value::Meta(v.clone()))),
            Key::Transaction(ref key) => db.transaction
                .get(key)
                .and_then(|v| Some(Value::Transaction(v.clone()))),
            Key::TransactionMeta(ref key) => db.transaction_meta
                .get(key)
                .and_then(|v| Some(Value::TransactionMeta(v.clone()))),
        };
        Ok(result)
    }
}
