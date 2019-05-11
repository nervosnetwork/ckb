use crate::error::RPCError;
use ckb_core::cell::{resolve_transaction, CellProvider, CellStatus, HeaderProvider, HeaderStatus};
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
use std::sync::Arc;

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
        if o.cell.is_none() {
            return CellStatus::Unspecified;
        }
        let co = o.cell.as_ref().expect("checked below");
        self
            .chain_state
            .store()
            .get_cell_meta(&co.tx_hash, co.index)
            .map(CellStatus::live_cell)  // treat as live cell, regardless of live or dead
            .unwrap_or(CellStatus::Unknown)
    }
}

impl<'a, CS: ChainStore> HeaderProvider for DryRunner<'a, CS> {
    fn header(&self, o: &OutPoint) -> HeaderStatus {
        if o.block_hash.is_none() {
            return HeaderStatus::Unspecified;
        }
        let block_hash = o.block_hash.as_ref().expect("checked below");
        self.chain_state
            .store()
            .get_header(&block_hash)
            .map(|header| HeaderStatus::Live(Box::new(header)))
            .unwrap_or(HeaderStatus::Unknown)
    }
}

impl<'a, CS: ChainStore> DryRunner<'a, CS> {
    pub(crate) fn new(chain_state: &'a ChainState<CS>) -> Self {
        Self { chain_state }
    }

    pub(crate) fn run(&self, tx: CoreTransaction) -> Result<DryRunResult> {
        match resolve_transaction(&tx, &mut Default::default(), self, self) {
            Ok(resolved) => {
                let consensus = self.chain_state.consensus();
                let max_cycles = consensus.max_block_cycles;
                let script_config = self.chain_state.script_config();
                let store = self.chain_state.store();
                match ScriptVerifier::new(&resolved, Arc::clone(store), script_config)
                    .verify(max_cycles)
                {
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
