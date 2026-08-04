use crate::error::Reject;
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
use ckb_types::prelude::Entity;
use ckb_verification::{
    ContextualTransactionVerifier, DaoScriptSizeVerifier, NonContextualTransactionVerifier,
    TimeRelativeTransactionVerifier, TxVerifyEnv, cache::Completed,
};
use std::sync::Arc;
use tokio::{runtime::Handle, sync::watch, task::block_in_place};

/// Copy a packed entity into an allocation that contains only that entity.
///
/// Generated molecule accessors are cheap views into their parent's `Bytes`.
/// Storing such a view as a long-lived hash-map key can therefore retain an
/// entire transaction or block after the authority that paid for the parent
/// payload has gone away. Persistent indexes must compact packed keys at
/// their ownership boundary so their resident charge matches what they keep.
pub(crate) fn compact_packed<T: Entity>(value: &T) -> T {
    // `value` is already a verified `T`, and copying its complete byte slice
    // preserves that representation exactly. Molecule's constructor is named
    // `new_unchecked` because it also accepts arbitrary bytes; this wrapper's
    // typed input makes arbitrary bytes unrepresentable at every call site.
    T::new_unchecked(ckb_types::bytes::Bytes::copy_from_slice(value.as_slice()))
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
    // Theoretically we cannot use size as weight directly to calculate fee_rate,
    // here min fee rate is used as a cheap check,
    // so we will use size to calculate fee_rate directly
    let min_fee = min_fee_rate.fee(tx_size as u64);
    // reject txs which fee lower than min fee rate
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

    // The ckb consensus does not limit the size of a single transaction,
    // but if the size of the transaction is close to the limit of the block,
    // it may cause the transaction to fail to be packed
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

/// Run a blocking operation off the async executor when running on a
/// multi-threaded tokio runtime (all production paths), or inline otherwise
/// (e.g. current-thread test runtimes, plain sync tests).
///
/// Used for operations that can hit disk, such as RocksDB access or the
/// snapshot data loader, which must not run directly on the async executor.
pub(crate) fn block_offload<T>(f: impl FnOnce() -> T) -> T {
    match Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            block_in_place(f)
        }
        _ => f(),
    }
}

fn verify_dao_script_size(
    snapshot: &Snapshot,
    rtx: Arc<ResolvedTransaction>,
) -> Result<(), ckb_error::Error> {
    // DAO verification loads cell data through the data loader, which may hit
    // RocksDB. Keep it off the async executor, like the script verifier.
    block_offload(|| {
        DaoScriptSizeVerifier::new(
            Arc::clone(&rtx),
            snapshot.cloned_consensus(),
            snapshot.borrow_as_data_loader(),
        )
        .verify()
    })
}

/// Revalidate every chain-context rule that is not covered by a reusable
/// script receipt. This is the only valid bridge from a cached script result
/// to a different tx-pool chain view: maturity, `since`, and the DAO location
/// rule all consume the same refreshed resolved transaction and environment.
pub(crate) fn revalidate_tx_context(
    snapshot: Arc<Snapshot>,
    rtx: Arc<ResolvedTransaction>,
    tx_env: Arc<TxVerifyEnv>,
) -> Result<(), Reject> {
    let consensus = snapshot.cloned_consensus();
    block_offload(|| {
        TimeRelativeTransactionVerifier::new(
            Arc::clone(&rtx),
            Arc::clone(&consensus),
            snapshot.as_data_loader(),
            tx_env,
        )
        .verify()
        .and_then(|_| {
            DaoScriptSizeVerifier::new(rtx, consensus, snapshot.borrow_as_data_loader()).verify()
        })
    })
    .map_err(Reject::Verification)
}

pub(crate) async fn verify_rtx(
    snapshot: Arc<Snapshot>,
    rtx: Arc<ResolvedTransaction>,
    tx_env: Arc<TxVerifyEnv>,
    cache_entry: &Option<Completed>,
    max_tx_verify_cycles: Cycle,
    command_rx: Option<&mut watch::Receiver<ChunkCommand>>,
) -> Result<Completed, Reject> {
    let consensus = snapshot.cloned_consensus();
    let data_loader = snapshot.as_data_loader();

    if let Some(completed) = cache_entry {
        revalidate_tx_context(snapshot, rtx, tx_env).map(|_| *completed)
    } else if let Some(command_rx) = command_rx {
        // The resumable verifier already executes each VM scheduler in its
        // own Tokio task. Wrapping this parent future in `block_in_place` and
        // synchronously blocking on the same runtime does not offload VM work;
        // it only forces a compensating runtime thread for every verification.
        ContextualTransactionVerifier::new(
            Arc::clone(&rtx),
            consensus,
            data_loader,
            Arc::clone(&tx_env),
        )
        .verify_with_pause(max_tx_verify_cycles, command_rx)
        .await
        .and_then(|result| {
            verify_dao_script_size(&snapshot, rtx)?;
            Ok(result)
        })
        .map_err(Reject::Verification)
    } else {
        block_in_place(|| {
            ContextualTransactionVerifier::new(Arc::clone(&rtx), consensus, data_loader, tx_env)
                .verify(max_tx_verify_cycles, false)
        })
        .and_then(|result| {
            verify_dao_script_size(&snapshot, rtx)?;
            Ok(result)
        })
        .map_err(Reject::Verification)
    }
}

#[cfg(test)]
#[path = "tests/block_offload.rs"]
mod block_offload_tests;
