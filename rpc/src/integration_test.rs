use super::{
    BlockTemplate, BlockWithHash, CellOutputWithOutPoint, Config, RpcController,
    TransactionWithHash,
};
use bigint::H256;
use ckb_core::header::{BlockNumber, Header};
use ckb_core::transaction::{OutPoint, Transaction};
use ckb_network::NetworkService;
use ckb_pool::txs_pool::TransactionPoolController;
use ckb_pow::Clicker;
use ckb_protocol::RelayMessage;
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::{ChainProvider, Shared};
use ckb_sync::RELAY_PROTOCOL_ID;
use flatbuffers::FlatBufferBuilder;
use jsonrpc_core::{Error, IoHandler, Result};
use jsonrpc_http_server::ServerBuilder;
use jsonrpc_server_utils::cors::AccessControlAllowOrigin;
use jsonrpc_server_utils::hosts::DomainsValidation;
use std::sync::Arc;

//TODO: build_rpc_trait! do not surppot trait bounds
build_rpc_trait! {
    pub trait IntegrationTestRpc {
        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"submit_solution","params": [1]}' -H 'content-type:application/json' 'http://localhost:8114'
        #[rpc(name = "submit_pow_solution")]
        fn submit_pow_solution(&self, u64) -> Result<()>;

        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"send_transaction","params": [{"version":2, "deps":[], "inputs":[], "outputs":[]}]}' -H 'content-type:application/json' 'http://localhost:8114'
        #[rpc(name = "send_transaction")]
        fn send_transaction(&self, Transaction) -> Result<H256>;

        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_block","params": ["0x0f9da6db98d0acd1ae0cf7ae3ee0b2b5ad2855d93c18d27c0961f985a62a93c3"]}' -H 'content-type:application/json' 'http://localhost:8114'
        #[rpc(name = "get_block")]
        fn get_block(&self, H256) -> Result<Option<BlockWithHash>>;

        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_transaction","params": ["0x0f9da6db98d0acd1ae0cf7ae3ee0b2b5ad2855d93c18d27c0961f985a62a93c3"]}' -H 'content-type:application/json' 'http://localhost:8114'
        #[rpc(name = "get_transaction")]
        fn get_transaction(&self, H256) -> Result<Option<TransactionWithHash>>;

        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_block_hash","params": [1]}' -H 'content-type:application/json' 'http://localhost:8114'
        #[rpc(name = "get_block_hash")]
        fn get_block_hash(&self, u64) -> Result<Option<H256>>;

        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_tip_header","params": []}' -H 'content-type:application/json' 'http://localhost:8114'
        #[rpc(name = "get_tip_header")]
        fn get_tip_header(&self) -> Result<Header>;

        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_block_template","params": []}' -H 'content-type:application/json' 'http://localhost:8114'
        #[rpc(name = "get_block_template")]
        fn get_block_template(&self) -> Result<BlockTemplate>;

        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_cells_by_type_hash","params": ["0x1b1c832d02fdb4339f9868c8a8636c3d9dd10bd53ac7ce99595825bd6beeffb3", 1, 10]}' -H 'content-type:application/json' 'http://localhost:8114'
        #[rpc(name = "get_cells_by_type_hash")]
        fn get_cells_by_type_hash(&self, H256, u64, u64) -> Result<Vec<CellOutputWithOutPoint>>;

        #[rpc(name = "local_node_id")]
        fn local_node_id(&self) -> Result<Option<String>>;

        #[rpc(name = "add_node")]
        fn add_node(&self, String) -> Result<()>;
    }
}

struct RpcImpl<CI> {
    pub network: Arc<NetworkService>,
    pub shared: Shared<CI>,
    pub rpc: RpcController,
    pub tx_pool: TransactionPoolController,
    pub pow: Arc<Clicker>,
}

impl<CI: ChainIndex + 'static> IntegrationTestRpc for RpcImpl<CI> {
    fn submit_pow_solution(&self, nonce: u64) -> Result<()> {
        self.pow.submit(nonce);
        Ok(())
    }

    fn send_transaction(&self, tx: Transaction) -> Result<H256> {
        let tx_hash = tx.hash();
        let pool_result = self.tx_pool.add_transaction(tx.clone());
        debug!(target: "rpc", "send_transaction add to pool result: {:?}", pool_result);

        let fbb = &mut FlatBufferBuilder::new();
        let message = RelayMessage::build_transaction(fbb, &tx);
        fbb.finish(message, None);

        self.network.with_protocol_context(RELAY_PROTOCOL_ID, |nc| {
            for peer in nc.connected_peers() {
                debug!(target: "rpc", "relay transaction {} to peer#{}", tx_hash, peer);
                let _ = nc.send(peer, fbb.finished_data().to_vec());
            }
        });
        Ok(tx_hash)
    }

    fn get_block(&self, hash: H256) -> Result<Option<BlockWithHash>> {
        Ok(self.shared.block(&hash).map(Into::into))
    }

    fn get_transaction(&self, hash: H256) -> Result<Option<TransactionWithHash>> {
        Ok(self.shared.get_transaction(&hash).map(Into::into))
    }

    fn get_block_hash(&self, number: BlockNumber) -> Result<Option<H256>> {
        Ok(self.shared.block_hash(number))
    }

    fn get_tip_header(&self) -> Result<Header> {
        Ok(self.shared.tip_header().read().inner().clone())
    }

    fn get_block_template(&self) -> Result<BlockTemplate> {
        Ok(self
            .rpc
            .get_block_template(H256::from(0), 20000, 20000)
            .unwrap())
    }

    fn get_cells_by_type_hash(
        &self,
        type_hash: H256,
        from: u64,
        to: u64,
    ) -> Result<Vec<CellOutputWithOutPoint>> {
        let mut result = Vec::new();
        for block_number in from..=to {
            if let Some(block_hash) = self.shared.block_hash(block_number) {
                let block = self
                    .shared
                    .block(&block_hash)
                    .ok_or_else(Error::internal_error)?;
                let tip_header = self.shared.tip_header().read();
                for transaction in block.commit_transactions() {
                    let transaction_meta = self
                        .shared
                        .get_transaction_meta(&tip_header.output_root(), &transaction.hash())
                        .ok_or_else(Error::internal_error)?;
                    for (i, output) in transaction.outputs().iter().enumerate() {
                        if output.lock == type_hash && (!transaction_meta.is_spent(i)) {
                            result.push(CellOutputWithOutPoint {
                                outpoint: OutPoint::new(transaction.hash(), i as u32),
                                capacity: output.capacity,
                                lock: output.lock,
                            });
                        }
                    }
                }
            }
        }
        Ok(result)
    }

    fn local_node_id(&self) -> Result<Option<String>> {
        Ok(self.network.external_url())
    }

    fn add_node(&self, _node_id: String) -> Result<()> {
        unimplemented!()
    }
}

pub struct RpcServer {
    pub config: Config,
}

impl RpcServer {
    pub fn start<CI>(
        &self,
        network: Arc<NetworkService>,
        shared: Shared<CI>,
        tx_pool: TransactionPoolController,
        rpc: RpcController,
        pow: Arc<Clicker>,
    ) where
        CI: ChainIndex + 'static,
    {
        let mut io = IoHandler::new();
        io.extend_with(
            RpcImpl {
                network,
                shared,
                tx_pool,
                rpc,
                pow,
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
