use ckb_core::transaction::Transaction as CoreTransaction;
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::Shared;
use ckb_network::{NetworkService, ProtocolId};
use ckb_pool::txs_pool::{TransactionPoolController, TxTrace};
use ckb_protocol::RelayMessage;
use ckb_sync::NetworkProtocol;
use flatbuffers::FlatBufferBuilder;
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use jsonrpc_types::Transaction;
use log::debug;
use numext_fixed_hash::H256;
use std::sync::Arc;

#[rpc]
pub trait TraceRpc {
    #[rpc(name = "trace_transaction")]
    fn trace_transaction(&self, _tx: Transaction) -> Result<H256>;

    #[rpc(name = "get_transaction_trace")]
    fn get_transaction_trace(&self, _hash: H256) -> Result<Option<Vec<TxTrace>>>;
}

pub(crate) struct TraceRpcImpl<CI> {
    pub network: Arc<NetworkService>,
    pub shared: Shared<CI>,
}

impl<CI: ChainIndex + 'static> TraceRpc for TraceRpcImpl<CI> {
    fn trace_transaction(&self, tx: Transaction) -> Result<H256> {
        let tx: CoreTransaction = tx.into();
        let tx_hash = tx.hash().clone();
        let mut chain_state = self.shared.chain_state().lock();
        let tx_pool = chain_state.mut_tx_pool();
        tx_pool.trace_tx(tx.clone());

        let fbb = &mut FlatBufferBuilder::new();
        let message = RelayMessage::build_transaction(fbb, &tx);
        fbb.finish(message, None);

        self.network
            .with_protocol_context(NetworkProtocol::RELAY as ProtocolId, |nc| {
                for peer in nc.connected_peers() {
                    debug!(target: "rpc", "relay transaction {} to peer#{}", tx_hash, peer);
                    let _ = nc.send(peer, fbb.finished_data().to_vec());
                }
            });
        Ok(tx_hash)
    }

    fn get_transaction_trace(&self, hash: H256) -> Result<Option<Vec<TxTrace>>> {
        let chain_state = self.shared.chain_state().lock();
        let tx_pool = chain_state.tx_pool();
        Ok(tx_pool.get_tx_traces(&hash).cloned())
    }
}
