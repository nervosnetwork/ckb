use crate::error::Reject;
use crate::pool::TxPool;
use ckb_chain_spec::consensus::Consensus;
use ckb_dao::DaoCalculator;
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_types::core::{cell::ResolvedTransaction, Capacity, Cycle, TransactionView};
use ckb_verification::{
    cache::CacheEntry, ContextualTransactionVerifier, NonContextualTransactionVerifier,
    TimeRelativeTransactionVerifier, TransactionVerificationPhase,
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
        .transaction_fee(&rtx)
        .map_err(|err| Reject::Malformed(format!("Transcation fee calculate overflow: {}", err)))?;
    let min_fee = tx_pool.config.min_fee_rate.fee(tx_size);
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
    // cellbase is only valid in a block, not as a loose transaction
    if tx.is_cellbase() {
        return Err(Reject::Malformed("cellbase like".to_owned()));
    }

    Ok(())
}

pub(crate) fn verify_rtx(
    snapshot: &Snapshot,
    rtx: &ResolvedTransaction,
    tx_phase: TransactionVerificationPhase,
    cache_entry: Option<CacheEntry>,
    max_tx_verify_cycles: Cycle,
) -> Result<CacheEntry, Reject> {
    let tip_header = snapshot.tip_header();
    let tip_number = tip_header.number();
    let epoch = tip_header.epoch();
    let consensus = snapshot.consensus();

    if let Some(cached) = cache_entry {
        TimeRelativeTransactionVerifier::new(
            &rtx,
            snapshot,
            tip_number,
            epoch,
            tip_header.hash(),
            tx_phase,
            consensus,
        )
        .verify()
        .map(|_| cached)
        .map_err(Reject::Verification)
    } else {
        block_in_place(|| {
            ContextualTransactionVerifier::new(
                &rtx,
                tip_number,
                epoch,
                tip_header.hash(),
                tx_phase,
                consensus,
                &snapshot.as_data_provider(),
            )
            .verify(max_tx_verify_cycles, false)
            .map_err(Reject::Verification)
        })
    }
}

pub(crate) fn is_missing_input(reject: &Reject) -> bool {
    matches!(reject, Reject::Resolve(out_point_err) if out_point_err.is_unknown())
}
