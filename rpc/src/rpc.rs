use bigint::H256;
use chain::chain::ChainProvider;
use ckb_protocol::Payload;
use core::header::{BlockNumber, Header};
use core::transaction::{IndexedTransaction, Transaction};
use jsonrpc_core::{IoHandler, Result};
use jsonrpc_http_server::ServerBuilder;
use jsonrpc_server_utils::cors::AccessControlAllowOrigin;
use jsonrpc_server_utils::hosts::DomainsValidation;
use miner::{build_block_template, BlockTemplate};
use network::NetworkContextExt;
use network::NetworkService;
use pool::TransactionPool;
use std::sync::Arc;
use sync::RELAY_PROTOCOL_ID;
use {BlockWithHashedTransactions, Config, TransactionWithHash};

build_rpc_trait! {
    pub trait Rpc {
        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"send_transaction","params": [{"version":2, "deps":[], "inputs":[], "outputs":[]}]}' -H 'content-type:application/json' 'http://localhost:3030'
        #[rpc(name = "send_transaction")]
        fn send_transaction(&self, Transaction) -> Result<H256>;

        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_block","params": ["0x0f9da6db98d0acd1ae0cf7ae3ee0b2b5ad2855d93c18d27c0961f985a62a93c3"]}' -H 'content-type:application/json' 'http://localhost:3030'
        #[rpc(name = "get_block")]
        fn get_block(&self, H256) -> Result<Option<BlockWithHashedTransactions>>;

        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_transaction","params": ["0x0f9da6db98d0acd1ae0cf7ae3ee0b2b5ad2855d93c18d27c0961f985a62a93c3"]}' -H 'content-type:application/json' 'http://localhost:3030'
        #[rpc(name = "get_transaction")]
        fn get_transaction(&self, H256) -> Result<Option<Transaction>>;

        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_block_hash","params": [1]}' -H 'content-type:application/json' 'http://localhost:3030'
        #[rpc(name = "get_block_hash")]
        fn get_block_hash(&self, u64) -> Result<Option<H256>>;

        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_tip_header","params": []}' -H 'content-type:application/json' 'http://localhost:3030'
        #[rpc(name = "get_tip_header")]
        fn get_tip_header(&self) -> Result<Header>;

        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_block_template","params": []}' -H 'content-type:application/json' 'http://localhost:3030'
        #[rpc(name = "get_block_template")]
        fn get_block_template(&self) -> Result<BlockTemplate>;
    }
}

struct RpcImpl<C> {
    pub network: Arc<NetworkService>,
    pub chain: Arc<C>,
    pub tx_pool: Arc<TransactionPool<C>>,
}

impl<C: ChainProvider + 'static> Rpc for RpcImpl<C> {
    fn send_transaction(&self, tx: Transaction) -> Result<H256> {
        let indexed_tx: IndexedTransaction = tx.into();
        let result = indexed_tx.hash();
        let pool_result = self.tx_pool.add_transaction(indexed_tx.clone());
        debug!(target: "rpc", "send_transaction add to pool result: {:?}", pool_result);

        let mut payload = Payload::new();
        payload.set_transaction((&indexed_tx).into());
        self.network.with_context_eval(RELAY_PROTOCOL_ID, |nc| {
            for (peer_id, _session) in nc.sessions(&self.network.connected_peers()) {
                let _ = nc.send_payload(peer_id, payload.clone());
            }
        });
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

    // TODO: the max size
    fn get_block_template(&self) -> Result<BlockTemplate> {
        Ok(build_block_template(&self.chain, &self.tx_pool, 20000, 20000).unwrap())
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
            ])).start_http(&self.config.listen_addr.parse().unwrap())
            .unwrap();

        info!(target: "rpc", "Now listening on {:?}", server.address());
        server.wait();
    }
}
