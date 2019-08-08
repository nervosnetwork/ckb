use crate::error::RPCError;
use ckb_core::cell::{resolve_transaction, CellProvider, CellStatus, HeaderChecker};
use ckb_core::script::Script as CoreScript;
use ckb_core::transaction::{OutPoint as CoreOutPoint, Transaction as CoreTransaction};
use ckb_dao::DaoCalculator;
use ckb_jsonrpc_types::{Capacity, Cycle, DryRunResult, OutPoint, Script, Transaction};
use ckb_logger::error;
use ckb_shared::chain_state::ChainState;
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_verification::ScriptVerifier;
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;
use numext_fixed_hash::H256;

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
        let tx: CoreTransaction = tx.into();
        Ok(tx.hash().to_owned())
    }

    fn compute_script_hash(&self, script: Script) -> Result<H256> {
        let script: CoreScript = script.into();
        Ok(script.hash())
    }

    fn dry_run_transaction(&self, tx: Transaction) -> Result<DryRunResult> {
        let tx: CoreTransaction = tx.into();
        let chain_state = self.shared.lock_chain_state();
        DryRunner::new(&chain_state).run(tx)
    }

    fn calculate_dao_maximum_withdraw(&self, out_point: OutPoint, hash: H256) -> Result<Capacity> {
        let chain_state = self.shared.lock_chain_state();
        let consensus = &chain_state.consensus();
        let calculator = DaoCalculator::new(consensus, chain_state.store());
        match calculator.maximum_withdraw(&out_point.into(), &hash) {
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
    chain_state: &'a ChainState,
}

impl<'a> CellProvider for DryRunner<'a> {
    fn cell(&self, out_point: &CoreOutPoint, with_data: bool) -> CellStatus {
        self
            .chain_state
            .store()
            .get_cell_meta(&out_point.tx_hash, out_point.index)
            .map(|mut cell_meta| {
                if with_data {
                    cell_meta.mem_cell_data = self
                        .chain_state
                        .store()
                        .get_cell_data(&out_point.tx_hash, out_point.index);
                }
                CellStatus::live_cell(cell_meta)
            })  // treat as live cell, regardless of live or dead
            .unwrap_or(CellStatus::Unknown)
    }
}

impl<'a> HeaderChecker for DryRunner<'a> {
    fn is_valid(&self, block_hash: &H256) -> bool {
        self.chain_state
            .store()
            .get_block_number(block_hash)
            .is_some()
    }
}

impl<'a> DryRunner<'a> {
    pub(crate) fn new(chain_state: &'a ChainState) -> Self {
        Self { chain_state }
    }

    pub(crate) fn run(&self, tx: CoreTransaction) -> Result<DryRunResult> {
        match resolve_transaction(&tx, &mut Default::default(), self, self) {
            Ok(resolved) => {
                let consensus = self.chain_state.consensus();
                let max_cycles = consensus.max_block_cycles;
                let script_config = self.chain_state.script_config();
                match ScriptVerifier::new(&resolved, self.chain_state.store(), script_config)
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
