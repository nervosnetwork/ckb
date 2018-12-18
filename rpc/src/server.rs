use ckb_chain::chain::ChainController;
use ckb_core::block::Block;
use ckb_core::cell::CellProvider;
use ckb_core::header::{BlockNumber, Header};
use ckb_core::transaction::{OutPoint, Transaction};
use ckb_miner::{AgentController, BlockTemplate};
use ckb_network::NetworkService;
use ckb_pool::txs_pool::TransactionPoolController;
use ckb_protocol::RelayMessage;
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::{ChainProvider, Shared};
use ckb_sync::RELAY_PROTOCOL_ID;
use flatbuffers::FlatBufferBuilder;
use jsonrpc_core::{Error, IoHandler, Result};
use jsonrpc_http_server::ServerBuilder;
use jsonrpc_server_utils::cors::AccessControlAllowOrigin;
use jsonrpc_server_utils::hosts::DomainsValidation;
use numext_fixed_hash::H256;
use std::collections::HashSet;
use std::sync::Arc;
use types::{BlockWithHash, CellOutputWithOutPoint, CellWithStatus, Config, TransactionWithHash};

build_rpc_trait! {
    pub trait ChainRpc {
        #[rpc(name = "get_block")]
        fn get_block(&self, H256) -> Result<Option<BlockWithHash>>;

        #[rpc(name = "get_transaction")]
        fn get_transaction(&self, H256) -> Result<Option<TransactionWithHash>>;

        #[rpc(name = "get_block_hash")]
        fn get_block_hash(&self, u64) -> Result<Option<H256>>;

        #[rpc(name = "get_tip_header")]
        fn get_tip_header(&self) -> Result<Header>;

        #[rpc(name = "get_cells_by_type_hash")]
        fn get_cells_by_type_hash(&self, H256, BlockNumber, BlockNumber) -> Result<Vec<CellOutputWithOutPoint>>;

        #[rpc(name = "get_current_cell")]
        fn get_current_cell(&self, OutPoint) -> Result<CellWithStatus>;

        #[rpc(name = "get_tip_block_number")]
        fn get_tip_block_number(&self) -> Result<BlockNumber>;
    }
}

pub struct ChainRpcImpl<CI> {
    pub shared: Shared<CI>,
}

impl<CI: ChainIndex + 'static> ChainRpc for ChainRpcImpl<CI> {
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

    // TODO: we need to build a proper index instead of scanning every time
    fn get_cells_by_type_hash(
        &self,
        type_hash: H256,
        from: BlockNumber,
        to: BlockNumber,
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
                                out_point: OutPoint::new(transaction.hash().clone(), i as u32),
                                capacity: output.capacity,
                                lock: output.lock.clone(),
                            });
                        }
                    }
                }
            }
        }
        Ok(result)
    }

    fn get_current_cell(&self, out_point: OutPoint) -> Result<CellWithStatus> {
        Ok(self.shared.cell(&out_point).into())
    }

    fn get_tip_block_number(&self) -> Result<BlockNumber> {
        Ok(self.shared.tip_header().read().number())
    }
}

build_rpc_trait! {
    pub trait PoolRpc {
        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"send_transaction","params": [{"version":2, "deps":[], "inputs":[], "outputs":[]}]}' -H 'content-type:application/json' 'http://localhost:8114'
        #[rpc(name = "send_transaction")]
        fn send_transaction(&self, Transaction) -> Result<H256>;
    }
}

pub struct PoolRpcImpl {
    pub network: Arc<NetworkService>,
    pub tx_pool: TransactionPoolController,
}

