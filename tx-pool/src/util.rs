//! Shared packed-value residency, fee arithmetic and blocking-runtime adapters.

use ckb_types::prelude::Entity;
use ckb_types::{
    bytes::Bytes,
    packed::{Byte32, OutPoint, ProposalShortId},
};
use tokio::{runtime::Handle, task::block_in_place};

/// Closed compile-time set of Molecule entities whose encoded length is
/// independent of hostile input.
pub(crate) trait FixedSizePackedEntity: Entity {}

impl FixedSizePackedEntity for Byte32 {}
impl FixedSizePackedEntity for OutPoint {}
impl FixedSizePackedEntity for ProposalShortId {}

/// Copy a packed entity into an allocation that contains only that entity.
///
/// Generated molecule accessors are cheap views into their parent's `Bytes`.
/// Storing such a view as a long-lived hash-map key can therefore retain an
/// entire transaction or block after the authority that paid for the parent
/// payload has gone away. Persistent indexes must compact packed keys at
/// their ownership boundary so their resident charge matches what they keep.
pub(crate) fn compact_packed<T: FixedSizePackedEntity>(value: &T) -> T {
    // `value` is already a verified `T`, and copying its complete byte slice
    // preserves that representation exactly. Molecule's constructor is named
    // `new_unchecked` because it also accepts arbitrary bytes; this wrapper's
    // typed input makes arbitrary bytes unrepresentable at every call site.
    T::new_unchecked(ckb_types::bytes::Bytes::copy_from_slice(value.as_slice()))
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

#[cfg(test)]
#[path = "tests/util.rs"]
mod util_tests;
