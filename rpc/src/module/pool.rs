use crate::error::RPCError;
use ckb_jsonrpc_types::{Timestamp, Transaction, TxPoolInfo, Unsigned};
use ckb_network::PeerIndex;
use ckb_shared::shared::Shared;
use ckb_sync::SyncSharedState;
use ckb_types::{core, packed, prelude::*, H256};
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
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

pub(crate) struct PoolRpcImpl {
    sync_shared_state: Arc<SyncSharedState>,
    shared: Shared,
}

impl PoolRpcImpl {
    pub fn new(shared: Shared, sync_shared_state: Arc<SyncSharedState>) -> PoolRpcImpl {
        PoolRpcImpl {
            sync_shared_state,
            shared,
        }
    }
}

impl PoolRpc for PoolRpcImpl {
    fn send_transaction(&self, tx: Transaction) -> Result<H256> {
        let tx: packed::Transaction = tx.into();
        let tx: core::TransactionView = tx.into_view();

        let tx_pool = self.shared.tx_pool_controller();
        let result = tx_pool.submit_txs(vec![tx.clone()]);

        match result {
            Ok(_) => {
                // workaround: we are using `PeerIndex(usize::max)` to indicate that tx hash source is itself.
                let peer_index = PeerIndex::new(usize::max_value());
                let hash = tx.hash().to_owned();
                self.sync_shared_state
                    .tx_hashes()
                    .entry(peer_index)
                    .or_default()
                    .insert(hash.clone());
                Ok(hash.unpack())
            }
            Err(e) => Err(RPCError::custom(RPCError::Invalid, e.to_string())),
        }
    }

    fn tx_pool_info(&self) -> Result<TxPoolInfo> {
        let tx_pool = self.shared.tx_pool_controller();
        let tx_pool_info = tx_pool.get_tx_pool_info();
        Ok(TxPoolInfo {
            pending: Unsigned(tx_pool_info.pending_size as u64),
            proposed: Unsigned(tx_pool_info.proposed_size as u64),
            orphan: Unsigned(tx_pool_info.orphan_size as u64),
            total_tx_size: Unsigned(tx_pool_info.total_tx_size as u64),
            total_tx_cycles: Unsigned(tx_pool_info.total_tx_cycles),
            last_txs_updated_at: Timestamp(tx_pool_info.last_txs_updated_at),
        })
    }
}
