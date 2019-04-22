use crate::agent::RpcAgentController;
use crate::error::RPCError;
use ckb_core::transaction::{ProposalShortId, Transaction as CoreTransaction};
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;
use jsonrpc_types::Transaction;
use numext_fixed_hash::H256;
use std::convert::TryInto;
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

pub(crate) struct PoolRpcImpl {
    pub agent_controller: Arc<RpcAgentController>,
}

impl PoolRpc for PoolRpcImpl {
    fn send_transaction(&self, tx: Transaction) -> Result<H256> {
        let tx: CoreTransaction = tx.try_into().map_err(|_| Error::parse_error())?;
        self.agent_controller
            .send_transaction(tx)
            .map(Into::into)
            .map_err(|err| RPCError::custom(RPCError::Invalid, format!("{:?}", err)))
    }

    fn get_pool_transaction(&self, hash: H256) -> Result<Option<Transaction>> {
        let id = ProposalShortId::from_tx_hash(&hash);
        Ok(self
            .agent_controller
            .get_pool_transaction(id)
            .as_ref()
            .map(Into::into))
    }
}
