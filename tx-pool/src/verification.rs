//! Canonical transaction verification adapters and node-local execution budgets.

pub(crate) mod calibration;
mod cpu_clock;
mod execution;

#[cfg(test)]
mod tests;

use crate::{authority::service::ComputePool, error::Reject, util::block_offload_future};
use ckb_chain_spec::consensus::{Consensus, TYPE_ID_CODE_HASH};
use ckb_dao::DaoCalculator;
use ckb_script::ChunkCommand;
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_store::data_loader_wrapper::AsDataLoader;
use ckb_types::{
    core::{
        Capacity, Cycle, FeeRate, ScriptHashType, TransactionView, cell::ResolvedTransaction,
        tx_pool::TRANSACTION_SIZE_LIMIT,
    },
    packed::Byte32,
};
use ckb_verification::{
    CellDataProvider, ContextualTransactionVerifier, DaoScriptSizeVerifier, EpochProvider,
    ExtensionProvider, HeaderFieldsProvider, HeaderProvider, NonContextualTransactionVerifier,
    TxVerifyEnv,
    cache::{ScriptVerificationOutcome, TxVerificationCache},
};
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::sync::{RwLock, watch};

/// Count distinct non-TypeID VM script groups executed for a resolved transaction.
///
/// Used only to scale the local VM CPU-time startup allowance proportionally to
/// legitimate multi-group transactions without creating VM machines upfront.
pub(crate) fn script_group_count(rtx: &ResolvedTransaction) -> usize {
    let type_id_hash: Byte32 = TYPE_ID_CODE_HASH.into();
    let is_type_id = |script: &ckb_types::packed::Script| {
        script.code_hash() == type_id_hash
            && Into::<u8>::into(script.hash_type()) == Into::<u8>::into(ScriptHashType::Type)
    };
    let mut locks: HashSet<Byte32> = HashSet::new();
    let mut types: HashSet<Byte32> = HashSet::new();
    for input in &rtx.resolved_inputs {
        locks.insert(input.cell_output.lock().calc_script_hash());
        if let Some(script) = input.cell_output.type_().to_opt()
            && !is_type_id(&script)
        {
            types.insert(script.calc_script_hash());
        }
    }
    for output in rtx.transaction.outputs() {
        if let Some(script) = output.type_().to_opt()
            && !is_type_id(&script)
        {
            types.insert(script.calc_script_hash());
        }
    }
    locks.len().saturating_add(types.len()).max(1)
}

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

/// Synchronous fast path for uncapped local transaction verification when the
/// sharded proof cache read lock is immediately available (100% of production
/// calls). Returns `None` only if the outer `RwLock<TxVerificationCache>` is
/// write-locked so the caller can fall back to the async path.
pub(crate) fn try_verify_rtx_sync(
    snapshot: &Arc<Snapshot>,
    rtx: &Arc<ResolvedTransaction>,
    tx_env: Arc<TxVerifyEnv>,
    cache: &RwLock<TxVerificationCache>,
    max_cycles: Cycle,
) -> Option<Result<(ScriptVerificationOutcome, u64), Reject>> {
    let guard = cache.try_read().ok()?;
    let consensus = snapshot.cloned_consensus();
    let verifier = ContextualTransactionVerifier::new(
        Arc::clone(rtx),
        Arc::clone(&consensus),
        snapshot.as_data_loader(),
        tx_env,
    );
    let cache_entry = guard.lookup(verifier.cache_key());
    drop(guard);
    let timestamp = ckb_systemtime::unix_time_as_millis();
    if let Err(err) =
        DaoScriptSizeVerifier::new(Arc::clone(rtx), consensus, snapshot.borrow_as_data_loader())
            .verify()
    {
        return Some(Err(Reject::Verification(err)));
    }
    let outcome = match verifier.verify_scripts(max_cycles, cache_entry) {
        Ok(outcome) => outcome,
        Err(err) => return Some(Err(Reject::Verification(err))),
    };
    if let Some(proof) = outcome.executed_proof()
        && let Ok(guard) = cache.try_read()
    {
        guard.insert_concurrent(proof);
    }
    Some(Ok((outcome, timestamp)))
}

fn map_compute_dispatch_error(error: crate::authority::service::Error) -> Reject {
    match error {
        crate::authority::service::Error::Full(reason) => Reject::Full(reason.to_string()),
        _ => Reject::Verification(ckb_script::ScriptError::Interrupts.unknown_source().into()),
    }
}

/// Pool expiry and RPC metadata use the time verification starts, after waiting
/// for the proof cache and before contextual checks or VM execution.
#[cfg(test)]
pub(crate) async fn verify_rtx(
    snapshot: Arc<Snapshot>,
    rtx: Arc<ResolvedTransaction>,
    tx_env: Arc<TxVerifyEnv>,
    cache: &RwLock<TxVerificationCache>,
    max_cycles: Cycle,
    command_rx: &mut watch::Receiver<ChunkCommand>,
    cpu_time: Option<Duration>,
) -> Result<(ScriptVerificationOutcome, u64), Reject> {
    verify_rtx_with_compute(
        snapshot,
        rtx,
        tx_env,
        cache,
        max_cycles,
        command_rx,
        cpu_time,
        None,
        #[cfg(any(test, feature = "internal"))]
        None,
    )
    .await
    .map(|(outcome, timestamp, _)| (outcome, timestamp))
}

