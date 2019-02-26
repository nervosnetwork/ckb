use ckb_core::transaction::{ProposalShortId, Transaction as CoreTransaction};
use ckb_network::NetworkService;
use ckb_protocol::RelayMessage;
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::Shared;
use ckb_network::{NetworkService, ProtocolId};
use ckb_pool::txs_pool::TransactionPoolController;
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
pub trait PoolRpc {
    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"send_transaction","params": [{"version":2, "deps":[], "inputs":[], "outputs":[]}]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "send_transaction")]
    fn send_transaction(&self, _tx: Transaction) -> Result<H256>;

    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_pool_transaction","params": [""]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "get_pool_transaction")]
    fn get_pool_transaction(&self, _hash: H256) -> Result<Option<Transaction>>;
}

pub(crate) struct PoolRpcImpl<CI> {
    pub network: Arc<NetworkService>,
    pub shared: Shared<CI>,
}

impl<CI: ChainIndex + 'static> PoolRpc for PoolRpcImpl<CI> {
    fn send_transaction(&self, tx: Transaction) -> Result<H256> {
        let tx: CoreTransaction = tx.into();
        let tx_hash = tx.hash().clone();
        {
            let mut chain_state = self.shared.chain_state().lock();
            let tx_pool = chain_state.mut_tx_pool();
            let pool_result = tx_pool.enqueue_tx(tx.clone());
            debug!(target: "rpc", "send_transaction add to pool result: {:?}", pool_result);
        }

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

    fn get_pool_transaction(&self, hash: H256) -> Result<Option<Transaction>> {
        let id = ProposalShortId::from_h256(&hash);
        Ok(self
            .shared
            .chain_state()
            .lock()
            .tx_pool()
            .get_tx(&id)
            .map(|tx| (&tx).into()))
    }
}
