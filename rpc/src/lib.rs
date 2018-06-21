#![feature(box_syntax)]
extern crate bigint;
extern crate jsonrpc_core;
#[macro_use]
extern crate jsonrpc_macros;
extern crate jsonrpc_minihttp_server;
extern crate jsonrpc_server_utils;
#[macro_use]
extern crate log;
extern crate nervos_chain as chain;
extern crate nervos_core as core;
extern crate nervos_network as network;
extern crate nervos_protocol;
extern crate nervos_sync as sync;
#[macro_use]
extern crate serde_derive;

use bigint::H256;
use chain::chain::ChainProvider;
use core::block::Block;
use core::header::Header;
use core::transaction::Transaction;
use jsonrpc_core::{IoHandler, Result};
use jsonrpc_minihttp_server::ServerBuilder;
use jsonrpc_server_utils::cors::AccessControlAllowOrigin;
use jsonrpc_server_utils::hosts::DomainsValidation;
use nervos_protocol::Payload;
use network::NetworkService;
use std::sync::Arc;
use sync::RELAY_PROTOCOL_ID;

build_rpc_trait! {
    pub trait Rpc {
        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"send_transaction","params": [{"version":2, "deps":[], "inputs":[], "outputs":[]}]}' -H 'content-type:application/json' 'http://localhost:3030'
        #[rpc(name = "send_transaction")]
        fn send_transaction(&self, Transaction) -> Result<H256>;

        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_block","params": ["0x0f9da6db98d0acd1ae0cf7ae3ee0b2b5ad2855d93c18d27c0961f985a62a93c3"]}' -H 'content-type:application/json' 'http://localhost:3030'
        #[rpc(name = "get_block")]
        fn get_block(&self, H256) -> Result<Option<Block>>;

        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_transaction","params": ["0x0f9da6db98d0acd1ae0cf7ae3ee0b2b5ad2855d93c18d27c0961f985a62a93c3"]}' -H 'content-type:application/json' 'http://localhost:3030'
        #[rpc(name = "get_transaction")]
        fn get_transaction(&self, H256) -> Result<Option<Transaction>>;

        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_block_hash","params": [1]}' -H 'content-type:application/json' 'http://localhost:3030'
        #[rpc(name = "get_block_hash")]
        fn get_block_hash(&self, u64) -> Result<Option<H256>>;

        #[rpc(name = "get_tip_header")]
        fn get_tip_header(&self) -> Result<Header>;
    }
}

struct RpcImpl {
    pub network: Arc<NetworkService>,
    pub chain: Arc<ChainProvider>,
}

impl Rpc for RpcImpl {
    fn send_transaction(&self, tx: Transaction) -> Result<H256> {
        let result = tx.hash();
        let tx: nervos_protocol::Transaction = (&tx).into();
        let mut payload = Payload::new();
        payload.set_transaction(tx);
        self.network.with_context_eval(RELAY_PROTOCOL_ID, |nc| {
            for (peer_id, _session) in nc.sessions() {
                nc.send(peer_id, payload.clone()).ok();
            }
        });
        Ok(result)
    }

    fn get_block(&self, hash: H256) -> Result<Option<Block>> {
        Ok(self.chain.block(&hash).map(Into::into))
    }

    fn get_transaction(&self, hash: H256) -> Result<Option<Transaction>> {
        Ok(self.chain.get_transaction(&hash).map(|t| t.transaction))
    }

    fn get_block_hash(&self, number: u64) -> Result<Option<H256>> {
        Ok(self.chain.block_hash(number))
    }

    // what's happening 😨
    fn get_tip_header(&self) -> Result<Header> {
        Ok(self.chain.tip_header().read().header.header.clone())
    }
}

pub struct RpcServer {
    pub config: Config,
}
impl RpcServer {
    pub fn start(&self, network: Arc<NetworkService>, chain: Arc<ChainProvider>) {
        let mut io = IoHandler::new();
        io.extend_with(RpcImpl { network, chain }.to_delegate());

        let server = ServerBuilder::new(io)
            .cors(DomainsValidation::AllowOnly(vec![
                AccessControlAllowOrigin::Null,
                AccessControlAllowOrigin::Any,
            ]))
            .threads(3)
            .start_http(&self.config.listen_addr.parse().unwrap())
            .unwrap();

        info!(target: "rpc", "Now listening on {:?}", server.address());
        server.wait().unwrap();
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Config {
    pub listen_addr: String,
}
