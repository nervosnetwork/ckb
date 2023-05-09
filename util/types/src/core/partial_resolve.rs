//! TODO(doc): @zhangsoledad
//!
use crate::{
    core::cell::{
        parse_dep_group_data, CellMeta, CellProvider, CellStatus, HeaderChecker, ResolvedDep,
        ResolvedTransaction, MAX_DEP_EXPANSION_LIMIT, SYSTEM_CELL,
    },
    core::error::OutPointError,
    core::{DepType, TransactionView},
    packed::{CellDep, OutPoint},
};
use std::collections::{hash_map::Entry, HashMap, HashSet};
use std::hash::{BuildHasher, Hash, Hasher};

/// TODO(doc): @zhangsoledad
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum PartialCellMeta {
    /// TODO(doc): @zhangsoledad
    Resolved(CellMeta),
    /// TODO(doc): @zhangsoledad
    Unknown(OutPoint),
}

impl Hash for PartialResolvedTransaction {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Hash::hash(&self.transaction, state);
    }
}

impl PartialEq for PartialResolvedTransaction {
    fn eq(&self, other: &PartialResolvedTransaction) -> bool {
        self.transaction == other.transaction
    }
}

/// Transaction with resolved input cells.
#[derive(Debug, Clone, Eq)]
pub struct PartialResolvedTransaction {
    /// TODO(doc): @quake
    pub transaction: TransactionView,
    /// TODO(doc): @quake
    pub resolved_cell_deps: Vec<PartialCellMeta>,
    /// TODO(doc): @quake
    pub resolved_inputs: Vec<PartialCellMeta>,
    /// TODO(doc): @quake
    pub resolved_dep_groups: Vec<PartialCellMeta>,
}

/// Resolve all cell meta from db base on the transaction.
pub fn partial_resolve_transaction<CP: CellProvider, S: BuildHasher>(
    transaction: TransactionView,
    seen_inputs: &mut HashSet<OutPoint, S>,
    cell_provider: &CP,
) -> Result<PartialResolvedTransaction, OutPointError> {
    let (mut resolved_inputs, mut resolved_cell_deps, mut resolved_dep_groups) = (
        Vec::with_capacity(transaction.inputs().len()),
        Vec::with_capacity(transaction.cell_deps().len()),
        Vec::new(),
    );
    let mut current_inputs = HashSet::new();

    let mut resolved_cells: HashMap<(OutPoint, bool), PartialCellMeta> = HashMap::new();
    let mut resolve_cell =
        |out_point: &OutPoint, eager_load: bool| -> Result<PartialCellMeta, OutPointError> {
            if seen_inputs.contains(out_point) {
                return Err(OutPointError::Dead(out_point.clone()));
            }

            match resolved_cells.entry((out_point.clone(), eager_load)) {
                Entry::Occupied(entry) => Ok(entry.get().clone()),
                Entry::Vacant(entry) => {
                    let cell_status = cell_provider.cell(out_point, eager_load);
                    match cell_status {
                        CellStatus::Dead => Err(OutPointError::Dead(out_point.clone())),
                        CellStatus::Unknown => Ok(PartialCellMeta::Unknown(out_point.clone())),
                        CellStatus::Live(cell_meta) => {
                            let cell = PartialCellMeta::Resolved(cell_meta);
                            entry.insert(cell.clone());
                            Ok(cell)
                        }
                    }
                }
            }
        };

    // skip resolve input of cellbase
    if !transaction.is_cellbase() {
        for out_point in transaction.input_pts_iter() {
            if !current_inputs.insert(out_point.to_owned()) {
                return Err(OutPointError::Dead(out_point));
            }
            resolved_inputs.push(resolve_cell(&out_point, false)?);
        }
    }

    resolve_transaction_deps_with_system_cell_cache(
        &transaction,
        &mut resolve_cell,
        &mut resolved_cell_deps,
        &mut resolved_dep_groups,
    )?;

    seen_inputs.extend(current_inputs);

    Ok(PartialResolvedTransaction {
        transaction,
        resolved_inputs,
        resolved_cell_deps,
        resolved_dep_groups,
    })
}

pub(crate) fn resolve_transaction_deps_with_system_cell_cache<
    F: FnMut(&OutPoint, bool) -> Result<PartialCellMeta, OutPointError>,
>(
    transaction: &TransactionView,
    cell_resolver: &mut F,
    resolved_cell_deps: &mut Vec<PartialCellMeta>,
    resolved_dep_groups: &mut Vec<PartialCellMeta>,
) -> Result<(), OutPointError> {
    // - If the dep expansion count of the transaction is not over the `MAX_DEP_EXPANSION_LIMIT`,
    //   it will always be accepted.
    // - If the dep expansion count of the transaction is over the `MAX_DEP_EXPANSION_LIMIT`, the
    //   behavior is as follow:
    //   | ckb v2021 | yes |             reject the transaction              |
    let mut remaining_dep_slots = MAX_DEP_EXPANSION_LIMIT;
    if let Some(system_cell) = SYSTEM_CELL.get() {
        for cell_dep in transaction.cell_deps_iter() {
            if let Some(resolved_dep) = system_cell.get(&cell_dep) {
                match resolved_dep {
                    ResolvedDep::Cell(cell_meta) => {
                        resolved_cell_deps.push(PartialCellMeta::Resolved(cell_meta.clone()));
                        remaining_dep_slots = remaining_dep_slots
                            .checked_sub(1)
                            .ok_or(OutPointError::OverMaxDepExpansionLimit)?;
                    }
                    ResolvedDep::Group(dep_group, cell_deps) => {
                        resolved_dep_groups.push(PartialCellMeta::Resolved(dep_group.clone()));
                        resolved_cell_deps
                            .extend(cell_deps.iter().cloned().map(PartialCellMeta::Resolved));
                        remaining_dep_slots = remaining_dep_slots
                            .checked_sub(cell_deps.len())
                            .ok_or(OutPointError::OverMaxDepExpansionLimit)?;
                    }
                }
            } else {
                resolve_transaction_dep(
                    &cell_dep,
                    cell_resolver,
                    resolved_cell_deps,
                    resolved_dep_groups,
                    false, // don't eager_load data
                    &mut remaining_dep_slots,
                )?;
            }
        }
    } else {
        for cell_dep in transaction.cell_deps_iter() {
            resolve_transaction_dep(
                &cell_dep,
                cell_resolver,
                resolved_cell_deps,
                resolved_dep_groups,
                false, // don't eager_load data
                &mut remaining_dep_slots,
            )?;
        }
    }
    Ok(())
}

