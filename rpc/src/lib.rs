extern crate bigint;
extern crate jsonrpc_core;
#[macro_use]
extern crate jsonrpc_macros;
extern crate jsonrpc_http_server;
extern crate jsonrpc_server_utils;
#[macro_use]
extern crate log;
extern crate ckb_chain as chain;
extern crate ckb_core as core;
extern crate ckb_miner as miner;
extern crate ckb_network as network;
extern crate ckb_pool as pool;
#[macro_use]
extern crate serde_derive;
#[cfg(feature = "integration_test")]
extern crate ckb_pow;

use bigint::H256;
use core::header::Header;
use core::transaction::Transaction;

#[cfg(feature = "integration_test")]
mod integration_test;
#[cfg(not(feature = "integration_test"))]
mod rpc;

#[cfg(feature = "integration_test")]
pub use integration_test::RpcServer;
#[cfg(not(feature = "integration_test"))]
pub use rpc::RpcServer;

#[derive(Serialize)]
pub struct TransactionWithHash {
    pub transaction: Transaction,
    pub hash: H256,
}

#[derive(Serialize)]
pub struct BlockWithHashedTransactions {
    pub header: Header,
    pub transactions: Vec<TransactionWithHash>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Config {
    pub listen_addr: String,
}
