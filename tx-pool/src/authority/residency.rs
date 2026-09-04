//! Materialization boundaries for long-lived unified-authority payloads.

use ckb_types::bytes::Bytes;
use ckb_types::core::cell::{CellMeta, ResolvedTransaction};
use ckb_types::prelude::Entity;
use std::sync::Arc;

fn compact_entity<T: Entity>(value: &T) -> T {
    T::new_unchecked(Bytes::copy_from_slice(value.as_slice()))
}

/// Detach every resolved-cell view before the result becomes authority-owned.
///
/// Snapshot and overlay providers may return molecule views or byte slices
/// backed by an entire producer transaction or block. The resolve boundary is
/// the first long-lived owner, so its charged payload must not retain that
/// uncharged backing allocation.
pub(super) fn compact_after_resolution(
    mut resolved: ResolvedTransaction,
) -> Arc<ResolvedTransaction> {
    fn compact_cell(cell: &mut CellMeta) {
        cell.cell_output = compact_entity(&cell.cell_output);
        cell.out_point = compact_entity(&cell.out_point);
        if let Some(info) = &mut cell.transaction_info {
            info.block_hash = compact_entity(&info.block_hash);
        }
        if let Some(data) = &cell.mem_cell_data {
            cell.mem_cell_data = Some(Bytes::copy_from_slice(data));
        }
        if let Some(hash) = &mut cell.mem_cell_data_hash {
            *hash = compact_entity(hash);
        }
    }

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

/// Materialize a location-refresh candidate from one immutable
/// resolved payload. `CellMeta` clones share their already detached packed
/// bytes; only the three attacker-count-sized vector backings are allocated.
pub(super) fn clone_for_location_refresh(resolved: &ResolvedTransaction) -> ResolvedTransaction {
    fn clone_cells(cells: &[CellMeta]) -> Vec<CellMeta> {
        let mut cloned = Vec::with_capacity(cells.len());
        cloned.extend(cells.iter().cloned());
        cloned
    }

    ResolvedTransaction {
        transaction: resolved.transaction.clone(),
        resolved_cell_deps: clone_cells(&resolved.resolved_cell_deps),
        resolved_inputs: clone_cells(&resolved.resolved_inputs),
        resolved_dep_groups: clone_cells(&resolved.resolved_dep_groups),
    }
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
    let mut resolved = match Arc::try_unwrap(resolved) {
        Ok(resolved) => resolved,
        // A shared immutable representation is already bounded and charged.
        // Preserve it rather than allocating a hostile-sized clone merely to
        // reduce residency. The caller recomputes the charge from the exact
        // representation it retains.
        Err(shared) => return shared,
    };
    for cell in resolved
        .resolved_cell_deps
        .iter_mut()
        .chain(resolved.resolved_dep_groups.iter_mut())
    {
        // Resolution already detached these fixed fields. Move them into the
        // accepted representation and drop verification-only fields without
        // another allocation.
        let out_point = std::mem::take(&mut cell.out_point);
        let transaction_info = cell.transaction_info.take();
        *cell = CellMeta {
            out_point,
            transaction_info,
            ..Default::default()
        };
    }
    Arc::new(resolved)
}
