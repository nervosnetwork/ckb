//! Retained-payload materialization and conservative resident-byte charges.

use crate::{component::entry::TxEntry, util::compact_packed};
use ckb_types::{
    bytes::Bytes,
    core::cell::{CellMeta, ResolvedTransaction},
    packed::CellOutput,
    prelude::Entity,
};
use std::sync::Arc;

/// Detach every retained cell view from its producer's backing allocation.
/// Provider calls this after charging and checking the materialized payload.
pub(super) fn detach_cell(cell: &mut CellMeta) {
    cell.cell_output =
        CellOutput::new_unchecked(Bytes::copy_from_slice(cell.cell_output.as_slice()));
    cell.out_point = compact_packed(&cell.out_point);
    if let Some(info) = &mut cell.transaction_info {
        info.block_hash = compact_packed(&info.block_hash);
    }
    if let Some(data) = &cell.mem_cell_data {
        cell.mem_cell_data = Some(Bytes::copy_from_slice(data));
    }
    if let Some(hash) = &cell.mem_cell_data_hash {
        cell.mem_cell_data_hash = Some(compact_packed(hash));
    }
}

/// Detach cell views supplied by the test/internal verified-entry fixture.
///
/// This fixture bypasses Provider's bounded materialization boundary and may
/// supply slices backed by complete producer transactions or blocks.
#[cfg(any(test, feature = "internal"))]
pub(super) fn compact_fixture_resolution(
    mut resolved: ResolvedTransaction,
) -> Arc<ResolvedTransaction> {
    for cell in resolved
        .resolved_inputs
        .iter_mut()
        .chain(resolved.resolved_cell_deps.iter_mut())
        .chain(resolved.resolved_dep_groups.iter_mut())
    {
        detach_cell(cell);
    }
    Arc::new(resolved)
}

/// Construct only the compact post-verification fields. The active result owns
/// its full cells until this admission finishes, without an intermediate full
/// clone of dependency scripts or data.
pub(super) fn accepted_resolution(resolved: &ResolvedTransaction) -> Arc<ResolvedTransaction> {
    fn compact(cells: &[CellMeta]) -> Vec<CellMeta> {
        cells
            .iter()
            .map(|cell| CellMeta {
                out_point: cell.out_point.clone(),
                transaction_info: cell.transaction_info.clone(),
                ..CellMeta::default()
            })
            .collect()
    }
    Arc::new(ResolvedTransaction {
        transaction: resolved.transaction.clone(),
        resolved_inputs: resolved.resolved_inputs.clone(),
        resolved_cell_deps: compact(&resolved.resolved_cell_deps),
        resolved_dep_groups: compact(&resolved.resolved_dep_groups),
    })
}

/// Conservative resident-byte charge for a resolved transaction.
///
/// This counts logical ownership, so shared `Bytes` are charged to each entry
/// that can independently extend their lifetime. Saturation turns impossible
/// arithmetic overflow into a value that every finite residency budget
/// rejects, rather than wrapping into an undercharge. Before verification this
/// includes complete dep expansion; accepted entries carry the compact
/// verified representation produced by the authority residency boundary.
pub(crate) fn resolved_transaction_charge_bytes(
    tx_size: usize,
    rtx: &ResolvedTransaction,
) -> usize {
    let mut bytes = std::mem::size_of::<TxEntry>()
        .saturating_add(std::mem::size_of::<ResolvedTransaction>())
        .saturating_add(tx_size)
        // `TransactionView` retains raw and witness hashes outside the packed
        // transaction backing bytes counted by `tx_size`.
        .saturating_add(64);

    for cells in [
        &rtx.resolved_inputs,
        &rtx.resolved_cell_deps,
        &rtx.resolved_dep_groups,
    ] {
        bytes = bytes.saturating_add(
            cells
                .capacity()
                .saturating_mul(std::mem::size_of::<ckb_types::core::cell::CellMeta>()),
        );
        for cell in cells {
            bytes = bytes
                .saturating_add(cell.cell_output.as_slice().len())
                .saturating_add(cell.out_point.as_slice().len())
                .saturating_add(cell.transaction_info.as_ref().map_or(0, |_| 32))
                .saturating_add(cell.mem_cell_data.as_ref().map_or(0, |data| data.len()))
                .saturating_add(cell.mem_cell_data_hash.as_ref().map_or(0, |_| 32));
        }
    }
    bytes
}

// Accepted owners retain compact resolved data and derived index memberships.
// These conservative weights cover owner records and per-input, expanded-dep
// and header footprints on 64-bit targets; they are not allocator-exact byte
// measurements. Keeping them explicit makes admission independent of the
// current collection representation and charges compact dep-group references
// for their expanded retained footprint.
const ACCEPTED_ENTRY_INDEX_BASE_CHARGE: usize = 1_024;
const ACCEPTED_INPUT_INDEX_CHARGE: usize = 256;
const ACCEPTED_DEP_INDEX_CHARGE: usize = 384;
const ACCEPTED_HEADER_INDEX_CHARGE: usize = 128;

/// Conservative resident-byte charge after a transaction becomes accepted.
///
/// Besides the compact verified resolved payload, the owner contributes to
/// proposal, expiry and dependency projections. Inputs and expanded deps carry
/// conservative per-footprint weights; header weights remain part of residency
/// accounting without requiring a separate header reverse index. Counts use
/// actual expanded resolved deps, so a tiny serialized dep-group reference
/// cannot hide its retained footprint.
pub(crate) fn accepted_transaction_charge_bytes(
    tx_size: usize,
    rtx: &ResolvedTransaction,
) -> usize {
    let input_count = rtx.transaction.inputs().len();
    let dep_count = rtx.related_dep_out_points().count();
    let header_count = rtx.transaction.header_deps().len();

    resolved_transaction_charge_bytes(tx_size, rtx)
        .saturating_add(ACCEPTED_ENTRY_INDEX_BASE_CHARGE)
        .saturating_add(input_count.saturating_mul(ACCEPTED_INPUT_INDEX_CHARGE))
        .saturating_add(dep_count.saturating_mul(ACCEPTED_DEP_INDEX_CHARGE))
        .saturating_add(header_count.saturating_mul(ACCEPTED_HEADER_INDEX_CHARGE))
}
