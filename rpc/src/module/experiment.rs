use crate::error::RPCError;
use ckb_core::cell::resolve_transaction;
use ckb_core::script::Script as CoreScript;
use ckb_core::transaction::Transaction as CoreTransaction;
use ckb_dao::DaoCalculator;
use ckb_jsonrpc_types::{
    Capacity, Cycle, DryRunResult, EstimateResult, FeeRate, OutPoint, Script, Transaction, Unsigned,
};
use ckb_logger::error;
use ckb_shared::chain_state::ChainState;
use ckb_shared::fee_estimator::MAX_CONFIRM_BLOCKS;
use ckb_shared::shared::Shared;
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

    // Estimate fee
    #[rpc(name = "estimate_fee_rate")]
    fn estimate_fee_rate(&self, confirm_blocks: Unsigned) -> Result<EstimateResult>;
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

    fn estimate_fee_rate(&self, confirm_blocks: Unsigned) -> Result<EstimateResult> {
        let confirm_blocks = confirm_blocks.0 as usize;
        if confirm_blocks == 0 || confirm_blocks > MAX_CONFIRM_BLOCKS {
            return Err(RPCError::custom(
                RPCError::Invalid,
                format!(
                    "confirm_blocks should between 1 and {}, got {}",
                    MAX_CONFIRM_BLOCKS, confirm_blocks
                ),
            ));
        }
        let chain_state = self.shared.lock_chain_state();
        let fee_rate = chain_state.fee_estimator().estimate(confirm_blocks);
        if fee_rate.as_u64() == 0 {
            return Err(RPCError::custom(
                RPCError::Invalid,
                "samples is not enough, please try later".into(),
            ));
        }
        Ok(EstimateResult {
            fee_rate: FeeRate(fee_rate.as_u64()),
        })
    }
}

// DryRunner dry run given transaction, and return the result, including execution cycles.
pub(crate) struct DryRunner<'a> {
    chain_state: &'a ChainState,
}

impl<'a> DryRunner<'a> {
    pub(crate) fn new(chain_state: &'a ChainState) -> Self {
        Self { chain_state }
    }

    pub(crate) fn run(&self, tx: CoreTransaction) -> Result<DryRunResult> {
        match resolve_transaction(
            &tx,
            &mut Default::default(),
            self.chain_state,
            self.chain_state,
        )
        .or_else(|_| {
            let tx_pool = self.chain_state.tx_pool();
            self.chain_state
                .resolve_tx_from_pending_and_proposed(&tx, &tx_pool)
        }) {
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
