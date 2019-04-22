use crate::agent::RpcAgentController;
use crate::error::RPCError;
use ckb_core::transaction::Transaction as CoreTransaction;
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;
use jsonrpc_types::{Transaction, TxTrace};
use numext_fixed_hash::H256;
use std::convert::TryInto;
use std::sync::Arc;

#[rpc]
pub trait TraceRpc {
    #[rpc(name = "trace_transaction")]
    fn trace_transaction(&self, _tx: Transaction) -> Result<H256>;

    #[rpc(name = "get_transaction_trace")]
    fn get_transaction_trace(&self, _hash: H256) -> Result<Option<Vec<TxTrace>>>;
}

pub(crate) struct TraceRpcImpl {
    pub agent_controller: Arc<RpcAgentController>,
}

impl TraceRpc for TraceRpcImpl {
    fn trace_transaction(&self, tx: Transaction) -> Result<H256> {
        let tx: CoreTransaction = tx.try_into().map_err(|_| Error::parse_error())?;
        self.agent_controller
            .trace_transaction(tx)
            .map_err(|err| RPCError::custom(RPCError::Invalid, format!("{:?}", err)))
    }

    fn get_transaction_trace(&self, hash: H256) -> Result<Option<Vec<TxTrace>>> {
        Ok(self.agent_controller.get_tx_traces(hash))
    }
}
