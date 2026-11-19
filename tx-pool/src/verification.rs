//! Canonical transaction verification adapters and node-local execution budgets.

pub(crate) mod calibration;
mod cpu_clock;
mod execution;

#[cfg(test)]
mod tests;

use crate::{error::Reject, util::block_offload_future};
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
    cache::{ScriptVerificationOutcome, TxVerificationCache},
};
use std::sync::Arc;
use tokio::sync::{RwLock, watch};

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

/// Pool expiry and RPC metadata use the time verification starts, after waiting
/// for the proof cache and before contextual checks or VM execution.
pub(crate) async fn verify_rtx(
    snapshot: Arc<Snapshot>,
    rtx: Arc<ResolvedTransaction>,
    tx_env: Arc<TxVerifyEnv>,
    cache: &RwLock<TxVerificationCache>,
    max_cycles: Cycle,
    command_rx: &mut watch::Receiver<ChunkCommand>,
    cpu_time: Option<std::time::Duration>,
) -> Result<(ScriptVerificationOutcome, u64), Reject> {
    block_offload_future(async move {
        let consensus = snapshot.cloned_consensus();
        let verifier = ContextualTransactionVerifier::new(
            Arc::clone(&rtx),
            Arc::clone(&consensus),
            snapshot.as_data_loader(),
            tx_env,
        );
        let cache_entry = cache.read().await.lookup(verifier.cache_key());
        let timestamp = ckb_systemtime::unix_time_as_millis();
        DaoScriptSizeVerifier::new(rtx, consensus, snapshot.borrow_as_data_loader())
            .verify()
            .map_err(Reject::Verification)?;
        let outcome = match cpu_time {
            None => verifier
                .verify_scripts(max_cycles, cache_entry)
                .map_err(Reject::Verification),
            Some(cpu_time) => verifier
                .verify_with_runner(
                    max_cycles,
                    cache_entry,
                    &mut execution::VmRunner {
                        command: command_rx,
                        remaining: cpu_time,
                    },
                )
                .await
                .map_err(Reject::Verification)?
                .ok_or(Reject::ExcessiveVerifyTime),
        }?;
        Ok((outcome, timestamp))
    })
    .await
}
