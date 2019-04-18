use crate::error::RPCError;
use ckb_core::transaction::Transaction as CoreTransaction;
use ckb_network::{NetworkController, ProtocolId};
use ckb_protocol::RelayMessage;
use ckb_shared::shared::Shared;
use ckb_shared::store::ChainStore;
use ckb_shared::tx_pool::types::PoolEntry;
use ckb_sync::NetworkProtocol;
use ckb_traits::chain_provider::ChainProvider;
use ckb_verification::TransactionError;
use flatbuffers::FlatBufferBuilder;
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;
use jsonrpc_types::{Transaction, TxTrace};
use log::{debug, warn};
use numext_fixed_hash::H256;
use std::convert::TryInto;

#[rpc]
pub trait TraceRpc {
    #[rpc(name = "trace_transaction")]
    fn trace_transaction(&self, _tx: Transaction) -> Result<H256>;

    #[rpc(name = "get_transaction_trace")]
    fn get_transaction_trace(&self, _hash: H256) -> Result<Option<Vec<TxTrace>>>;
}

pub(crate) struct TraceRpcImpl<CS> {
    pub network_controller: NetworkController,
    pub shared: Shared<CS>,
}

impl<CS: ChainStore + 'static> TraceRpc for TraceRpcImpl<CS> {
    fn trace_transaction(&self, tx: Transaction) -> Result<H256> {
        let tx: CoreTransaction = tx.try_into().map_err(|_| Error::parse_error())?;

        let mut chain_state = self.shared.chain_state().lock();
        let rtx = chain_state.resolve_tx_from_pool(&tx, &chain_state.tx_pool());
        let tx_result = chain_state.verify_rtx(&rtx, self.shared.consensus().max_block_cycles());

        match tx_result {
            Err(TransactionError::Unknown) => Err(RPCError::custom(
                RPCError::Staging,
                "unknown inputs or deps".to_string(),
            )),
            Err(err) => Err(RPCError::custom(RPCError::Invalid, format!("{:?}", err))),
            Ok(cycles) => {
                let tx_hash = tx.hash().clone();
                let entry = PoolEntry::new(tx.clone(), 0, Some(cycles));

                if !chain_state.mut_tx_pool().trace_tx(entry) {
                    // Duplicate tx
                    Ok(tx_hash)
                } else {
                    let fbb = &mut FlatBufferBuilder::new();
                    let message = RelayMessage::build_transaction(fbb, &tx, cycles);
                    fbb.finish(message, None);

                    self.network_controller.with_protocol_context(
                        NetworkProtocol::RELAY as ProtocolId,
                        |mut nc| {
                            for peer in nc.connected_peers() {
                                debug!(target: "rpc", "relay transaction {} to peer#{}", tx_hash, peer);
                                let ret = nc.send(peer, fbb.finished_data().to_vec());
                                if ret.is_err() {
                                    warn!(target: "rpc", "relay transaction error {:?}", ret);
                                }
                            }
                        },
                    );
                    Ok(tx_hash)
                }
            }
        }
    }

    fn get_transaction_trace(&self, hash: H256) -> Result<Option<Vec<TxTrace>>> {
        let chain_state = self.shared.chain_state().lock();
        let tx_pool = chain_state.tx_pool();
        Ok(tx_pool.get_tx_traces(&hash).cloned())
    }
}
