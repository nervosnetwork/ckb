use crate::error::RPCError;
use ckb_core::cell::{resolve_transaction, CellProvider, CellStatus};
use ckb_core::transaction::{OutPoint, Transaction as CoreTransaction};
use ckb_shared::chain_state::ChainState;
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_verification::ScriptVerifier;
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;
use jsonrpc_types::Transaction;
use numext_fixed_hash::H256;
use serde_derive::Serialize;
use std::convert::TryInto;

#[rpc]
pub trait ExperimentRpc {
    #[rpc(name = "_compute_transaction_hash")]
    fn compute_transaction_hash(&self, tx: Transaction) -> Result<H256>;

    #[rpc(name = "dry_run_transaction")]
    fn dry_run_transaction(&self, _tx: Transaction) -> Result<DryRunResult>;
}

pub(crate) struct ExperimentRpcImpl<CS> {
    pub shared: Shared<CS>,
}

impl<CS: ChainStore + 'static> ExperimentRpc for ExperimentRpcImpl<CS> {
    fn compute_transaction_hash(&self, tx: Transaction) -> Result<H256> {
        let tx: CoreTransaction = tx.try_into().map_err(|_| Error::parse_error())?;
        Ok(tx.hash().clone())
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
                let consensus = self.chain_state.consensus();
                let max_cycles = consensus.max_block_cycles;
                let vm = consensus.vm();
                let store = self.chain_state.get_store();
                match ScriptVerifier::new(&resolved, store, vm).verify(max_cycles) {
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
