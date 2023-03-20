use crate::error::Reject;
use crate::pool::TxPool;
use ckb_chain_spec::consensus::Consensus;
use ckb_dao::DaoCalculator;
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_types::core::{
    cell::ResolvedTransaction, tx_pool::TRANSACTION_SIZE_LIMIT, Capacity, Cycle, TransactionView,
};
use ckb_verification::{
    cache::{CacheEntry, Completed},
    ContextualTransactionVerifier, NonContextualTransactionVerifier,
    TimeRelativeTransactionVerifier, TxVerifyEnv,
};
use tokio::task::block_in_place;

pub(crate) fn check_txid_collision(tx_pool: &TxPool, tx: &TransactionView) -> Result<(), Reject> {
    let short_id = tx.proposal_short_id();
    if tx_pool.contains_proposal_id(&short_id) {
        return Err(Reject::Duplicated(tx.hash()));
    }
    Ok(())
}

pub(crate) fn check_tx_size_limit(tx_pool: &TxPool, tx_size: usize) -> Result<(), Reject> {
    if tx_pool.reach_size_limit(tx_size) {
        return Err(Reject::Full(
            "size".to_owned(),
            tx_pool.config.max_mem_size as u64,
        ));
    }
    Ok(())
}

pub(crate) fn check_tx_cycle_limit(tx_pool: &TxPool, cycles: Cycle) -> Result<(), Reject> {
    if tx_pool.reach_cycles_limit(cycles) {
        return Err(Reject::Full("cycles".to_owned(), tx_pool.config.max_cycles));
    }
    Ok(())
}

pub(crate) fn check_tx_fee(
    tx_pool: &TxPool,
    snapshot: &Snapshot,
    rtx: &ResolvedTransaction,
    tx_size: usize,
) -> Result<Capacity, Reject> {
    let fee = DaoCalculator::new(snapshot.consensus(), &snapshot.as_data_provider())
        .transaction_fee(rtx)
        .map_err(|err| Reject::Malformed(format!("{err}")))?;
    // Theoretically we cannot use size as weight directly to calculate fee_rate,
    // here min fee rate is used as a cheap check,
    // so we will use size to calculate fee_rate directly
    let min_fee = tx_pool.config.min_fee_rate.fee(tx_size as u64);
    // reject txs which fee lower than min fee rate
    if fee < min_fee {
        let reject =
            Reject::LowFeeRate(tx_pool.config.min_fee_rate, min_fee.as_u64(), fee.as_u64());
        ckb_logger::debug!("reject tx {}", reject);
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
        return Err(Reject::Malformed("cellbase like".to_owned()));
    }

    Ok(())
}

pub(crate) fn verify_rtx(
    snapshot: &Snapshot,
    rtx: &ResolvedTransaction,
    tx_env: &TxVerifyEnv,
    cache_entry: &Option<CacheEntry>,
    max_tx_verify_cycles: Cycle,
) -> Result<Completed, Reject> {
    let consensus = snapshot.consensus();

    if let Some(ref cached) = cache_entry {
        match cached {
            CacheEntry::Completed(completed) => {
                TimeRelativeTransactionVerifier::new(rtx, consensus, snapshot, tx_env)
                    .verify()
                    .map(|_| *completed)
                    .map_err(Reject::Verification)
            }
            CacheEntry::Suspended(suspended) => ContextualTransactionVerifier::new(
                rtx,
                consensus,
                &snapshot.as_data_provider(),
                tx_env,
            )
            .complete(max_tx_verify_cycles, false, &suspended.snap)
            .map_err(Reject::Verification),
        }
    } else {
        block_in_place(|| {
            ContextualTransactionVerifier::new(rtx, consensus, &snapshot.as_data_provider(), tx_env)
                .verify(max_tx_verify_cycles, false)
                .map_err(Reject::Verification)
        })
    }
}

pub(crate) fn time_relative_verify(
    snapshot: &Snapshot,
    rtx: &ResolvedTransaction,
    tx_env: &TxVerifyEnv,
) -> Result<(), Reject> {
    let consensus = snapshot.consensus();
    TimeRelativeTransactionVerifier::new(rtx, consensus, snapshot, tx_env)
        .verify()
        .map_err(Reject::Verification)
}

pub(crate) fn is_missing_input(reject: &Reject) -> bool {
    matches!(reject, Reject::Resolve(out_point_err) if out_point_err.is_unknown())
}

/// Unwraps a result or propagates its error with snapshot.
#[macro_export]
macro_rules! try_or_return_with_snapshot {
    ($expr:expr, $snapshot:expr) => {
        match $expr {
            core::result::Result::Ok(val) => val,
            core::result::Result::Err(err) => {
                return Some((
                    core::result::Result::Err(core::convert::From::from(err)),
                    $snapshot,
                ));
            }
        }
    };
}
