#![feature(box_syntax)]
extern crate bigint;
extern crate jsonrpc_core;
#[macro_use]
extern crate jsonrpc_macros;
extern crate jsonrpc_minihttp_server;
#[macro_use]
extern crate log;
extern crate nervos_core as core;
extern crate nervos_network as network;

use bigint::H256;
use core::adapter::NetAdapter;
use core::transaction::Transaction;
use jsonrpc_core::{IoHandler, Result};
use jsonrpc_minihttp_server::ServerBuilder;
use network::Network;
use std::sync::Arc;

build_rpc_trait! {
    pub trait Rpc {
        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"send_transaction","params": [{"version":2, "inputs":[], "outputs":[], "groupings":[]}]}' -H 'content-type:application/json' 'http://localhost:3030'
        #[rpc(name = "send_transaction")]
        fn send_transaction(&self, Transaction) -> Result<H256>;
    }
}

struct RpcImpl {
    pub network: Arc<Network>,
}
impl Rpc for RpcImpl {
    fn send_transaction(&self, tx: Transaction) -> Result<H256> {
        let result = tx.hash();
        // self.network.transaction_received(tx.clone());
        // TODO: should only broadcast after validate the transaction
        self.network.broadcast(vec![1, 2, 3, 4]);
        Ok(result)
    }
}

pub struct RpcServer {
    pub config: Config,
}
impl RpcServer {
    pub fn start(&self, network: Arc<Network>) {
        let mut io = IoHandler::new();
        io.extend_with(RpcImpl { network }.to_delegate());

        let server = ServerBuilder::new(io)
            .threads(3)
            .start_http(&self.config.listen_addr.parse().unwrap())
            .unwrap();

        info!(target: "rpc", "Now listening on {:?}", server.address());
        server.wait().unwrap();
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub listen_addr: String,
}
