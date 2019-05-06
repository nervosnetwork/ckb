use crate::error::RPCError;
use ckb_core::cell::{resolve_transaction, CellProvider, CellStatus};
use ckb_core::transaction::{OutPoint, Transaction as CoreTransaction};
use ckb_network::NetworkController;
use ckb_shared::chain_state::ChainState;
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_verification::ScriptVerifier;
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;
use jsonrpc_types::{Transaction, TxTrace};
use numext_fixed_hash::H256;
use serde_derive::Serialize;
use std::convert::TryInto;

#[rpc]
pub trait TraceRpc {
    #[rpc(name = "trace_transaction")]
    fn trace_transaction(&self, _tx: Transaction) -> Result<H256>;

    #[rpc(name = "get_transaction_trace")]
    fn get_transaction_trace(&self, _hash: H256) -> Result<Option<Vec<TxTrace>>>;

    #[rpc(name = "dry_run_transaction")]
    fn dry_run_transaction(&self, _tx: Transaction) -> Result<DryRunResult>;
}

pub(crate) struct TraceRpcImpl<CS> {
    pub network_controller: NetworkController,
    pub shared: Shared<CS>,
}

impl<CS: ChainStore + 'static> TraceRpc for TraceRpcImpl<CS> {
    fn trace_transaction(&self, tx: Transaction) -> Result<H256> {
        let tx: CoreTransaction = tx.try_into().map_err(|_| Error::parse_error())?;
        let tx_hash = tx.hash();
        let mut chain_state = self.shared.chain_state().lock();
        chain_state.mut_tx_pool().trace_tx(tx);
        Ok(tx_hash)
    }

    fn get_transaction_trace(&self, hash: H256) -> Result<Option<Vec<TxTrace>>> {
        let chain_state = self.shared.chain_state().lock();
        let tx_pool = chain_state.tx_pool();
        Ok(tx_pool.get_tx_traces(&hash).cloned())
    }

    fn dry_run_transaction(&self, tx: Transaction) -> Result<DryRunResult> {
        let tx: CoreTransaction = tx.try_into().map_err(|_| Error::parse_error())?;
        let chain_state = self.shared.chain_state().lock();
        DryRunner::new(&chain_state).run(tx)
    }
}

#[derive(Serialize)]
pub struct DryRunResult {
    pub cycles: String,
}

// DryRunner dry run given transaction, and return the result, including execution cycles.
pub(crate) struct DryRunner<'a, CS> {
    chain_state: &'a ChainState<CS>,
}

impl<'a, CS: ChainStore> CellProvider for DryRunner<'a, CS> {
    fn cell(&self, o: &OutPoint) -> CellStatus {
        self
            .chain_state
            .get_store()
            .get_cell_meta(&o.tx_hash, o.index)
            .map(CellStatus::live_cell)  // treat as live cell, regardless of live or dead
            .unwrap_or(CellStatus::Unknown)
    }
}

impl<'a, CS: ChainStore> DryRunner<'a, CS> {
    pub(crate) fn new(chain_state: &'a ChainState<CS>) -> Self {
        Self { chain_state }
    }

    pub(crate) fn run(&self, tx: CoreTransaction) -> Result<DryRunResult> {
        match resolve_transaction(&tx, &mut Default::default(), self) {
            Ok(resolved) => {
                let max_cycles = self.chain_state.consensus().max_block_cycles;
                let store = self.chain_state.get_store();
                match ScriptVerifier::new(&resolved, store).verify(max_cycles) {
                    Ok(cycles) => Ok(DryRunResult {
                        cycles: cycles.to_string(),
                    }),
                    Err(err) => Err(RPCError::custom(RPCError::Invalid, format!("{:?}", err))),
                }
            }
            Err(err) => Err(RPCError::custom(RPCError::Invalid, format!("{:?}", err))),
        }
    }
}
