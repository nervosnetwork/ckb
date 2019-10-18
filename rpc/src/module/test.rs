use crate::error::RPCError;
use ckb_chain::{chain::ChainController, switch::Switch};
use ckb_jsonrpc_types::{Block, Cycle, Transaction};
use ckb_logger::error;
use ckb_network::NetworkController;
use ckb_shared::shared::Shared;
use ckb_sync::NetworkProtocol;
use ckb_types::core::TransactionView;
use ckb_types::packed::Byte32;
use ckb_types::{bytes, core, packed, prelude::*, H256};
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
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

    #[rpc(name = "send_mock_transaction")]
    fn send_mock_transaction(&self, mock_transaction: Transaction) -> Result<H256>;

    #[rpc(name = "send_mock_block")]
    fn send_mock_block(&self, mock_block: Block) -> Result<H256>;
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
        let block: packed::Block = data.into();
        let block: Arc<core::BlockView> = Arc::new(block.into_view());
        let ret = self
            .chain
            .internal_process_block(Arc::clone(&block), Switch::DISABLE_ALL);
        if ret.is_ok() {
            Ok(Some(block.hash().unpack()))
        } else {
            error!("process_block_without_verify error: {:?}", ret);
            Ok(None)
        }
    }

    fn broadcast_transaction(&self, transaction: Transaction, cycles: Cycle) -> Result<H256> {
        let tx: packed::Transaction = transaction.into();
        let hash = tx.calc_tx_hash();
        let relay_tx = packed::RelayTransaction::new_builder()
            .cycles(cycles.value().pack())
            .transaction(tx)
            .build();
        let relay_txs = packed::RelayTransactions::new_builder()
            .transactions(vec![relay_tx].pack())
            .build();
        let message = packed::RelayMessage::new_builder().set(relay_txs).build();
        let data = message.as_slice().into();
        if let Err(err) = self
            .network_controller
            .broadcast(NetworkProtocol::RELAY.into(), data)
        {
            error!("Broadcast transaction failed: {:?}", err);
            Err(RPCError::custom(RPCError::Invalid, err.to_string()))
        } else {
            Ok(hash.unpack())
        }
    }

    fn send_mock_transaction(&self, mock_transaction: Transaction) -> Result<H256> {
        let tx: packed::Transaction = mock_transaction.into();
        let mut tx = tx.into_view();
        if is_fake_transaction(&tx) {
            tx = to_fake_transaction(tx);
        }
        match self
            .shared
            .tx_pool_controller()
            .submit_txs(vec![tx.clone()])
        {
            Ok(_) => Ok(tx.hash().unpack()),
            Err(err) => Err(RPCError::custom(
                RPCError::Invalid,
                format!("send_mock_transaction error: {:?}", err),
            )),
        }
    }

    fn send_mock_block(&self, mock_block: Block) -> Result<H256> {
        let block: packed::Block = mock_block.into();
        let block = block.into_view();
        let fake_block = block
            .as_advanced_builder()
            .set_transactions(vec![])
            .transactions(block.transactions().into_iter().map(|tx| {
                if is_fake_transaction(&tx) {
                    to_fake_transaction(tx)
                } else {
                    tx
                }
            }))
            .build();
        match self.chain.process_block(Arc::new(fake_block.clone())) {
            Ok(_) => Ok(fake_block.hash().unpack()),
            Err(err) => Err(RPCError::custom(
                RPCError::Invalid,
                format!("send_mock_block error: {:?}", err),
            )),
        }
    }
}

fn is_fake_transaction(tx: &TransactionView) -> bool {
    tx.witnesses()
        .get(0)
        .map(|witness| witness.len() == Byte32::TOTAL_SIZE)
        .unwrap_or(false)
}

fn to_fake_transaction(tx: TransactionView) -> TransactionView {
    let fake_hash = {
        let witness: bytes::Bytes = tx.witnesses().get(0).expect("expect 0th witness").unpack();
        Byte32::from_slice(&witness).expect("expect 0th witness is Byte32")
    };
    tx.fake_hash(fake_hash)
}