/// Pool verification returning `(outcome, start_timestamp_ms, vm_cpu_spent)`.
///
/// When `compute` is supplied, both synchronous contextual/script preparation
/// (DAO check, time-relative/header checks, cell-dep data-hash loading, and
/// script grouping) and budgeted VM slices run on the pool's dedicated
/// `ComputePool` workers rather than blocking Tokio runtime worker threads.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn verify_rtx_with_compute(
    snapshot: Arc<Snapshot>,
    rtx: Arc<ResolvedTransaction>,
    tx_env: Arc<TxVerifyEnv>,
    cache: &RwLock<TxVerificationCache>,
    max_cycles: Cycle,
    command_rx: &mut watch::Receiver<ChunkCommand>,
    cpu_time: Option<Duration>,
    compute: Option<&ComputePool>,
    #[cfg(any(test, feature = "internal"))] before_prepare: Option<Box<dyn FnOnce() + Send>>,
) -> Result<(ScriptVerificationOutcome, u64, Duration), Reject> {
    let consensus = snapshot.cloned_consensus();
    let data_loader = snapshot.as_data_loader();
    verify_rtx_with_loader(
        consensus,
        data_loader,
        rtx,
        tx_env,
        cache,
        max_cycles,
        command_rx,
        cpu_time,
        compute,
        #[cfg(any(test, feature = "internal"))]
        before_prepare,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn verify_rtx_with_loader<DL>(
    consensus: Arc<Consensus>,
    data_loader: DL,
    rtx: Arc<ResolvedTransaction>,
    tx_env: Arc<TxVerifyEnv>,
    cache: &RwLock<TxVerificationCache>,
    max_cycles: Cycle,
    command_rx: &mut watch::Receiver<ChunkCommand>,
    cpu_time: Option<Duration>,
    compute: Option<&ComputePool>,
    #[cfg(any(test, feature = "internal"))] before_prepare: Option<Box<dyn FnOnce() + Send>>,
) -> Result<(ScriptVerificationOutcome, u64, Duration), Reject>
where
    DL: CellDataProvider
        + HeaderProvider
        + ExtensionProvider
        + HeaderFieldsProvider
        + EpochProvider
        + Send
        + Sync
        + Clone
        + 'static,
{
    let verifier = ContextualTransactionVerifier::new(
        Arc::clone(&rtx),
        Arc::clone(&consensus),
        data_loader.clone(),
        tx_env,
    );
    let cache_entry = match cache.try_read() {
        Ok(guard) => guard.lookup(verifier.cache_key()),
        Err(_) => cache.read().await.lookup(verifier.cache_key()),
    };
    let timestamp = ckb_systemtime::unix_time_as_millis();
    match cpu_time {
        None => {
            if let Some(compute) = compute {
                let outcome = compute
                    .run_permitted(move || {
                        #[cfg(any(test, feature = "internal"))]
                        if let Some(hook) = before_prepare {
                            hook();
                        }
                        DaoScriptSizeVerifier::new(rtx, consensus, data_loader)
                            .verify()
                            .map_err(Reject::Verification)?;
                        verifier
                            .verify_scripts(max_cycles, cache_entry)
                            .map_err(Reject::Verification)
                    })
                    .await
                    .map_err(map_compute_dispatch_error)??;
                Ok((outcome, timestamp, Duration::ZERO))
            } else {
                block_offload_future(async move {
                    #[cfg(any(test, feature = "internal"))]
                    if let Some(hook) = before_prepare {
                        hook();
                    }
                    DaoScriptSizeVerifier::new(rtx, consensus, data_loader)
                        .verify()
                        .map_err(Reject::Verification)?;
                    let outcome = verifier
                        .verify_scripts(max_cycles, cache_entry)
                        .map_err(Reject::Verification)?;
                    Ok((outcome, timestamp, Duration::ZERO))
                })
                .await
            }
        }
        Some(budget) => {
            if let Some(compute) = compute {
                let prepared = compute
                    .run_permitted(move || {
                        #[cfg(any(test, feature = "internal"))]
                        if let Some(hook) = before_prepare {
                            hook();
                        }
                        DaoScriptSizeVerifier::new(rtx, consensus, data_loader)
                            .verify()
                            .map_err(Reject::Verification)?;
                        verifier
                            .prepare_for_runner(max_cycles, cache_entry)
                            .map_err(Reject::Verification)
                    })
                    .await
                    .map_err(map_compute_dispatch_error)??;
                let mut runner = execution::VmRunner {
                    command: command_rx,
                    remaining: budget,
                    compute: Some(compute),
                };
                let outcome = prepared
                    .verify_with_runner(&mut runner)
                    .await
                    .map_err(Reject::Verification)?;
                let spent = budget.saturating_sub(runner.remaining);
                crate::metrics::record_verify_cpu_budget_ratio(spent, budget);
                let outcome = outcome.ok_or(Reject::ExcessiveVerifyTime)?;
                Ok((outcome, timestamp, spent))
            } else {
                block_offload_future(async move {
                    #[cfg(any(test, feature = "internal"))]
                    if let Some(hook) = before_prepare {
                        hook();
                    }
                    DaoScriptSizeVerifier::new(rtx, consensus, data_loader)
                        .verify()
                        .map_err(Reject::Verification)?;
                    let mut runner = execution::VmRunner {
                        command: command_rx,
                        remaining: budget,
                        compute: None,
                    };
                    let outcome = verifier
                        .verify_with_runner(max_cycles, cache_entry, &mut runner)
                        .await
                        .map_err(Reject::Verification)?;
                    let spent = budget.saturating_sub(runner.remaining);
                    crate::metrics::record_verify_cpu_budget_ratio(spent, budget);
                    let outcome = outcome.ok_or(Reject::ExcessiveVerifyTime)?;
                    Ok((outcome, timestamp, spent))
                })
                .await
            }
        }
    }
}
