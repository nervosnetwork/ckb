use ckb_core::transaction::Transaction as CoreTransaction;
use ckb_network::NetworkController;
use ckb_shared::shared::Shared;
use ckb_shared::store::ChainStore;
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;
use jsonrpc_types::{Transaction, TxTrace};
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
        let tx_hash = tx.hash().clone();
        let mut chain_state = self.shared.chain_state().lock();
        chain_state.mut_tx_pool().trace_tx(tx);
        Ok(tx_hash)
    }

    fn get_transaction_trace(&self, hash: H256) -> Result<Option<Vec<TxTrace>>> {
        let chain_state = self.shared.chain_state().lock();
        let tx_pool = chain_state.tx_pool();
        Ok(tx_pool.get_tx_traces(&hash).cloned())
    }
}
