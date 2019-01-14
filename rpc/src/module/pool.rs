use crate::types::Transaction;
use ckb_core::transaction::Transaction as CoreTransaction;
use ckb_network::NetworkService;
use ckb_pool::txs_pool::TransactionPoolController;
use ckb_protocol::RelayMessage;
use ckb_sync::RELAY_PROTOCOL_ID;
use flatbuffers::FlatBufferBuilder;
use jsonrpc_core::Result;
use jsonrpc_macros::build_rpc_trait;
use log::debug;
use numext_fixed_hash::H256;
use std::sync::Arc;

build_rpc_trait! {
    pub trait PoolRpc {
        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"send_transaction","params": [{"version":2, "deps":[], "inputs":[], "outputs":[]}]}' -H 'content-type:application/json' 'http://localhost:8114'
        #[rpc(name = "send_transaction")]
        fn send_transaction(&self, _tx: Transaction) -> Result<H256>;
    }
}

pub(crate) struct PoolRpcImpl {
    pub network: Arc<NetworkService>,
    pub tx_pool: TransactionPoolController,
}

impl PoolRpc for PoolRpcImpl {
    fn send_transaction(&self, tx: Transaction) -> Result<H256> {
        let tx: CoreTransaction = tx.into();
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
