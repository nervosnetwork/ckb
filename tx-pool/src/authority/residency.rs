//! Materialization boundaries for long-lived unified-authority payloads.

use crate::util::compact_packed;
use ckb_types::{
    bytes::Bytes,
    core::cell::{CellMeta, ResolvedTransaction},
};
use std::sync::Arc;

/// Detach every resolved-cell view before the result becomes authority-owned.
///
/// Snapshot and overlay providers may return molecule views or byte slices
/// backed by an entire producer transaction or block. The resolve boundary is
/// the first long-lived owner, so its charged payload must not retain that
/// uncharged backing allocation.
pub(super) fn compact_after_resolution(
    resolved: Arc<ResolvedTransaction>,
) -> Arc<ResolvedTransaction> {
    fn compact_cell(cell: &mut CellMeta) {
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

    let mut resolved = Arc::try_unwrap(resolved).unwrap_or_else(|shared| (*shared).clone());
    for cell in resolved
        .resolved_inputs
        .iter_mut()
        .chain(resolved.resolved_cell_deps.iter_mut())
        .chain(resolved.resolved_dep_groups.iter_mut())
    {
        compact_cell(cell);
    }
    Arc::new(resolved)
}

/// Drop cell-dependency payload which cannot be reused after script verification.
///
/// Resolved inputs remain complete for DAO accounting. Accepted membership
/// needs only each dependency's outpoint and chain transaction information for
/// liveness and maturity, so retaining expanded scripts or data would create an
/// attacker-controlled residency multiplier with no semantic owner.
pub(super) fn compact_after_verification(
    resolved: Arc<ResolvedTransaction>,
) -> Arc<ResolvedTransaction> {
    let mut resolved = Arc::try_unwrap(resolved).unwrap_or_else(|shared| (*shared).clone());
    for cell in resolved
        .resolved_cell_deps
        .iter_mut()
        .chain(resolved.resolved_dep_groups.iter_mut())
    {
        let out_point = compact_packed(&cell.out_point);
        let transaction_info = cell.transaction_info.take().map(|mut info| {
            info.block_hash = compact_packed(&info.block_hash);
            info
        });
        *cell = CellMeta {
            out_point,
            transaction_info,
            ..Default::default()
        };
    }
    Arc::new(resolved)
}
