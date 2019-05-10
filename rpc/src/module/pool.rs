use crate::error::RPCError;
use ckb_core::transaction::Transaction as CoreTransaction;
use ckb_network::NetworkController;
use ckb_protocol::RelayMessage;
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_sync::NetworkProtocol;
use ckb_tx_pool_executor::TxPoolExecutor;
use flatbuffers::FlatBufferBuilder;
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;
use jsonrpc_types::{Transaction, TxPoolInfo};
use numext_fixed_hash::H256;
use std::convert::TryInto;
use std::sync::Arc;

#[rpc]
pub trait PoolRpc {
    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"send_transaction","params": [{"version":2, "deps":[], "inputs":[], "outputs":[]}]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "send_transaction")]
    fn send_transaction(&self, _tx: Transaction) -> Result<H256>;

    // curl -d '{"params": [], "method": "tx_pool_info", "jsonrpc": "2.0", "id": 2}' -H 'content-type:application/json' http://localhost:8114
    #[rpc(name = "tx_pool_info")]
    fn tx_pool_info(&self) -> Result<TxPoolInfo>;
}

pub(crate) struct PoolRpcImpl<CS> {
    network_controller: NetworkController,
    shared: Shared<CS>,
    tx_pool_executor: Arc<TxPoolExecutor<CS>>,
}

impl<CS: ChainStore + 'static> PoolRpcImpl<CS> {
    pub fn new(shared: Shared<CS>, network_controller: NetworkController) -> PoolRpcImpl<CS> {
        let tx_pool_executor = Arc::new(TxPoolExecutor::new(shared.clone()));
        PoolRpcImpl {
            shared,
            network_controller,
            tx_pool_executor,
        }
    }
}

impl<CS: ChainStore + 'static> PoolRpc for PoolRpcImpl<CS> {
    fn send_transaction(&self, tx: Transaction) -> Result<H256> {
        let tx: CoreTransaction = tx.try_into().map_err(|_| Error::parse_error())?;

        let result = self.tx_pool_executor.verify_and_add_tx_to_pool(tx.clone());

        match result {
            Ok(cycles) => {
                let fbb = &mut FlatBufferBuilder::new();
                let message = RelayMessage::build_transaction(fbb, &tx, cycles);
                fbb.finish(message, None);
                let data = fbb.finished_data().into();
                self.network_controller
                    .broadcast(NetworkProtocol::RELAY.into(), data);
                Ok(tx.hash().to_owned())
            }
            Err(e) => Err(RPCError::custom(RPCError::Invalid, e.to_string())),
        }
    }

    fn tx_pool_info(&self) -> Result<TxPoolInfo> {
        let chain_state = self.shared.chain_state().lock();
        let tx_pool = chain_state.tx_pool();
        Ok(TxPoolInfo {
            pending: tx_pool.pending_size(),
            proposed: tx_pool.proposed_size(),
            orphan: tx_pool.orphan_size(),
            last_txs_updated_at: chain_state.get_last_txs_updated_at().to_string(),
        })
    }
}
