use crate::error::RPCError;
use ckb_core::cell::{resolve_transaction, CellProvider, CellStatus, HeaderProvider, HeaderStatus};
use ckb_core::script::Script as CoreScript;
use ckb_core::transaction::{
    CellOutput as CoreCellOutput, OutPoint as CoreOutPoint, Transaction as CoreTransaction,
};
use ckb_shared::chain_state::ChainState;
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_verification::ScriptVerifier;
use dao::calculate_maximum_withdraw;
use failure::Error as FailureError;
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;
use jsonrpc_types::{Capacity, Cycle, DryRunResult, JsonBytes, OutPoint, Script, Transaction};
use log::error;
use numext_fixed_hash::H256;
use serde_derive::Serialize;
use std::convert::TryInto;
use std::sync::Arc;

#[rpc]
pub trait ExperimentRpc {
    #[rpc(name = "_compute_transaction_hash")]
    fn compute_transaction_hash(&self, tx: Transaction) -> Result<H256>;

    #[rpc(name = "_compute_code_hash")]
    fn compute_code_hash(&self, data: JsonBytes) -> Result<H256>;

    #[rpc(name = "_compute_script_hash")]
    fn compute_script_hash(&self, script: Script) -> Result<H256>;

    #[rpc(name = "dry_run_transaction")]
    fn dry_run_transaction(&self, _tx: Transaction) -> Result<DryRunResult>;

    // Calculate the maximum withdraw one can get, given a referenced DAO cell,
    // and a withdraw block hash
    #[rpc(name = "calculate_dao_maximum_withdraw")]
    fn calculate_dao_maximum_withdraw(&self, _out_point: OutPoint, _hash: H256)
        -> Result<Capacity>;
}

pub(crate) struct ExperimentRpcImpl<CS> {
    pub shared: Shared<CS>,
}

impl<CS: ChainStore + 'static> ExperimentRpc for ExperimentRpcImpl<CS> {
    fn compute_transaction_hash(&self, tx: Transaction) -> Result<H256> {
        let tx: CoreTransaction = tx
            .try_into()
            .map_err(|err: FailureError| Error::invalid_params(err.to_string()))?;
        Ok(tx.hash().clone())
    }

    fn compute_code_hash(&self, data: JsonBytes) -> Result<H256> {
        let mut cell = CoreCellOutput::default();
        cell.data = data.into_bytes();
        Ok(cell.data_hash())
    }

    fn compute_script_hash(&self, script: Script) -> Result<H256> {
        let script: CoreScript = script
            .try_into()
            .map_err(FailureError::from)
            .map_err(|err| Error::invalid_params(err.to_string()))?;
        Ok(script.hash().clone())
    }

    fn dry_run_transaction(&self, tx: Transaction) -> Result<DryRunResult> {
        let tx: CoreTransaction = tx
            .try_into()
            .map_err(FailureError::from)
            .map_err(|err| Error::invalid_params(err.to_string()))?;
        let chain_state = self.shared.lock_chain_state();
        DryRunner::new(&chain_state).run(tx)
    }

    fn calculate_dao_maximum_withdraw(&self, out_point: OutPoint, hash: H256) -> Result<Capacity> {
        let chain_state = self.shared.lock_chain_state();
        match DaoWithdrawCalculator::new(&chain_state).calculate(
            out_point
                .clone()
                .try_into()
                .map_err(FailureError::from)
                .map_err(|err| Error::invalid_params(err.to_string()))?,
            hash,
        ) {
            Ok(capacity) => Ok(capacity),
            Err(err) => {
                error!(target: "rpc", "calculate_dao_maximum_withdraw error {:?}", err);
                Err(Error::internal_error())
            }
        }
    }
}

#[derive(Serialize, Debug)]
pub enum DaoWithdrawError {
    MissingCell,
    MissingHeader,
    CalculationError,
}

pub(crate) struct DaoWithdrawCalculator<'a, CS> {
    chain_state: &'a ChainState<CS>,
}

impl<'a, CS: ChainStore> DaoWithdrawCalculator<'a, CS> {
    pub(crate) fn new(chain_state: &'a ChainState<CS>) -> Self {
        Self { chain_state }
    }

    pub(crate) fn calculate(
        &self,
        out_point: CoreOutPoint,
        withdraw_hash: H256,
    ) -> ::std::result::Result<Capacity, DaoWithdrawError> {
        let cell_out_point = out_point.cell.ok_or(DaoWithdrawError::MissingCell)?;
        let (tx, block_hash) = self
            .chain_state
            .store()
            .get_transaction(&cell_out_point.tx_hash)
            .ok_or(DaoWithdrawError::MissingCell)?;
        let deposit_ext = self
            .chain_state
            .store()
            .get_block_ext(&block_hash)
            .ok_or(DaoWithdrawError::MissingHeader)?;
        let withdraw_ext = self
            .chain_state
            .store()
            .get_block_ext(&withdraw_hash)
            .ok_or(DaoWithdrawError::MissingHeader)?;

        let output = &tx.outputs()[cell_out_point.index as usize];
        let withdraw_capacity =
            calculate_maximum_withdraw(&output, &deposit_ext.dao_stats, &withdraw_ext.dao_stats)
                .map_err(|_| DaoWithdrawError::CalculationError)?;

        Ok(Capacity(withdraw_capacity))
    }
}

// DryRunner dry run given transaction, and return the result, including execution cycles.
pub(crate) struct DryRunner<'a, CS> {
    chain_state: &'a ChainState<CS>,
}

impl<'a, CS: ChainStore> CellProvider for DryRunner<'a, CS> {
    fn cell(&self, o: &CoreOutPoint) -> CellStatus {
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
    fn header(&self, o: &CoreOutPoint) -> HeaderStatus {
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
                        cycles: Cycle(cycles),
                    }),
                    Err(err) => Err(RPCError::custom(RPCError::Invalid, format!("{:?}", err))),
                }
            }
            Err(err) => Err(RPCError::custom(RPCError::Invalid, format!("{:?}", err))),
        }
    }
}
