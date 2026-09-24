//! Pure parent-first ordering for transaction cohorts.
//!
//! Reorg recovery and persisted startup replay share this algorithm. It owns
//! no pool state and performs no I/O, so neither service path has to depend on
//! the other's mutable authority merely to establish deterministic ordering.

use ckb_types::{core::TransactionView, packed::Byte32, prelude::*};
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap},
};

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
    transactions: &mut [TransactionView],
) -> Result<(), DependencySortError> {
    sort_by_dependencies(transactions, |transaction| transaction)
}

/// Topologically sort transaction-bearing items, preferring the earliest input
/// among ready entries. Already ordered input keeps its order, including any
/// read-before-spend precedence carried by a legacy snapshot. A cyclic input
/// keeps its original order; callers still revalidate every transaction during
/// replay. All fallible preparation finishes before the items are permuted.
pub(crate) fn sort_by_dependencies<T>(
    items: &mut [T],
    transaction: impl Fn(&T) -> &TransactionView,
) -> Result<(), DependencySortError> {
    if items.len() <= 1 {
        return Ok(());
    }

    let Some(mut sorted) = dependency_order(items, transaction)? else {
        return Ok(());
    };

    // sorted[destination] is its original source. Walk each permutation cycle,
    // placing that source and marking the destination complete as we go. The
    // final source is already in place after the preceding swaps. No item is
    // cloned, dropped or temporarily removed from the caller's slice.
    #[expect(
        clippy::indexing_slicing,
        reason = "A complete topological order is a permutation of the item indices, each emitted once."
    )]
    for start in 0..sorted.len() {
        let mut destination = start;
        loop {
            let source = std::mem::replace(&mut sorted[destination], destination);
            if source == start {
                break;
            }
            debug_assert_ne!(
                source, destination,
                "topological order revisited a completed index"
            );
            items.swap(destination, source);
            destination = source;
        }
    }
    Ok(())
}

/// Complete all fallible ordering work before the caller permutes its items.
fn dependency_order<T>(
    items: &[T],
    transaction: impl Fn(&T) -> &TransactionView,
) -> Result<Option<Vec<usize>>, DependencySortError> {
    // Canonical raw hashes identify the complete output vector. Keep the last
    // cohort member for duplicate hashes, including different witnesses.
    let mut producers: HashMap<Byte32, (usize, usize)> = HashMap::new();
    producers
        .try_reserve(items.len())
        .map_err(|_| DependencySortError::Allocation("producer index"))?;
    for (index, item) in items.iter().enumerate() {
        let tx = transaction(item);
        let outputs = tx.outputs().len();
        if outputs != 0 {
            producers.insert(tx.hash(), (index, outputs));
        }
    }
    let producer = |point: &ckb_types::packed::OutPoint| {
        let output: u32 = point.index().unpack();
        producers
            .get(&point.tx_hash())
            .filter(|(_, outputs)| (output as usize) < *outputs)
            .map(|(index, _)| *index)
    };

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
        let dependencies = tx
            .input_pts_iter()
            .map(|point| (point, DependencyRelation::Input))
            .chain(
                tx.cell_deps_iter()
                    .map(|dep| (dep.out_point(), DependencyRelation::CellDep)),
            );
        for (point, relation) in dependencies {
            if let Some(parent) = producer(&point)
                && parent != index
            {
                register_edge(&mut in_degree, &mut children, parent, index, relation)?;
            }
        }
    }

    // Producer lookup is no longer needed; release it before the sort buffers.
    drop(producers);
    let mut ready = BinaryHeap::new();
    ready
        .try_reserve(in_degree.len())
        .map_err(|_| DependencySortError::Allocation("ready queue"))?;
    ready.extend(
        (0..in_degree.len())
            .filter(|&index| in_degree.get(index).is_some_and(|degree| *degree == 0))
            .map(Reverse),
    );
    let mut sorted = Vec::new();
    sorted
        .try_reserve_exact(in_degree.len())
        .map_err(|_| DependencySortError::Allocation("sorted indexes"))?;
    while let Some(Reverse(index)) = ready.pop() {
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
                ready.push(Reverse(child));
            }
        }
    }

    Ok((sorted.len() == in_degree.len()).then_some(sorted))
}

fn register_edge(
    in_degree: &mut [usize],
    children: &mut [Vec<usize>],
    parent: usize,
    child: usize,
    relation: DependencyRelation,
) -> Result<(), DependencySortError> {
    let degree = in_degree
        .get_mut(child)
        .ok_or(DependencySortError::Projection(
            relation.child_index_error(),
        ))?;
    let next_degree = degree
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
    *degree = next_degree;
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

#[cfg(test)]
#[path = "tests/dependency_sort.rs"]
mod tests;
