extern crate bigint;
extern crate jsonrpc_core;
#[macro_use]
extern crate jsonrpc_macros;
extern crate jsonrpc_http_server;
extern crate jsonrpc_server_utils;
#[macro_use]
extern crate log;
extern crate ckb_core as core;
#[cfg(test)]
extern crate ckb_db as db;
extern crate ckb_network as network;
extern crate ckb_notify as notify;
extern crate ckb_pool as pool;
extern crate ckb_shared as shared;
extern crate ckb_time;
#[cfg(test)]
extern crate ckb_verification as verification;
#[macro_use]
extern crate serde_derive;
#[cfg(feature = "integration_test")]
extern crate ckb_pow;
#[macro_use]
extern crate crossbeam_channel as channel;
extern crate fnv;

use bigint::H256;
use core::block::Block;
use core::header::Header;
use core::transaction::{Capacity, OutPoint, Transaction};

mod service;

pub use service::{BlockTemplate, RpcController, RpcReceivers, RpcService};

#[cfg(feature = "integration_test")]
mod integration_test;
#[cfg(not(feature = "integration_test"))]
mod server;

#[cfg(feature = "integration_test")]
pub use integration_test::RpcServer;
#[cfg(not(feature = "integration_test"))]
pub use server::RpcServer;

#[derive(Serialize)]
pub struct TransactionWithHash {
    pub hash: H256,
    pub transaction: Transaction,
}

impl From<Transaction> for TransactionWithHash {
    fn from(transaction: Transaction) -> Self {
        Self {
            hash: transaction.hash(),
            transaction,
        }
    }
}

#[derive(Serialize)]
pub struct BlockWithHash {
    pub hash: H256,
    pub header: Header,
    pub transactions: Vec<TransactionWithHash>,
}

impl From<Block> for BlockWithHash {
    fn from(block: Block) -> Self {
        Self {
            header: block.header().clone(),
            transactions: block
                .commit_transactions()
                .iter()
                .map(|tx| tx.clone().into())
                .collect(),
            hash: block.header().hash(),
        }
    }
}

// This is used as return value of get_cells_by_type_hash RPC:
// it contains both OutPoint data used for referencing a cell, as well as
// cell's own data such as lock and capacity
#[derive(Serialize)]
pub struct CellOutputWithOutPoint {
    pub outpoint: OutPoint,
    pub capacity: Capacity,
    pub lock: H256,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Config {
    pub listen_addr: String,
}
