//! Pure parent-first ordering for transaction cohorts.
//!
//! Reorg recovery and persisted startup replay share this algorithm. It owns
//! no pool state and performs no I/O, so neither service path has to depend on
//! the other's mutable authority merely to establish deterministic ordering.

use ckb_types::{core::TransactionView, packed::OutPoint};
use std::collections::{HashMap, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DependencySortError {
    Allocation(&'static str),
    Arithmetic(&'static str),
    Projection(&'static str),
}

impl std::fmt::Display for DependencySortError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Allocation(context) => {
                write!(formatter, "dependency-sort allocation failed: {context}")
            }
            Self::Arithmetic(context) => {
                write!(formatter, "dependency-sort arithmetic overflow: {context}")
            }
            Self::Projection(context) => {
                write!(formatter, "dependency-sort projection drift: {context}")
            }
        }
    }
}

impl std::error::Error for DependencySortError {}

pub(crate) fn sort_transactions(
    transactions: &mut Vec<TransactionView>,
) -> Result<(), DependencySortError> {
    sort_by_dependencies(transactions, |transaction| transaction)
}

/// Topologically sort transaction-bearing items while preserving FIFO order
/// among simultaneously ready entries. A cyclic input keeps its original
/// order; callers still revalidate every transaction during replay.
pub(crate) fn sort_by_dependencies<T>(
    items: &mut Vec<T>,
    transaction: impl Fn(&T) -> &TransactionView,
) -> Result<(), DependencySortError> {
    if items.len() <= 1 {
        return Ok(());
    }

    let initial_outputs = items
        .len()
        .checked_mul(2)
        .ok_or(DependencySortError::Arithmetic("output index estimate"))?;
    let mut output_to_index: HashMap<OutPoint, usize> = HashMap::new();
    output_to_index
        .try_reserve(initial_outputs)
        .map_err(|_| DependencySortError::Allocation("output index"))?;
    for (index, item) in items.iter().enumerate() {
        let tx_hash = transaction(item).hash();
        for output in 0..transaction(item).outputs().len() {
            let output = u32::try_from(output)
                .map_err(|_| DependencySortError::Arithmetic("transaction output index"))?;
            output_to_index
                .try_reserve(1)
                .map_err(|_| DependencySortError::Allocation("output index growth"))?;
            output_to_index.insert(OutPoint::new(tx_hash.clone(), output), index);
        }
    }

    let mut in_degree = Vec::new();
    in_degree
        .try_reserve_exact(items.len())
        .map_err(|_| DependencySortError::Allocation("indegree"))?;
    in_degree.resize(items.len(), 0usize);
    let mut children: Vec<Vec<usize>> = Vec::new();
    children
        .try_reserve_exact(items.len())
        .map_err(|_| DependencySortError::Allocation("child lists"))?;
    children.resize_with(items.len(), Vec::new);
    for (index, item) in items.iter().enumerate() {
        let tx = transaction(item);
        for input in tx.input_pts_iter() {
            if let Some(&parent) = output_to_index.get(&input)
                && parent != index
            {
                register_edge(
                    parent,
                    index,
                    &mut in_degree,
                    &mut children,
                    DependencyRelation::Input,
                )?;
            }
        }
        for dependency in tx.cell_deps_iter() {
            if let Some(&parent) = output_to_index.get(&dependency.out_point())
                && parent != index
            {
                register_edge(
                    parent,
                    index,
                    &mut in_degree,
                    &mut children,
                    DependencyRelation::CellDep,
                )?;
            }
        }
    }

    let mut ready = VecDeque::new();
    ready
        .try_reserve(items.len())
        .map_err(|_| DependencySortError::Allocation("ready queue"))?;
    ready.extend(
        (0..items.len()).filter(|&index| in_degree.get(index).is_some_and(|degree| *degree == 0)),
    );
    let mut sorted = Vec::new();
    sorted
        .try_reserve_exact(items.len())
        .map_err(|_| DependencySortError::Allocation("sorted indexes"))?;
    while let Some(index) = ready.pop_front() {
        sorted.push(index);
        let planned_children = children
            .get(index)
            .ok_or(DependencySortError::Projection("ready child-list index"))?;
        for &child in planned_children {
            let degree = in_degree
                .get_mut(child)
                .ok_or(DependencySortError::Projection(
                    "ready child indegree index",
                ))?;
            *degree = degree
                .checked_sub(1)
                .ok_or(DependencySortError::Projection(
                    "dependency indegree underflow",
                ))?;
            if *degree == 0 {
                ready.push_back(child);
            }
        }
    }

    if sorted.len() != items.len() {
        return Ok(());
    }

    let mut remaining = Vec::new();
    remaining
        .try_reserve_exact(items.len())
        .map_err(|_| DependencySortError::Allocation("permutation storage"))?;
    let mut reordered = Vec::new();
    reordered
        .try_reserve_exact(items.len())
        .map_err(|_| DependencySortError::Allocation("reordered cohort"))?;
    remaining.extend(items.drain(..).map(Some));
    for index in sorted {
        let Some(item) = remaining.get_mut(index).and_then(Option::take) else {
            reordered.extend(
                remaining
                    .into_iter()
                    .enumerate()
                    .filter_map(|(index, item)| item.map(|item| (index, item))),
            );
            reordered.sort_unstable_by_key(|(index, _)| *index);
            items.extend(reordered.into_iter().map(|(_, item)| item));
            return Err(DependencySortError::Projection(
                "topological permutation index",
            ));
        };
        reordered.push((index, item));
    }
    items.extend(reordered.into_iter().map(|(_, item)| item));
    Ok(())
}

fn register_edge(
    parent: usize,
    child: usize,
    in_degree: &mut [usize],
    children: &mut [Vec<usize>],
    relation: DependencyRelation,
) -> Result<(), DependencySortError> {
    let degree = in_degree
        .get_mut(child)
        .ok_or(DependencySortError::Projection(
            relation.child_index_error(),
        ))?;
    *degree = degree
        .checked_add(1)
        .ok_or(DependencySortError::Arithmetic(relation.degree_error()))?;
    let planned_children = children
        .get_mut(parent)
        .ok_or(DependencySortError::Projection(
            relation.parent_index_error(),
        ))?;
    planned_children
        .try_reserve(1)
        .map_err(|_| DependencySortError::Allocation("child-list growth"))?;
    planned_children.push(child);
    Ok(())
}

#[derive(Clone, Copy)]
enum DependencyRelation {
    Input,
    CellDep,
}

impl DependencyRelation {
    const fn child_index_error(self) -> &'static str {
        match self {
            Self::Input => "child indegree index",
            Self::CellDep => "dep child indegree index",
        }
    }

    const fn degree_error(self) -> &'static str {
        match self {
            Self::Input => "child indegree",
            Self::CellDep => "dep child indegree",
        }
    }

    const fn parent_index_error(self) -> &'static str {
        match self {
            Self::Input => "parent child-list index",
            Self::CellDep => "dep parent child-list index",
        }
    }
}
