//! Types for the tx-pool resolve stage.
//!
//! The resolve stage turns a raw [`TransactionView`] into a [`ResolvedTx`] by
//! resolving inputs/cell_deps against the current chain snapshot and the
//! in-pool cell overlay.  It runs as a single ordered worker so that dependent
//! transactions are resolved in the order they arrive.

use crate::component::{entry::accepted_transaction_charge_bytes, pool_map::Status};
use crate::tx_source::TxSource;
use crate::util::compact_packed;
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, TransactionView, cell::ResolvedTransaction},
    packed::Byte32,
};
use std::sync::Arc;

/// A transaction that has been resolved and is ready for verification.
#[derive(Debug, Clone)]
pub struct ResolvedTx {
    /// The resolved transaction.
    pub rtx: Arc<ResolvedTransaction>,
    /// Pool status derived at resolve time.
    pub status: Status,
    /// Transaction fee calculated at resolve time.
    pub fee: Capacity,
    /// Serialized transaction size.
    pub tx_size: usize,
    /// Conservative resolved payload residency, computed once at resolve.
    pub resident_size: usize,
    /// Tip hash at resolve time; used to detect snapshot drift before submit.
    pub pre_resolve_tip: Byte32,
    /// The origin of the transaction (remote, local, or proposal notification).
    pub source: TxSource,
    /// Pipeline generation inherited from the resolve job.
    pub(crate) epoch: u64,
}

impl PartialEq for ResolvedTx {
    fn eq(&self, other: &Self) -> bool {
        self.rtx == other.rtx
            && self.status == other.status
            && self.fee == other.fee
            && self.tx_size == other.tx_size
            && self.resident_size == other.resident_size
            && self.pre_resolve_tip == other.pre_resolve_tip
            && self.source == other.source
            && self.epoch == other.epoch
    }
}

/// Snapshot-independent payload retained after script verification.
///
/// A chain snapshot is needed while resolving and verifying, but keeping it in
/// conflict-waiting or commit-ready entries pins old database snapshots across
/// arbitrarily many tip changes. This type makes that retention impossible at
/// the verified lifecycle boundary. Its resolved inputs remain complete for
/// DAO accounting, while cell deps retain only the outpoint and transaction
/// information needed by final liveness and cellbase-maturity revalidation.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PoolCandidate {
    pub(crate) tx: TransactionView,
    pub(crate) rtx: Arc<ResolvedTransaction>,
    pub(crate) status: Status,
    pub(crate) fee: Capacity,
    pub(crate) tx_size: usize,
    pub(crate) resident_size: usize,
    pub(crate) pre_resolve_tip: Byte32,
    pub(crate) source: TxSource,
    pub(crate) epoch: u64,
}

impl ResolvedTx {
    /// Return the sole transaction view owned by this resolved bundle.
    ///
    /// Keeping the raw transaction inside `ResolvedTransaction` avoids two
    /// independently constructible identities and makes a hash/witness-hash
    /// mismatch unrepresentable.
    pub(crate) fn transaction(&self) -> &TransactionView {
        &self.rtx.transaction
    }

    pub(crate) fn into_pool_candidate(self) -> PoolCandidate {
        // Script verification is the last consumer of dep outputs and dep
        // data. A tiny dep-group reference can otherwise pin the expanded
        // payload of up to thousands of cells for the entire pool lifetime.
        // Compact exactly at this typed lifecycle boundary: later paths only
        // run `ResolvedTransaction::check` (outpoints), time-relative checks
        // (dep transaction_info) and DAO calculation (resolved inputs).
        let rtx = compact_verified_resolved_transaction(self.rtx);
        // Reserve the complete accepted-state footprint before publishing the
        // candidate to the commit path. This includes the PoolMap indexes and
        // dependency graph that will be allocated during insertion, so the
        // coordinator cannot hand an already-undercharged object to the pool.
        let resident_size = accepted_transaction_charge_bytes(self.tx_size, &rtx);
        // Resolution starts from the compact raw owner, so both views can
        // share that exact tx-sized allocation instead of retaining two
        // independently copied transactions in the verified phase bundle.
        let tx = rtx.transaction.clone();
        PoolCandidate {
            tx,
            rtx,
            status: self.status,
            fee: self.fee,
            tx_size: self.tx_size,
            resident_size,
            pre_resolve_tip: self.pre_resolve_tip,
            source: self.source,
            epoch: self.epoch,
        }
    }
}

/// Materialize every resolved-cell field before the payload becomes a
/// coordinator-owned object.
///
/// Snapshot and pool providers commonly return packed accessors or `Bytes`
/// slices into an entire producer transaction/block. Charging only the
/// logical CellOutput/data length while retaining that shared backing lets a
/// tiny transaction pin much more memory than its residency budget. Resolve
/// is the single long-lived ownership boundary, so copy once here and keep
/// every later state transition move-only.
pub(crate) fn compact_resolved_transaction_for_residency(
    rtx: Arc<ResolvedTransaction>,
) -> Arc<ResolvedTransaction> {
    fn compact_cell(cell: &mut ckb_types::core::cell::CellMeta) {
        cell.cell_output = compact_packed(&cell.cell_output);
        cell.out_point = compact_packed(&cell.out_point);
        if let Some(info) = &mut cell.transaction_info {
            info.block_hash = compact_packed(&info.block_hash);
        }
        if let Some(data) = cell.mem_cell_data.take() {
            cell.mem_cell_data = Some(Bytes::copy_from_slice(&data));
        }
        if let Some(hash) = &mut cell.mem_cell_data_hash {
            *hash = compact_packed(hash);
        }
    }

    let mut rtx = Arc::try_unwrap(rtx).unwrap_or_else(|shared| (*shared).clone());
    for cell in rtx
        .resolved_inputs
        .iter_mut()
        .chain(rtx.resolved_cell_deps.iter_mut())
        .chain(rtx.resolved_dep_groups.iter_mut())
    {
        compact_cell(cell);
    }
    Arc::new(rtx)
}

/// Drop verification-only cell-dependency payload after script verification.
///
/// Resolved inputs intentionally remain untouched: DAO fee/template
/// calculation can require their output, data length, transaction information
/// and in-memory data when the input is produced by another in-pool
/// transaction. For cell deps, accepted-pool consumers use only liveness and
/// maturity, so retaining the expanded output/script/data is both unnecessary
/// and an attacker-controlled resident-memory multiplier.
pub(crate) fn compact_verified_resolved_transaction(
    rtx: Arc<ResolvedTransaction>,
) -> Arc<ResolvedTransaction> {
    let mut rtx = Arc::try_unwrap(rtx).unwrap_or_else(|shared| (*shared).clone());
    // Inputs were already materialized at the resolve/coordinator ownership
    // boundary. Keep them move-only here: DAO calculation still needs their
    // complete output/data payload.
    for cell in rtx
        .resolved_cell_deps
        .iter_mut()
        .chain(rtx.resolved_dep_groups.iter_mut())
    {
        let out_point = compact_packed(&cell.out_point);
        let transaction_info = cell.transaction_info.take().map(|mut info| {
            info.block_hash = compact_packed(&info.block_hash);
            info
        });
        *cell = ckb_types::core::cell::CellMeta {
            out_point,
            transaction_info,
            ..Default::default()
        };
    }
    Arc::new(rtx)
}

#[cfg(test)]
#[path = "tests/resolved_tx.rs"]
mod tests;
