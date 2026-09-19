//! Canonical transaction verification adapters and node-local execution budgets.

pub(crate) mod calibration;

use crate::{error::Reject, util::block_offload};
use ckb_chain_spec::consensus::Consensus;
use ckb_dao::DaoCalculator;
use ckb_script::ChunkCommand;
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_store::data_loader_wrapper::AsDataLoader;
use ckb_types::core::{
    Capacity, Cycle, FeeRate, TransactionView, cell::ResolvedTransaction,
    tx_pool::TRANSACTION_SIZE_LIMIT,
};
use ckb_verification::{
    ContextualTransactionVerifier, DaoScriptSizeVerifier, NonContextualTransactionVerifier,
    TxVerifyEnv,
    cache::{ScriptVerificationOutcome, ScriptVerificationProof},
};
use std::sync::Arc;
use tokio::sync::watch;

pub(crate) fn check_tx_fee_with_min_fee_rate(
    snapshot: &Snapshot,
    rtx: &ResolvedTransaction,
    tx_size: usize,
    min_fee_rate: FeeRate,
) -> Result<Capacity, Reject> {
    let fee = DaoCalculator::new(snapshot.consensus(), &snapshot.borrow_as_data_loader())
        .transaction_fee(rtx)
        .map_err(|err| {
            Reject::Malformed(
                format!("{err}"),
                "expect (outputs capacity) <= (inputs capacity)".to_owned(),
            )
        })?;
    // This early admission floor uses serialized size. Cycle-weighted ordering
    // is computed after verification has established the transaction's cycles.
    let min_fee = min_fee_rate.fee(tx_size as u64);
    if fee < min_fee {
        let reject = Reject::LowFeeRate(min_fee_rate, min_fee.as_u64(), fee.as_u64());
        ckb_logger::debug!("Reject tx {}", reject);
        return Err(reject);
    }
    Ok(fee)
}

pub(crate) fn non_contextual_verify(
    consensus: &Consensus,
    tx: &TransactionView,
) -> Result<(), Reject> {
    NonContextualTransactionVerifier::new(tx, consensus)
        .verify()
        .map_err(Reject::Verification)?;

    // Apply the pool's individual transaction size limit in addition to
    // canonical verification; a loose transaction must remain packable.
    let tx_size = tx.data().serialized_size_in_block() as u64;
    if tx_size > TRANSACTION_SIZE_LIMIT {
        return Err(Reject::ExceededTransactionSizeLimit(
            tx_size,
            TRANSACTION_SIZE_LIMIT,
        ));
    }
    // cellbase is only valid in a block, not as a loose transaction
    if tx.is_cellbase() {
        return Err(Reject::Malformed(
            "cellbase like".to_owned(),
            Default::default(),
        ));
    }

    Ok(())
}

fn verify_dao_script_size(
    snapshot: &Snapshot,
    rtx: Arc<ResolvedTransaction>,
) -> Result<(), ckb_error::Error> {
    let verifier = DaoScriptSizeVerifier::new(
        rtx,
        snapshot.cloned_consensus(),
        snapshot.borrow_as_data_loader(),
    );
    // The verifier owns the exact predicate for whether either of its branches
    // can reach the data provider. Keep only that potentially blocking path off
    // the async executor; the common non-DAO path still runs the complete
    // verifier, but avoids a compensating Tokio worker handoff.
    if verifier.may_load_cell_data() {
        block_offload(|| verifier.verify())
    } else {
        verifier.verify()
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TxPoolVerificationBudget {
    active_vm_time: std::time::Duration,
    vm_execution_mode: ckb_script::TxPoolVmExecutionMode,
}

impl TxPoolVerificationBudget {
    pub(crate) const fn new(
        active_vm_time: std::time::Duration,
        vm_execution_mode: ckb_script::TxPoolVmExecutionMode,
    ) -> Self {
        Self {
            active_vm_time,
            vm_execution_mode,
        }
    }
}

pub(crate) async fn verify_rtx(
    snapshot: Arc<Snapshot>,
    rtx: Arc<ResolvedTransaction>,
    tx_env: Arc<TxVerifyEnv>,
    cache_entry: Option<ScriptVerificationProof>,
    max_cycles: Cycle,
    command_rx: &mut watch::Receiver<ChunkCommand>,
    budget: Option<TxPoolVerificationBudget>,
) -> Result<ScriptVerificationOutcome, Reject> {
    let consensus = snapshot.cloned_consensus();
    let data_loader = snapshot.as_data_loader();

    let verifier = ContextualTransactionVerifier::new(
        Arc::clone(&rtx),
        consensus,
        data_loader,
        Arc::clone(&tx_env),
    );
    let outcome = match budget {
        // Local submissions verify synchronously. Reuse the blocking boundary
        // so this work cannot occupy the executor needed by chain controls.
        None => block_offload(|| verifier.verify_scripts(max_cycles, cache_entry))
            .map_err(Reject::Verification)?,
        Some(budget) => match verifier
            .verify_with_pause_and_budget(
                max_cycles,
                cache_entry,
                command_rx,
                budget.active_vm_time,
                budget.vm_execution_mode,
            )
            .await
            .map_err(Reject::Verification)?
        {
            ckb_verification::DeadlineVerificationOutcome::Verified(outcome) => outcome,
            ckb_verification::DeadlineVerificationOutcome::DeadlineExceeded => {
                return Err(Reject::ExcessiveVerifyTime);
            }
        },
    };
    verify_dao_script_size(&snapshot, rtx).map_err(Reject::Verification)?;
    Ok(outcome)
}
