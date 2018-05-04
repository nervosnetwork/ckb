use bigint::H256;
use core::{header, transaction};
use transaction_meta::TransactionMeta;

#[derive(Debug)]
pub enum Operation {
    Insert(KeyValue),
    Delete(Key),
}

#[derive(Debug)]
pub enum KeyValue {
    BlockHeight(H256, u64),
    BlockHash(u64, H256),
    BlockHeader(H256, Box<header::Header>),
    BlockTransactions(H256, Vec<H256>),
    Meta(&'static str, Vec<u8>),
    Transaction(H256, Box<transaction::Transaction>),
    TransactionMeta(H256, Box<TransactionMeta>),
}

#[derive(Debug, PartialEq)]
pub enum Key {
    BlockHeight(H256),
    BlockHash(u64),
    BlockHeader(H256),
    BlockTransactions(H256),
    Meta(&'static str),
    Transaction(H256),
    TransactionMeta(H256),
}

#[derive(Debug, PartialEq)]
pub enum Value {
    BlockHeight(u64),
    BlockHash(H256),
    BlockHeader(Box<header::Header>),
    BlockTransactions(Vec<H256>),
    Meta(Vec<u8>),
    Transaction(Box<transaction::Transaction>),
    TransactionMeta(Box<TransactionMeta>),
}

#[derive(Debug)]
pub struct Batch {
    pub operations: Vec<Operation>,
}

impl Default for Batch {
    fn default() -> Self {
        Batch {
            operations: Vec::with_capacity(32),
        }
    }
}

impl Batch {
    pub fn new() -> Self {
        Batch::default()
    }

    pub fn insert(&mut self, insert: KeyValue) {
        self.operations.push(Operation::Insert(insert));
    }

    pub fn delete(&mut self, delete: Key) {
        self.operations.push(Operation::Delete(delete));
    }
}
