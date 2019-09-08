use crate::error::RPCError;
use ckb_dao::DaoCalculator;
use ckb_jsonrpc_types::{Capacity, Cycle, DryRunResult, OutPoint, Script, Transaction};
use ckb_logger::error;
use ckb_shared::{shared::Shared, Snapshot};
use ckb_store::ChainStore;
use ckb_traits::chain_provider::ChainProvider;
use ckb_types::{
    core::cell::{resolve_transaction, CellProvider, CellStatus, HeaderChecker, UnresolvableError},
    packed,
    prelude::*,
    H256,
};
use ckb_verification::ScriptVerifier;
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;
use std::collections::HashSet;

#[rpc]
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
            Ok(capacity) => Ok(Capacity(capacity)),
            Err(err) => {
                error!("calculate_dao_maximum_withdraw error {:?}", err);
                Err(Error::internal_error())
            }
        }
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
            .get_cell_meta(&out_point.tx_hash(), out_point.index().unpack())
            .map(|mut cell_meta| {
                if with_data {
                    cell_meta.mem_cell_data = snapshot
                        .get_cell_data(&out_point.tx_hash(), out_point.index().unpack());
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
    ) -> std::result::Result<(), UnresolvableError> {
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
                match ScriptVerifier::new(&resolved, snapshot, self.shared.script_config())
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
