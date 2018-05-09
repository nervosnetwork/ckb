use bigint::H256;
use core::header::Header;
use core::transaction::{Transaction, TransactionAddress};
use core::transaction_meta::TransactionMeta;

#[derive(Debug)]
pub enum Operation {
    Insert(KeyValue),
    Delete(Key),
}

#[derive(Debug)]
pub enum KeyValue {
    BlockHeight(H256, u64),
    OutputRoot(H256, H256),
    BlockHash(u64, H256),
    BlockHeader(H256, Box<Header>),
    BlockBody(H256, Vec<Transaction>),
    Meta(&'static str, Vec<u8>),
    TransactionAddress(H256, TransactionAddress),
    TransactionMeta(H256, Box<TransactionMeta>),
    Raw(H256, Vec<u8>),
}

#[derive(Debug, PartialEq)]
pub enum Key {
    BlockHeight(H256),
    OutputRoot(H256),
    BlockHash(u64),
    BlockHeader(H256),
    BlockBody(H256),
    Meta(&'static str),
    TransactionAddress(H256),
    TransactionMeta(H256),
    Raw(H256),
}

#[derive(Debug, PartialEq)]
pub enum Value {
    BlockHeight(u64),
    OutputRoot(H256),
    BlockHash(H256),
    BlockHeader(Box<Header>),
    BlockBody(Vec<Transaction>),
    Meta(Vec<u8>),
    TransactionAddress(TransactionAddress),
    TransactionMeta(Box<TransactionMeta>),
    Raw(Vec<u8>),
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
