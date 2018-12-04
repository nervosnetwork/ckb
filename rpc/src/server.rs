use bigint::H256;
use ckb_chain::chain::ChainController;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::cell::CellProvider;
use ckb_core::header::{BlockNumber, Header, HeaderBuilder};
use ckb_core::transaction::{CellInput, CellOutput, OutPoint, Transaction, TransactionBuilder};
use ckb_miner::BlockTemplate;
use ckb_network::NetworkService;
use ckb_pool::txs_pool::TransactionPoolController;
use ckb_protocol::RelayMessage;
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::{ChainProvider, Shared};
use ckb_sync::RELAY_PROTOCOL_ID;
use ckb_time::now_ms;
use flatbuffers::FlatBufferBuilder;
use jsonrpc_core::{Error, IoHandler, Result};
use jsonrpc_http_server::ServerBuilder;
use jsonrpc_server_utils::cors::AccessControlAllowOrigin;
use jsonrpc_server_utils::hosts::DomainsValidation;
use std::cmp;
use std::collections::HashSet;
use std::sync::Arc;
use types::{BlockWithHash, CellOutputWithOutPoint, CellWithStatus, Config, TransactionWithHash};

build_rpc_trait! {
    pub trait ChainRpc {
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

        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_cells_by_type_hash","params": ["0x1b1c832d02fdb4339f9868c8a8636c3d9dd10bd53ac7ce99595825bd6beeffb3", 1, 10]}' -H 'content-type:application/json' 'http://localhost:8114'
        #[rpc(name = "get_cells_by_type_hash")]
        fn get_cells_by_type_hash(&self, H256, u64, u64) -> Result<Vec<CellOutputWithOutPoint>>;

        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_current_cell","params": [{"hash": "0x1b1c832d02fdb4339f9868c8a8636c3d9dd10bd53ac7ce99595825bd6beeffb3", "index": 1}]}' -H 'content-type:application/json' 'http://localhost:3030'
        #[rpc(name = "get_current_cell")]
        fn get_current_cell(&self, OutPoint) -> Result<CellWithStatus>;
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

    fn get_current_cell(&self, out_point: OutPoint) -> Result<CellWithStatus> {
        Ok(self.shared.cell(&out_point).into())
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
}

build_rpc_trait! {
    pub trait MinerRpc {
        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_block_template","params": ["0x1b1c832d02fdb4339f9868c8a8636c3d9dd10bd53ac7ce99595825bd6beeffb3"]}' -H 'content-type:application/json' 'http://localhost:8114'
        #[rpc(name = "get_block_template")]
        fn get_block_template(&self, H256) -> Result<BlockTemplate>;

        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"submit_block","params": [{"header":{}, "uncles":[], "commit_transactions":[], "proposal_transactions":[]}]}' -H 'content-type:application/json' 'http://localhost:8114'
        #[rpc(name = "submit_block")]
        fn submit_block(&self, Block) -> Result<H256>;
    }
}

pub struct MinerRpcImpl<CI> {
    pub network: Arc<NetworkService>,
    pub shared: Shared<CI>,
    pub tx_pool: TransactionPoolController,
    pub chain: ChainController,
}

impl<CI: ChainIndex + 'static> MinerRpc for MinerRpcImpl<CI> {
    fn get_block_template(&self, type_hash: H256) -> Result<BlockTemplate> {
        let (cellbase, commit_transactions, proposal_transactions, header_builder) = {
            let tip_header = self.shared.tip_header().read();
            let header = tip_header.inner();
            let now = cmp::max(now_ms(), header.timestamp() + 1);
            let difficulty = self
                .shared
                .calculate_difficulty(header)
                .expect("get difficulty");

            let (proposal_transactions, commit_transactions) =
                self.tx_pool.get_proposal_commit_transactions(1000, 1000);

            // NOTE: To generate different cellbase txid, we put header number in the input script
            let input = CellInput::new_cellbase_input(header.number() + 1);
            let block_reward = self.shared.block_reward(header.number() + 1);
            let mut fee = 0;
            for transaction in &commit_transactions {
                fee += self
                    .shared
                    .calculate_transaction_fee(transaction)
                    .map_err(|_| Error::internal_error())?
            }
            let output = CellOutput::new(block_reward + fee, Vec::new(), type_hash, None);

            let cellbase = TransactionBuilder::default()
                .input(input)
                .output(output)
                .build();

            let header_builder = HeaderBuilder::default()
                .parent_hash(&header.hash())
                .timestamp(now)
                .number(header.number() + 1)
                .difficulty(&difficulty)
                .cellbase_id(&cellbase.hash());
            (
                cellbase,
                commit_transactions,
                proposal_transactions,
                header_builder,
            )
        };

        let block = BlockBuilder::default()
            .commit_transaction(cellbase)
            .commit_transactions(commit_transactions)
            .proposal_transactions(proposal_transactions)
            .uncles(self.shared.get_tip_uncles())
            .with_header_builder(header_builder);

        Ok(BlockTemplate {
            raw_header: block.header().clone().into_raw(),
            uncles: block.uncles().to_vec(),
            commit_transactions: block.commit_transactions().to_vec(),
            proposal_transactions: block.proposal_transactions().to_vec(),
        })
    }

    fn submit_block(&self, block: Block) -> Result<H256> {
        let block = Arc::new(block);
        if self.chain.process_block(Arc::clone(&block)).is_ok() {
            // announce new block
            self.network.with_protocol_context(RELAY_PROTOCOL_ID, |nc| {
                let fbb = &mut FlatBufferBuilder::new();
                let message = RelayMessage::build_compact_block(fbb, &block, &HashSet::new());
                fbb.finish(message, None);
                for peer in nc.connected_peers() {
                    let _ = nc.send(peer, fbb.finished_data().to_vec());
                }
            });
            Ok(block.header().hash())
        } else {
            Err(Error::internal_error())
        }
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
    ) where
        CI: ChainIndex,
    {
        let mut io = IoHandler::new();
        io.extend_with(
            ChainRpcImpl {
                shared: shared.clone(),
            }.to_delegate(),
        );
        io.extend_with(
            PoolRpcImpl {
                network: Arc::clone(&network),
                tx_pool: tx_pool.clone(),
            }.to_delegate(),
        );
        io.extend_with(
            MinerRpcImpl {
                network,
                shared,
                tx_pool,
                chain,
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
