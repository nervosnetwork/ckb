use batch::{Batch, Key, KeyValue, Operation, Value};
use bigint::H256;
use core::header::Header;
use core::transaction::{Transaction, TransactionAddress};
use core::transaction_meta::TransactionMeta;
use kvdb::{KeyValueDB, Result};
use std::collections::HashMap;
use util::RwLock;

#[derive(Default, Debug)]
struct Inner {
    meta: HashMap<&'static str, Vec<u8>>,
    block_height: HashMap<H256, u64>,
    block_hash: HashMap<u64, H256>,
    block_header: HashMap<H256, Box<Header>>,
    block_body: HashMap<H256, Vec<Transaction>>,
    transaction_address: HashMap<H256, TransactionAddress>,
    transaction_meta: HashMap<H256, Box<TransactionMeta>>,
    raw: HashMap<H256, Vec<u8>>,
    output_root: HashMap<H256, H256>,
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
                KeyValue::BlockHeight(key, value) => {
                    db.block_height.insert(key, value);
                }
                KeyValue::BlockHeader(key, value) => {
                    db.block_header.insert(key, value);
                }
                KeyValue::BlockBody(key, value) => {
                    db.block_body.insert(key, value);
                }
                KeyValue::Meta(key, value) => {
                    db.meta.insert(key, value);
                }
                KeyValue::TransactionAddress(key, value) => {
                    db.transaction_address.insert(key, value);
                }
                KeyValue::TransactionMeta(key, value) => {
                    db.transaction_meta.insert(key, value);
                }
                KeyValue::Raw(key, value) => {
                    db.raw.insert(key, value);
                }
                KeyValue::OutputRoot(key, value) => {
                    db.output_root.insert(key, value);
                }
            },
            Operation::Delete(delete) => match delete {
                Key::BlockHash(key) => {
                    db.block_hash.remove(&key);
                }
                Key::BlockHeight(key) => {
                    db.block_height.remove(&key);
                }
                Key::BlockHeader(key) => {
                    db.block_header.remove(&key);
                }
                Key::BlockBody(key) => {
                    db.block_body.remove(&key);
                }
                Key::Meta(key) => {
                    db.meta.remove(&key);
                }
                Key::TransactionAddress(key) => {
                    db.transaction_address.remove(&key);
                }
                Key::TransactionMeta(key) => {
                    db.transaction_meta.remove(&key);
                }
                Key::Raw(key) => {
                    db.raw.remove(&key);
                }
                Key::OutputRoot(key) => {
                    db.output_root.remove(&key);
                }
            },
        });
        Ok(())
    }

    fn read(&self, key: &Key) -> Result<Option<Value>> {
        let db = self.db.read();
        let result = match *key {
            Key::BlockHash(ref key) => db
                .block_hash
                .get(key)
                .and_then(|v| Some(Value::BlockHash(*v))),
            Key::BlockHeight(ref key) => db
                .block_height
                .get(key)
                .and_then(|v| Some(Value::BlockHeight(*v))),
            Key::BlockHeader(ref key) => db
                .block_header
                .get(key)
                .and_then(|v| Some(Value::BlockHeader(v.clone()))),
            Key::BlockBody(ref key) => db
                .block_body
                .get(key)
                .and_then(|v| Some(Value::BlockBody(v.clone()))),
            Key::Meta(key) => db.meta.get(key).and_then(|v| Some(Value::Meta(v.clone()))),
            Key::TransactionAddress(ref key) => db
                .transaction_address
                .get(key)
                .and_then(|v| Some(Value::TransactionAddress(v.clone()))),
            Key::TransactionMeta(ref key) => db
                .transaction_meta
                .get(key)
                .and_then(|v| Some(Value::TransactionMeta(v.clone()))),
            Key::Raw(ref key) => db.raw.get(key).and_then(|v| Some(Value::Raw(v.clone()))),
            Key::OutputRoot(ref key) => db
                .output_root
                .get(key)
                .and_then(|v| Some(Value::OutputRoot(*v))),
        };
        Ok(result)
    }
}
