use crate::error::RPCError;
use ckb_dao::DaoCalculator;
use ckb_jsonrpc_types::{
    Capacity, DryRunResult, EstimateResult, OutPoint, Script, Transaction, Uint64,
};
use ckb_shared::{shared::Shared, Snapshot};
use ckb_store::ChainStore;
use ckb_types::{
    core::cell::{resolve_transaction, CellProvider, CellStatus, HeaderChecker},
    packed,
    prelude::*,
    H256,
};
use ckb_verification::ScriptVerifier;
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use std::collections::HashSet;

#[rpc(server)]
pub trait ExperimentRpc {
    #[rpc(name = "_compute_transaction_hash")]
    fn compute_transaction_hash(&self, tx: Transaction) -> Result<H256>;

    #[rpc(name = "_compute_script_hash")]
    fn compute_script_hash(&self, script: Script) -> Result<H256>;

    #[rpc(name = "dry_run_transaction")]
    fn dry_run_transaction(&self, _tx: Transaction) -> Result<DryRunResult>;

    // Calculate the maximum withdraw one can get, given a referenced DAO cell,
    // and a withdraw block hash
    #[rpc(name = "calculate_dao_maximum_withdraw")]
    fn calculate_dao_maximum_withdraw(&self, _out_point: OutPoint, _hash: H256)
        -> Result<Capacity>;

    // Estimate fee
    #[rpc(name = "estimate_fee_rate")]
    fn estimate_fee_rate(&self, expect_confirm_blocks: Uint64) -> Result<EstimateResult>;
}

pub(crate) struct ExperimentRpcImpl {
    pub shared: Shared,
}

impl ExperimentRpc for ExperimentRpcImpl {
    fn compute_transaction_hash(&self, tx: Transaction) -> Result<H256> {
        let tx: packed::Transaction = tx.into();
        Ok(tx.calc_tx_hash().unpack())
    }

    fn compute_script_hash(&self, script: Script) -> Result<H256> {
        let script: packed::Script = script.into();
        Ok(script.calc_script_hash().unpack())
    }

    fn dry_run_transaction(&self, tx: Transaction) -> Result<DryRunResult> {
        let tx: packed::Transaction = tx.into();
        DryRunner::new(&self.shared).run(tx)
    }

    fn calculate_dao_maximum_withdraw(&self, out_point: OutPoint, hash: H256) -> Result<Capacity> {
        let snapshot: &Snapshot = &self.shared.snapshot();
        let consensus = snapshot.consensus();
        let calculator = DaoCalculator::new(consensus, snapshot);
        match calculator.maximum_withdraw(&out_point.into(), &hash.pack()) {
            Ok(capacity) => Ok(capacity.into()),
            Err(err) => Err(RPCError::from_ckb_error(err)),
        }
    }

    fn estimate_fee_rate(&self, _expect_confirm_blocks: Uint64) -> Result<EstimateResult> {
        Err(RPCError::custom(
            RPCError::Deprecated,
            "estimate_fee_rate have been deprecated due to it has availability and performance issue"
        ))
    }
}

// DryRunner dry run given transaction, and return the result, including execution cycles.
pub(crate) struct DryRunner<'a> {
    shared: &'a Shared,
}

impl<'a> CellProvider for DryRunner<'a> {
    fn cell(&self, out_point: &packed::OutPoint, with_data: bool) -> CellStatus {
        let snapshot = self.shared.snapshot();
        snapshot
            .get_cell(out_point)
            .map(|mut cell_meta| {
                if with_data {
                    cell_meta.mem_cell_data = snapshot.get_cell_data(out_point);
                }
                CellStatus::live_cell(cell_meta)
            })  // treat as live cell, regardless of live or dead
            .unwrap_or(CellStatus::Unknown)
    }
}

impl<'a> HeaderChecker for DryRunner<'a> {
    fn check_valid(
        &self,
        block_hash: &packed::Byte32,
    ) -> std::result::Result<(), ckb_error::Error> {
        self.shared.snapshot().check_valid(block_hash)
    }
}

impl<'a> DryRunner<'a> {
    pub(crate) fn new(shared: &'a Shared) -> Self {
        Self { shared }
    }

    pub(crate) fn run(&self, tx: packed::Transaction) -> Result<DryRunResult> {
        let snapshot: &Snapshot = &self.shared.snapshot();
        match resolve_transaction(tx.into_view(), &mut HashSet::new(), self, self) {
            Ok(resolved) => {
                let consensus = snapshot.consensus();
                let max_cycles = consensus.max_block_cycles;
                match ScriptVerifier::new(&resolved, snapshot).verify(max_cycles) {
                    Ok(cycles) => Ok(DryRunResult {
                        cycles: cycles.into(),
                    }),
                    Err(err) => Err(RPCError::custom_with_error(
                        RPCError::TransactionFailedToVerify,
                        err,
                    )),
                }
            }
            Err(err) => Err(RPCError::custom_with_error(
                RPCError::TransactionFailedToResolve,
                err,
            )),
        }
    }
}
