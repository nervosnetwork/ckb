use bigint::H256;
use core::{block, transaction};

#[derive(Debug)]
pub enum Operation {
    Insert(KeyValue),
    Delete(Key),
}

#[derive(Debug)]
pub enum KeyValue {
    BlockHash(u64, H256),
    BlockHeader(H256, Box<block::Header>),
    BlockTransactions(H256, Vec<H256>),
    Meta(&'static str, Vec<u8>),
    Transaction(H256, Box<transaction::Transaction>),
}

#[derive(Debug, PartialEq)]
pub enum Key {
    BlockHash(u64),
    BlockHeader(H256),
    BlockTransactions(H256),
    Meta(&'static str),
    Transaction(H256),
}

#[derive(Debug, PartialEq)]
pub enum Value {
    BlockHash(H256),
    BlockHeader(Box<block::Header>),
    BlockTransactions(Vec<H256>),
    Meta(Vec<u8>),
    Transaction(Box<transaction::Transaction>),
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
