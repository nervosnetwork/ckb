use crate::error::RPCError;
use ckb_core::transaction::Transaction as CoreTransaction;
use ckb_network::{NetworkService, ProtocolId};
use ckb_protocol::RelayMessage;
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::Shared;
use ckb_shared::tx_pool::types::PoolEntry;
use ckb_shared::tx_pool::TxTrace;
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
        let tx_result = {
            let chain_state = self.shared.chain_state().lock();
            let rtx = chain_state.resolve_tx_from_pool(&tx, &chain_state.tx_pool());
            chain_state.verify_rtx(&rtx, self.shared.consensus().max_block_cycles())
        };

        match tx_result {
            Ok(cycles) => {
                let entry = PoolEntry::new(tx.clone(), 0, Some(cycles));
                self.shared
                    .chain_state()
                    .lock()
                    .mut_tx_pool()
                    .enqueue_tx(entry);
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
                self.shared
                    .chain_state()
                    .lock()
                    .mut_tx_pool()
                    .enqueue_tx(entry);
                Err(RPCError::custom(
                    RPCError::Staging,
                    "tx missing inputs".to_string(),
                ))
            }
            Err(err) => Err(RPCError::custom(RPCError::Invalid, format!("{:?}", err))),
        }
    }

    fn get_transaction_trace(&self, hash: H256) -> Result<Option<Vec<TxTrace>>> {
        let chain_state = self.shared.chain_state().lock();
        let tx_pool = chain_state.tx_pool();
        Ok(tx_pool.get_tx_traces(&hash).cloned())
    }
}
