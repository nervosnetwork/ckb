//! Materialization boundaries for long-lived unified-authority payloads.

use crate::util::{try_compact_bytes, try_compact_packed};
use ckb_types::core::cell::{CellMeta, ResolvedTransaction};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResolutionResidencyError {
    Allocation,
}

/// Detach every resolved-cell view before the result becomes authority-owned.
///
/// Snapshot and overlay providers may return molecule views or byte slices
/// backed by an entire producer transaction or block. The resolve boundary is
/// the first long-lived owner, so its charged payload must not retain that
/// uncharged backing allocation.
pub(super) fn compact_after_resolution(
    mut resolved: ResolvedTransaction,
) -> Result<Arc<ResolvedTransaction>, ResolutionResidencyError> {
    fn compact_cell(cell: &mut CellMeta) -> Result<(), ResolutionResidencyError> {
        cell.cell_output = try_compact_packed(&cell.cell_output)
            .map_err(|_| ResolutionResidencyError::Allocation)?;
        cell.out_point = try_compact_packed(&cell.out_point)
            .map_err(|_| ResolutionResidencyError::Allocation)?;
        if let Some(info) = &mut cell.transaction_info {
            info.block_hash = try_compact_packed(&info.block_hash)
                .map_err(|_| ResolutionResidencyError::Allocation)?;
        }
        if let Some(data) = &cell.mem_cell_data {
            cell.mem_cell_data =
                Some(try_compact_bytes(data).map_err(|_| ResolutionResidencyError::Allocation)?);
        }
        if let Some(hash) = &mut cell.mem_cell_data_hash {
            *hash = try_compact_packed(hash).map_err(|_| ResolutionResidencyError::Allocation)?;
        }
        Ok(())
    }

    for cell in resolved
        .resolved_inputs
        .iter_mut()
        .chain(resolved.resolved_cell_deps.iter_mut())
        .chain(resolved.resolved_dep_groups.iter_mut())
    {
        compact_cell(cell)?;
    }
    Ok(Arc::new(resolved))
}

/// Fallibly materialize a location-refresh candidate from one immutable
/// resolved payload. `CellMeta` clones share their already detached packed
/// bytes; only the three attacker-count-sized vector backings are allocated.
/// Final validation can therefore classify pressure before publishing a new
/// payload instead of invoking `ResolvedTransaction::clone` infallibly.
pub(super) fn try_clone_for_location_refresh(
    resolved: &ResolvedTransaction,
) -> Result<ResolvedTransaction, ResolutionResidencyError> {
    fn try_clone_cells(cells: &[CellMeta]) -> Result<Vec<CellMeta>, ResolutionResidencyError> {
        let mut cloned = Vec::new();
        cloned
            .try_reserve_exact(cells.len())
            .map_err(|_| ResolutionResidencyError::Allocation)?;
        cloned.extend(cells.iter().cloned());
        Ok(cloned)
    }

    Ok(ResolvedTransaction {
        transaction: resolved.transaction.clone(),
        resolved_cell_deps: try_clone_cells(&resolved.resolved_cell_deps)?,
        resolved_inputs: try_clone_cells(&resolved.resolved_inputs)?,
        resolved_dep_groups: try_clone_cells(&resolved.resolved_dep_groups)?,
    })
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