impl PoolRpc for PoolRpcImpl {
    fn send_transaction(&self, tx: Transaction) -> Result<H256> {
        let tx_hash = tx.hash().clone();
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
}

build_rpc_trait! {
    pub trait MinerRpc {
        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_block_template","params": ["0x1b1c832d02fdb4339f9868c8a8636c3d9dd10bd53ac7ce99595825bd6beeffb3", 1000, 1000]}' -H 'content-type:application/json' 'http://localhost:8114'
        #[rpc(name = "get_block_template")]
        fn get_block_template(&self, H256, usize, usize) -> Result<BlockTemplate>;

        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"submit_block","params": [{"header":{}, "uncles":[], "commit_transactions":[], "proposal_transactions":[]}]}' -H 'content-type:application/json' 'http://localhost:8114'
        #[rpc(name = "submit_block")]
        fn submit_block(&self, Block) -> Result<H256>;
    }
}

pub struct MinerRpcImpl<CI> {
    pub network: Arc<NetworkService>,
    pub shared: Shared<CI>,
    pub agent: AgentController,
    pub chain: ChainController,
}

impl<CI: ChainIndex + 'static> MinerRpc for MinerRpcImpl<CI> {
    fn get_block_template(
        &self,
        type_hash: H256,
        max_transactions: usize,
        max_proposals: usize,
    ) -> Result<BlockTemplate> {
        self.agent
            .get_block_template(type_hash, max_transactions, max_proposals)
            .map_err(|_| Error::internal_error())
    }

    fn submit_block(&self, block: Block) -> Result<H256> {
        let block = Arc::new(block);
        let ret = self.chain.process_block(Arc::clone(&block));
        if ret.is_ok() {
            // announce new block
            self.network.with_protocol_context(RELAY_PROTOCOL_ID, |nc| {
                let fbb = &mut FlatBufferBuilder::new();
                let message = RelayMessage::build_compact_block(fbb, &block, &HashSet::new());
                fbb.finish(message, None);
                for peer in nc.connected_peers() {
                    let _ = nc.send(peer, fbb.finished_data().to_vec());
                }
            });
            Ok(block.header().hash().clone())
        } else {
            debug!(target: "rpc", "submit_block process_block {:?}", ret);
            Err(Error::internal_error())
        }
    }
}

build_rpc_trait! {
    pub trait NetworkRpc {
        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"send_transaction","params": [{"version":2, "deps":[], "inputs":[], "outputs":[]}]}' -H 'content-type:application/json' 'http://localhost:8114'
        #[rpc(name = "local_node_id")]
        fn local_node_id(&self) -> Result<Option<String>>;
    }
}

struct NetworkRpcImpl {
    pub network: Arc<NetworkService>,
}

impl NetworkRpc for NetworkRpcImpl {
    fn local_node_id(&self) -> Result<Option<String>> {
        Ok(self.network.external_url())
    }
}

pub struct RpcServer {
    pub config: Config,
}

impl RpcServer {
    pub fn start<CI: ChainIndex + 'static>(
        &self,
        network: Arc<NetworkService>,
        shared: Shared<CI>,
        tx_pool: TransactionPoolController,
        chain: ChainController,
        agent: AgentController,
    ) where
        CI: ChainIndex,
    {
        let mut io = IoHandler::new();
        io.extend_with(
            ChainRpcImpl {
                shared: shared.clone(),
            }
            .to_delegate(),
        );
        io.extend_with(
            PoolRpcImpl {
                network: Arc::clone(&network),
                tx_pool,
            }
            .to_delegate(),
        );
        io.extend_with(
            MinerRpcImpl {
                shared,
                agent,
                chain,
                network: Arc::clone(&network),
            }
            .to_delegate(),
        );
        io.extend_with(NetworkRpcImpl { network }.to_delegate());

        let server = ServerBuilder::new(io)
            .cors(DomainsValidation::AllowOnly(vec![
                AccessControlAllowOrigin::Null,
                AccessControlAllowOrigin::Any,
            ]))
            .start_http(&self.config.listen_addr.parse().unwrap())
            .unwrap();

        info!(target: "rpc", "Now listening on {:?}", server.address());
        server.wait();
    }
}
