use crate::error::RPCError;
use ckb_chain::chain::ChainController;
use ckb_core::block::Block as CoreBlock;
use ckb_core::transaction::Transaction as CoreTransaction;
use ckb_jsonrpc_types::{Block, Cycle, Transaction};
use ckb_logger::error;
use ckb_network::NetworkController;
use ckb_protocol::RelayMessage;
use ckb_shared::shared::Shared;
use ckb_sync::NetworkProtocol;
use flatbuffers::FlatBufferBuilder;
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use numext_fixed_hash::H256;
use std::sync::Arc;

#[rpc]
pub trait IntegrationTestRpc {
    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"add_node","params": ["QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS", "/ip4/192.168.2.100/tcp/30002"]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "add_node")]
    fn add_node(&self, peer_id: String, address: String) -> Result<()>;

    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"remove_node","params": ["QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS"]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "remove_node")]
    fn remove_node(&self, peer_id: String) -> Result<()>;

    #[rpc(name = "process_block_without_verify")]
    fn process_block_without_verify(&self, data: Block) -> Result<Option<H256>>;

    #[rpc(name = "broadcast_transaction")]
    fn broadcast_transaction(&self, transaction: Transaction, cycles: Cycle) -> Result<H256>;
}

pub(crate) struct IntegrationTestRpcImpl {
    pub network_controller: NetworkController,
    pub shared: Shared,
    pub chain: ChainController,
}

impl IntegrationTestRpc for IntegrationTestRpcImpl {
    fn add_node(&self, peer_id: String, address: String) -> Result<()> {
        self.network_controller.add_node(
            &peer_id.parse().expect("invalid peer_id"),
            address.parse().expect("invalid address"),
        );
        Ok(())
    }

    fn remove_node(&self, peer_id: String) -> Result<()> {
        self.network_controller
            .remove_node(&peer_id.parse().expect("invalid peer_id"));
        Ok(())
    }

    fn process_block_without_verify(&self, data: Block) -> Result<Option<H256>> {
        let block: Arc<CoreBlock> = Arc::new(data.into());
        let ret = self.chain.process_block(Arc::clone(&block), false);
        if ret.is_ok() {
            Ok(Some(block.header().hash().to_owned()))
        } else {
            error!("process_block_without_verify error: {:?}", ret);
            Ok(None)
        }
    }

    fn broadcast_transaction(&self, transaction: Transaction, cycles: Cycle) -> Result<H256> {
        let tx: CoreTransaction = transaction.into();
        let fbb = &mut FlatBufferBuilder::new();
        let hash = tx.hash().to_owned();
        let relay_tx = (tx, cycles.0);
        let message = RelayMessage::build_transactions(fbb, &[relay_tx]);
        fbb.finish(message, None);
        let data = fbb.finished_data().into();
        if let Err(err) = self
            .network_controller
            .broadcast(NetworkProtocol::RELAY.into(), data)
        {
            error!("Broadcast transaction failed: {:?}", err);
            Err(RPCError::custom(RPCError::Invalid, err.to_string()))
        } else {
            Ok(hash)
        }
    }
}
