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

use bigint::H256;
use chain::chain::ChainProvider;
use core::header::{BlockNumber, Header};
use core::transaction::{IndexedTransaction, Transaction};
use jsonrpc_core::{IoHandler, Result};
use jsonrpc_minihttp_server::ServerBuilder;
use jsonrpc_server_utils::cors::AccessControlAllowOrigin;
use jsonrpc_server_utils::hosts::DomainsValidation;
use miner::{build_block_template, BlockTemplate};
use network::NetworkService;
use pool::TransactionPool;
use std::sync::Arc;
use sync::RELAY_PROTOCOL_ID;

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

impl<C: ChainProvider + 'static> Rpc for RpcImpl<C> {
    fn send_transaction(&self, tx: Transaction) -> Result<H256> {
        let indexed_tx: IndexedTransaction = tx.into();
        let result = indexed_tx.hash();
        let pool_result = self.tx_pool.insert_candidate(indexed_tx.clone());
        debug!(target: "rpc", "send_transaction add to pool result: {:?}", pool_result);
        // TODO PENDING new network api
        // let mut payload = Payload::new();
        // payload.set_transaction((&indexed_tx).into());
        // self.network.with_context_eval(RELAY_PROTOCOL_ID, |nc| {
        //     for (peer_id, _session) in nc.sessions(&self.network.connected_peers()) {
        //         let _ = nc.send_payload(peer_id, payload.clone());
        //     }
        // });
        Ok(result)
    }

    fn get_block(&self, hash: H256) -> Result<Option<BlockWithHashedTransactions>> {
        Ok(self
            .chain
            .block(&hash)
            .map(|block| BlockWithHashedTransactions {
                header: block.header.into(),
                transactions: block
                    .commit_transactions
                    .into_iter()
                    .map(|transaction| {
                        let hash = transaction.hash();
                        TransactionWithHash {
                            transaction: transaction.into(),
                            hash,
                        }
                    }).collect(),
            }))
    }

    fn get_transaction(&self, hash: H256) -> Result<Option<Transaction>> {
        Ok(self.chain.get_transaction(&hash).map(|t| t.transaction))
    }

    fn get_block_hash(&self, number: BlockNumber) -> Result<Option<H256>> {
        Ok(self.chain.block_hash(number))
    }

    // what's happening 😨
    fn get_tip_header(&self) -> Result<Header> {
        Ok(self.chain.tip_header().read().header.header.clone())
    }

    fn get_block_template(&self) -> Result<BlockTemplate> {
        Ok(build_block_template(&self.chain, &self.tx_pool).unwrap())
    }
}

pub struct RpcServer {
    pub config: Config,
}

impl RpcServer {
    pub fn start<C>(
        &self,
        network: Arc<NetworkService>,
        chain: Arc<C>,
        tx_pool: Arc<TransactionPool<C>>,
    ) where
        C: ChainProvider + 'static,
    {
        let mut io = IoHandler::new();
        io.extend_with(
            RpcImpl {
                network,
                chain,
                tx_pool,
            }.to_delegate(),
        );

        let server = ServerBuilder::new(io)
            .cors(DomainsValidation::AllowOnly(vec![
                AccessControlAllowOrigin::Null,
                AccessControlAllowOrigin::Any,
            ])).threads(3)
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
