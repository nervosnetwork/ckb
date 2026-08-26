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
use ckb_types::{
    bytes::Bytes,
    packed::{Byte32, OutPoint, ProposalShortId},
};
use ckb_verification::{
    ContextualTransactionVerifier, DaoScriptSizeVerifier, NonContextualTransactionVerifier,
    TimeRelativeTransactionVerifier, TxVerifyEnv,
    cache::{ScriptVerificationOutcome, ScriptVerificationProof},
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
/// Closed compile-time set of Molecule entities whose encoded length is
/// independent of hostile input. Variable-sized entities must use
/// `try_compact_packed` so allocation remains a typed outcome.
pub(crate) trait FixedSizePackedEntity: Entity {}

impl FixedSizePackedEntity for Byte32 {}
impl FixedSizePackedEntity for OutPoint {}
impl FixedSizePackedEntity for ProposalShortId {}

pub(crate) fn compact_packed<T: FixedSizePackedEntity>(value: &T) -> T {
    // `value` is already a verified `T`, and copying its complete byte slice
    // preserves that representation exactly. Molecule's constructor is named
    // `new_unchecked` because it also accepts arbitrary bytes; this wrapper's
    // typed input makes arbitrary bytes unrepresentable at every call site.
    T::new_unchecked(ckb_types::bytes::Bytes::copy_from_slice(value.as_slice()))
}

/// Fallibly copy a packed entity into an allocation containing only that
/// entity. Use this at hostile collection boundaries where infallible backing
/// compaction would bypass the subsystem's typed allocation algebra.
pub(crate) fn try_compact_packed<T: Entity>(
    value: &T,
) -> Result<T, std::collections::TryReserveError> {
    let mut owned = Vec::new();
    owned.try_reserve_exact(value.as_slice().len())?;
    owned.extend_from_slice(value.as_slice());
    Ok(T::new_unchecked(ckb_types::bytes::Bytes::from(owned)))
}

/// Fallibly detach a byte view from a potentially much larger hostile backing
/// allocation. The successful allocation is exact; failure creates no
/// long-lived owner and is handled by the caller's allocation terminal.
pub(crate) fn try_compact_bytes(value: &Bytes) -> Result<Bytes, std::collections::TryReserveError> {
    let mut owned = Vec::new();
    owned.try_reserve_exact(value.len())?;
    owned.extend_from_slice(value);
    Ok(Bytes::from(owned))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FixedPackedSequenceError {
    Arithmetic,
    Allocation,
}

/// Copy a finite fixed-size packed sequence into one shared exact backing
/// buffer. Copying every entity independently would turn one bounded query
/// into `O(n)` allocator calls; retaining caller entities could keep `n`
/// unrelated envelopes alive. This is the sole fallible sequence residency
/// mechanism for both full transaction hashes and proposal IDs.
fn try_compact_fixed_packed<T: FixedSizePackedEntity + Default>(
    values: impl ExactSizeIterator<Item = T>,
) -> Result<Vec<T>, FixedPackedSequenceError> {
    let count = values.len();
    let item_bytes = T::default().as_slice().len();
    let total_bytes = count
        .checked_mul(item_bytes)
        .ok_or(FixedPackedSequenceError::Arithmetic)?;

    let mut backing = Vec::new();
    backing
        .try_reserve_exact(total_bytes)
        .map_err(|_| FixedPackedSequenceError::Allocation)?;
    for value in values {
        if value.as_slice().len() != item_bytes {
            return Err(FixedPackedSequenceError::Arithmetic);
        }
        backing.extend_from_slice(value.as_slice());
    }
    if backing.len() != total_bytes {
        return Err(FixedPackedSequenceError::Arithmetic);
    }

    let backing = Bytes::from(backing);
    let mut compact = Vec::new();
    compact
        .try_reserve_exact(count)
        .map_err(|_| FixedPackedSequenceError::Allocation)?;
    let mut start = 0usize;
    for _ in 0..count {
        let end = start
            .checked_add(item_bytes)
            .ok_or(FixedPackedSequenceError::Arithmetic)?;
        if end > backing.len() {
            return Err(FixedPackedSequenceError::Arithmetic);
        }
        compact.push(T::new_unchecked(backing.slice(start..end)));
        start = end;
    }
    Ok(compact)
}

pub(crate) fn try_compact_proposal_ids(
    ids: impl ExactSizeIterator<Item = ProposalShortId>,
) -> Result<Vec<ProposalShortId>, FixedPackedSequenceError> {
    try_compact_fixed_packed(ids)
}

pub(crate) fn try_compact_transaction_hashes(
    hashes: impl ExactSizeIterator<Item = Byte32>,
) -> Result<Vec<Byte32>, FixedPackedSequenceError> {
    try_compact_fixed_packed(hashes)
}

/// Exact cross-product term for comparing two `u64` fee/weight ratios.
#[inline]
#[allow(
    clippy::arithmetic_side_effects,
    reason = "the product of two u64 values is representable in u128"
)]
pub(crate) fn fee_rate_cross_product(fee: u64, weight: u64) -> u128 {
    u128::from(fee) * u128::from(weight)
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

pub(crate) enum TxPoolVerificationOutcome {
    Verified(ScriptVerificationOutcome),
    DeadlineExceeded,
    InitialLoadExceeded,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TxPoolVerificationBudget {
    deadline: std::time::Instant,
    initial_load_limit: ckb_script::InitialProgramLoadLimit,
    vm_execution_mode: ckb_script::TxPoolVmExecutionMode,
}

impl TxPoolVerificationBudget {
    pub(crate) const fn new(
        deadline: std::time::Instant,
        initial_load_limit: ckb_script::InitialProgramLoadLimit,
    ) -> Self {
        Self {
            deadline,
            initial_load_limit,
            vm_execution_mode: ckb_script::TxPoolVmExecutionMode::Inline,
        }
    }

    pub(crate) const fn with_vm_execution_mode(
        mut self,
        vm_execution_mode: ckb_script::TxPoolVmExecutionMode,
    ) -> Self {
        self.vm_execution_mode = vm_execution_mode;
        self
    }

    pub(crate) const fn deadline(self) -> std::time::Instant {
        self.deadline
    }

    pub(crate) const fn initial_load_limit(self) -> ckb_script::InitialProgramLoadLimit {
        self.initial_load_limit
    }

    pub(crate) const fn vm_execution_mode(self) -> ckb_script::TxPoolVmExecutionMode {
        self.vm_execution_mode
    }
}

pub(crate) async fn verify_rtx(
    snapshot: Arc<Snapshot>,
    rtx: Arc<ResolvedTransaction>,
    tx_env: Arc<TxVerifyEnv>,
    cache_entry: Option<ScriptVerificationProof>,
    max_tx_verify_cycles: Cycle,
    command_rx: Option<&mut watch::Receiver<ChunkCommand>>,
    budget: TxPoolVerificationBudget,
) -> Result<TxPoolVerificationOutcome, Reject> {
    let consensus = snapshot.cloned_consensus();
    let data_loader = snapshot.as_data_loader();

    if let Some(command_rx) = command_rx {
        // The resumable verifier already executes each VM scheduler in its
        // own Tokio task. Wrapping this parent future in `block_in_place` and
        // synchronously blocking on the same runtime does not offload VM work;
        // it only forces a compensating runtime thread for every verification.
        let outcome = ContextualTransactionVerifier::new(
            Arc::clone(&rtx),
            consensus,
            data_loader,
            Arc::clone(&tx_env),
        )
        .verify_with_pause_and_deadline(
            max_tx_verify_cycles,
            cache_entry,
            command_rx,
            budget.deadline(),
            budget.initial_load_limit(),
            budget.vm_execution_mode(),
        )
        .await
        .map_err(Reject::Verification)?;
        match outcome {
            ckb_verification::DeadlineVerificationOutcome::DeadlineExceeded => {
                Ok(TxPoolVerificationOutcome::DeadlineExceeded)
            }
            ckb_verification::DeadlineVerificationOutcome::InitialLoadExceeded => {
                Ok(TxPoolVerificationOutcome::InitialLoadExceeded)
            }
            ckb_verification::DeadlineVerificationOutcome::Verified(outcome) => {
                verify_dao_script_size(&snapshot, rtx).map_err(Reject::Verification)?;
                if std::time::Instant::now() >= budget.deadline() {
                    Ok(TxPoolVerificationOutcome::DeadlineExceeded)
                } else {
                    Ok(TxPoolVerificationOutcome::Verified(outcome))
                }
            }
        }
    } else {
        block_in_place(|| {
            ContextualTransactionVerifier::new(Arc::clone(&rtx), consensus, data_loader, tx_env)
                .verify_scripts(max_tx_verify_cycles, cache_entry)
        })
        .and_then(|result| {
            verify_dao_script_size(&snapshot, rtx)?;
            Ok(TxPoolVerificationOutcome::Verified(result))
        })
        .map_err(Reject::Verification)
    }
}

#[cfg(test)]
#[path = "tests/block_offload.rs"]
mod block_offload_tests;
