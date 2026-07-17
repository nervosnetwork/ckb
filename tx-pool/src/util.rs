use crate::error::Reject;
use crate::pool::TxPool;
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
    TimeRelativeTransactionVerifier, TxVerifyEnv,
    cache::{CacheEntry, Completed},
};
use std::sync::Arc;
use tokio::{runtime::Handle, sync::watch, task::block_in_place};

pub(crate) fn check_txid_collision(tx_pool: &TxPool, tx: &TransactionView) -> Result<(), Reject> {
    let short_id = tx.proposal_short_id();
    if tx_pool.contains_proposal_id(&short_id) {
        return Err(Reject::Duplicated(tx.hash()));
    }
    Ok(())
}

pub(crate) fn check_tx_fee(
    tx_pool: &TxPool,
    snapshot: &Snapshot,
    rtx: &ResolvedTransaction,
    tx_size: usize,
) -> Result<Capacity, Reject> {
    check_tx_fee_with_min_fee_rate(snapshot, rtx, tx_size, tx_pool.config.min_fee_rate)
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

pub(crate) async fn verify_rtx(
    snapshot: Arc<Snapshot>,
    rtx: Arc<ResolvedTransaction>,
    tx_env: Arc<TxVerifyEnv>,
    cache_entry: &Option<CacheEntry>,
    max_tx_verify_cycles: Cycle,
    command_rx: Option<&mut watch::Receiver<ChunkCommand>>,
) -> Result<Completed, Reject> {
    let consensus = snapshot.cloned_consensus();
    let data_loader = snapshot.as_data_loader();

    if let Some(completed) = cache_entry {
        TimeRelativeTransactionVerifier::new(Arc::clone(&rtx), consensus, data_loader, tx_env)
            .verify()
            .and_then(|_| verify_dao_script_size(&snapshot, Arc::clone(&rtx)))
            .map(|_| *completed)
            .map_err(Reject::Verification)
    } else if let Some(command_rx) = command_rx {
        // Transaction verification can be CPU-heavy (script VM). Run the verifier on
        // the blocking pool so it doesn't starve the async runtime.
        let mut command_rx = command_rx.clone();
        let rtx_for_verifier = Arc::clone(&rtx);
        block_in_place(|| {
            Handle::current().block_on(async move {
                ContextualTransactionVerifier::new(
                    rtx_for_verifier,
                    consensus,
                    data_loader,
                    Arc::clone(&tx_env),
                )
                .verify_with_pause(max_tx_verify_cycles, &mut command_rx)
                .await
            })
        })
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

pub(crate) fn time_relative_verify(
    snapshot: Arc<Snapshot>,
    rtx: Arc<ResolvedTransaction>,
    tx_env: TxVerifyEnv,
) -> Result<(), Reject> {
    let consensus = snapshot.cloned_consensus();
    TimeRelativeTransactionVerifier::new(
        rtx,
        consensus,
        snapshot.as_data_loader(),
        Arc::new(tx_env),
    )
    .verify()
    .map_err(Reject::Verification)
}

pub(crate) fn is_missing_input(reject: &Reject) -> bool {
    matches!(reject, Reject::Resolve(out_point_err) if out_point_err.is_unknown())
}

/// Convert a panic payload to a human-readable string for logging.
pub(crate) fn panic_payload_to_string(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}
