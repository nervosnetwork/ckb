use crate::error::RPCError;
use ckb_core::transaction::Transaction as CoreTransaction;
use ckb_network::NetworkController;
use ckb_protocol::RelayMessage;
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_sync::NetworkProtocol;
use flatbuffers::FlatBufferBuilder;
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;
use jsonrpc_types::Transaction;
use numext_fixed_hash::H256;
use std::convert::TryInto;

#[rpc]
pub trait PoolRpc {
    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"send_transaction","params": [{"version":2, "deps":[], "inputs":[], "outputs":[]}]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "send_transaction")]
    fn send_transaction(&self, _tx: Transaction) -> Result<H256>;
}

pub(crate) struct PoolRpcImpl<CS> {
    pub network_controller: NetworkController,
    pub shared: Shared<CS>,
}

impl<CS: ChainStore + 'static> PoolRpc for PoolRpcImpl<CS> {
    fn send_transaction(&self, tx: Transaction) -> Result<H256> {
        let tx: CoreTransaction = tx.try_into().map_err(|_| Error::parse_error())?;

        let result = {
            let chain_state = self.shared.chain_state().lock();
            chain_state.add_tx_to_pool(tx.clone())
        };

        match result {
            Ok(_) => {
                let tx_hash = tx.hash();
                let fbb = &mut FlatBufferBuilder::new();
                let message = RelayMessage::build_transaction_hash(fbb, &tx_hash);
                fbb.finish(message, None);
                let data = fbb.finished_data().into();
                self.network_controller
                    .broadcast(NetworkProtocol::RELAY.into(), data);
                Ok(tx_hash)
            }
            Err(e) => Err(RPCError::custom(RPCError::Invalid, e.to_string())),
        }
    }
}
