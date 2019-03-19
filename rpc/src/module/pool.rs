use crate::error::RPCError;
use ckb_core::transaction::{ProposalShortId, Transaction as CoreTransaction};
use ckb_network::{NetworkService, ProtocolId};
use ckb_protocol::RelayMessage;
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::Shared;
use ckb_shared::tx_pool::types::PoolEntry;
use ckb_sync::NetworkProtocol;
use ckb_traits::chain_provider::ChainProvider;
use ckb_verification::TransactionError;
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
        let mut chain_state = self.shared.chain_state().lock();
        let rtx = chain_state.resolve_tx_from_pool(&tx, &chain_state.tx_pool());
        let tx_result = chain_state.verify_rtx(&rtx, self.shared.consensus().max_block_cycles());
        debug!(target: "rpc", "send_transaction add to pool result: {:?}", tx_result);
        match tx_result {
            Ok(cycles) => {
                let entry = PoolEntry::new(tx.clone(), 0, Some(cycles));
                chain_state.mut_tx_pool().enqueue_tx(entry);
                let fbb = &mut FlatBufferBuilder::new();
                let message = RelayMessage::build_transaction(fbb, &tx, cycles);
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
            Err(TransactionError::UnknownInput) => {
                let entry = PoolEntry::new(tx, 0, None);
                chain_state.mut_tx_pool().enqueue_tx(entry);
                Err(RPCError::custom(
                    RPCError::Staging,
                    "tx missing inputs".to_string(),
                ))
            }
            Err(err) => Err(RPCError::custom(RPCError::Invalid, format!("{:?}", err))),
        }
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
