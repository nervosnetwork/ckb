use crate::error::RPCError;
use ckb_jsonrpc_types::{Transaction, TxPoolInfo};
use ckb_logger::error;
use ckb_network::PeerIndex;
use ckb_shared::shared::Shared;
use ckb_sync::SyncSharedState;
use ckb_tx_pool::{error::SubmitTxError, FeeRate};
use ckb_types::{core, packed, prelude::*, H256};
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;
use std::sync::Arc;

#[rpc(server)]
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
    min_fee_rate: FeeRate,
}

impl PoolRpcImpl {
    pub fn new(
        shared: Shared,
        sync_shared_state: Arc<SyncSharedState>,
        min_fee_rate: FeeRate,
    ) -> PoolRpcImpl {
        PoolRpcImpl {
            sync_shared_state,
            shared,
            min_fee_rate,
        }
    }
}

impl PoolRpc for PoolRpcImpl {
    fn send_transaction(&self, tx: Transaction) -> Result<H256> {
        let tx: packed::Transaction = tx.into();
        let tx: core::TransactionView = tx.into_view();

        let tx_pool = self.shared.tx_pool_controller();
        let submit_txs = tx_pool.submit_txs(vec![tx.clone()]);

        if let Err(e) = submit_txs {
            error!("send submit_txs request error {}", e);
            return Err(Error::internal_error());
        }

        match submit_txs.unwrap() {
            Ok(_) => {
                // workaround: we are using `PeerIndex(usize::max)` to indicate that tx hash source is itself.
                let peer_index = PeerIndex::new(usize::max_value());
                let hash = tx.hash();
                self.sync_shared_state
                    .state()
                    .tx_hashes()
                    .entry(peer_index)
                    .or_default()
                    .insert(hash.clone());
                Ok(hash.unpack())
            }
            Err(e) => {
                if let Some(e) = e.downcast_ref::<SubmitTxError>() {
                    match *e {
                        SubmitTxError::LowFeeRate(min_fee) => {
                            return Err(RPCError::custom(
                                RPCError::Invalid,
                                format!(
                                    "transaction fee rate lower than min_fee_rate: {} shannons/KB, min fee for current tx: {}",
                                    self.min_fee_rate, min_fee,
                                ),
                            ));
                        }
                        SubmitTxError::ExceededMaximumAncestorsCount => {
                            return Err(RPCError::custom(
                                RPCError::Invalid,
                                    "transaction exceeded maximum ancestors count limit, try send it later".to_string(),
                            ));
                        }
                    }
                }
                Err(RPCError::custom(RPCError::Invalid, format!("{:#}", e)))
            }
        }
    }

    fn tx_pool_info(&self) -> Result<TxPoolInfo> {
        let tx_pool = self.shared.tx_pool_controller();
        let get_tx_pool_info = tx_pool.get_tx_pool_info();
        if let Err(e) = get_tx_pool_info {
            error!("send get_tx_pool_info request error {}", e);
            return Err(Error::internal_error());
        };

        let tx_pool_info = get_tx_pool_info.unwrap();

        Ok(TxPoolInfo {
            pending: (tx_pool_info.pending_size as u64).into(),
            proposed: (tx_pool_info.proposed_size as u64).into(),
            orphan: (tx_pool_info.orphan_size as u64).into(),
            total_tx_size: (tx_pool_info.total_tx_size as u64).into(),
            total_tx_cycles: tx_pool_info.total_tx_cycles.into(),
            last_txs_updated_at: tx_pool_info.last_txs_updated_at.into(),
        })
    }
}