fn resolve_transaction_dep<F: FnMut(&OutPoint, bool) -> Result<PartialCellMeta, OutPointError>>(
    cell_dep: &CellDep,
    cell_resolver: &mut F,
    resolved_cell_deps: &mut Vec<PartialCellMeta>,
    resolved_dep_groups: &mut Vec<PartialCellMeta>,
    eager_load: bool,
    remaining_dep_slots: &mut usize,
) -> Result<(), OutPointError> {
    if cell_dep.dep_type() == DepType::DepGroup.into() {
        let outpoint = cell_dep.out_point();
        let dep_group = cell_resolver(&outpoint, true)?;

        if let PartialCellMeta::Resolved(ref group_cell) = dep_group {
            let data = group_cell
                .mem_cell_data
                .as_ref()
                .expect("Load cell meta must with data");
            let sub_out_points =
                parse_dep_group_data(data).map_err(|_| OutPointError::InvalidDepGroup(outpoint))?;

            *remaining_dep_slots = remaining_dep_slots
                .checked_sub(sub_out_points.len())
                .ok_or(OutPointError::OverMaxDepExpansionLimit)?;

            for sub_out_point in sub_out_points.into_iter() {
                resolved_cell_deps.push(cell_resolver(&sub_out_point, eager_load)?);
            }
        }
        resolved_dep_groups.push(dep_group);
    } else {
        *remaining_dep_slots = remaining_dep_slots
            .checked_sub(1)
            .ok_or(OutPointError::OverMaxDepExpansionLimit)?;

        resolved_cell_deps.push(cell_resolver(&cell_dep.out_point(), eager_load)?);
    }
    Ok(())
}

/// Resolve all cell meta from db base on the transaction.
pub fn complete_resolve_transaction<CP: CellProvider, HC: HeaderChecker>(
    mut rtx: PartialResolvedTransaction,
    cell_provider: &CP,
    header_checker: &HC,
) -> Result<ResolvedTransaction, OutPointError> {
    let (mut resolved_inputs, mut resolved_cell_deps, mut resolved_dep_groups) = (
        Vec::with_capacity(rtx.transaction.inputs().len()),
        Vec::with_capacity(rtx.transaction.cell_deps().len()),
        Vec::new(),
    );

    let mut resolved_cells: HashMap<(OutPoint, bool), CellMeta> = HashMap::new();
    let mut resolve_cell =
        |out_point: &OutPoint, eager_load: bool| -> Result<CellMeta, OutPointError> {
            match resolved_cells.entry((out_point.clone(), eager_load)) {
                Entry::Occupied(entry) => Ok(entry.get().clone()),
                Entry::Vacant(entry) => {
                    let cell_status = cell_provider.cell(out_point, eager_load);
                    match cell_status {
                        CellStatus::Dead => Err(OutPointError::Dead(out_point.clone())),
                        CellStatus::Unknown => Err(OutPointError::Unknown(out_point.clone())),
                        CellStatus::Live(cell_meta) => {
                            entry.insert(cell_meta.clone());
                            Ok(cell_meta)
                        }
                    }
                }
            }
        };

    if !rtx.transaction.is_cellbase() {
        for partial in rtx.resolved_inputs.drain(..) {
            resolved_inputs.push(match partial {
                PartialCellMeta::Unknown(out_point) => resolve_cell(&out_point, false)?,
                PartialCellMeta::Resolved(cell_meta) => cell_meta,
            });
        }
    }

    for partial in rtx.resolved_dep_groups.drain(..) {
        resolved_dep_groups.push(match partial {
            PartialCellMeta::Unknown(out_point) => {
                let group_cell = resolve_cell(&out_point, true)?;

                let data = group_cell
                    .mem_cell_data
                    .as_ref()
                    .expect("Load cell meta must with data");
                let sub_out_points = parse_dep_group_data(data)
                    .map_err(|_| OutPointError::InvalidDepGroup(out_point))?;

                for sub_out_point in sub_out_points.into_iter() {
                    resolved_cell_deps.push(resolve_cell(&sub_out_point, false)?);
                }
                group_cell
            }
            PartialCellMeta::Resolved(cell_meta) => cell_meta,
        });
    }

    for partial in rtx.resolved_cell_deps.drain(..) {
        resolved_cell_deps.push(match partial {
            PartialCellMeta::Unknown(out_point) => resolve_cell(&out_point, false)?,
            PartialCellMeta::Resolved(cell_meta) => cell_meta,
        });
    }

    for block_hash in rtx.transaction.header_deps_iter() {
        header_checker.check_valid(&block_hash)?;
    }

    Ok(ResolvedTransaction {
        transaction: rtx.transaction,
        resolved_inputs,
        resolved_cell_deps,
        resolved_dep_groups,
    })
}
