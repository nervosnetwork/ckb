use crate::types::Transaction;
use ckb_core::transaction::Transaction as CoreTransaction;
use ckb_network::NetworkService;
use ckb_pool::txs_pool::{TransactionPoolController, TxTrace};
use ckb_protocol::RelayMessage;
use ckb_sync::RELAY_PROTOCOL_ID;
use flatbuffers::FlatBufferBuilder;
use jsonrpc_core::Result;
use jsonrpc_macros::build_rpc_trait;
use log::debug;
use numext_fixed_hash::H256;
use std::sync::Arc;

build_rpc_trait! {
    pub trait TraceRpc {
        #[rpc(name = "trace_transaction")]
        fn trace_transaction(&self, _tx: Transaction) -> Result<H256>;

        #[rpc(name = "get_transaction_trace")]
        fn get_transaction_trace(&self, _hash: H256) -> Result<Option<Vec<TxTrace>>> ;
    }
}

pub(crate) struct TraceRpcImpl {
    pub network: Arc<NetworkService>,
    pub tx_pool: TransactionPoolController,
}

impl TraceRpc for TraceRpcImpl {
    fn trace_transaction(&self, tx: Transaction) -> Result<H256> {
        let tx: CoreTransaction = tx.into();
        let tx_hash = tx.hash().clone();
        let pool_result = self.tx_pool.trace_transaction(tx.clone());
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

    fn get_transaction_trace(&self, hash: H256) -> Result<Option<Vec<TxTrace>>> {
        Ok(self.tx_pool.get_transaction_trace(hash))
    }
}
