//! One immutable causal graph compiled from the captured accepted owners.

use super::{Candidate, PackageAggregate, PackingError};
use crate::authority::model::Error;
use crate::error::Reject;
use std::{collections::HashMap, ops::Index};

/// Immutable adjacency with every endpoint in `0..len()`. Construction checks
/// external edges; traversal can use their indices in equally sized work arrays.
pub(super) struct Links {
    offsets: Vec<usize>,
    edges: Vec<usize>,
}

impl Links {
    pub(super) fn len(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "Endpoints are checked against len before counting; prefix sums and cursors describe exactly the allocated edge array."
    )]
    pub(super) fn from_edges(len: usize, edges: &[(usize, usize)]) -> Result<Self, PackingError> {
        let mut counts = vec![0usize; len];
        for &(from, to) in edges {
            if from >= len || to >= len {
                return Err(PackingError::Projection);
            }
            counts[from] = counts[from]
                .checked_add(1)
                .ok_or(PackingError::Arithmetic)?;
        }
        let mut offsets = Vec::with_capacity(len.saturating_add(1));
        offsets.push(0usize);
        for count in &counts {
            offsets.push(
                offsets
                    .last()
                    .copied()
                    .ok_or(PackingError::Projection)?
                    .checked_add(*count)
                    .ok_or(PackingError::Arithmetic)?,
            );
        }
        let mut values = vec![0; edges.len()];
        counts.copy_from_slice(&offsets[..len]);
        for &(from, to) in edges {
            values[counts[from]] = to;
            counts[from] = counts[from]
                .checked_add(1)
                .ok_or(PackingError::Arithmetic)?;
        }
        Ok(Self {
            offsets,
            edges: values,
        })
    }

    /// Reverse the active subgraph directly into compact adjacency. Incoming
    /// rows are already sorted because source indices are visited in order.
    #[expect(
        clippy::indexing_slicing,
        reason = "Links supplies checked indices and this method checks the mask length."
    )]
    pub(super) fn reversed(&self, active: &[bool]) -> Result<Self, PackingError> {
        let len = self.len();
        if active.len() != len {
            return Err(PackingError::Projection);
        }
        Ok(self.reverse_where(|index| active[index]))
    }

    #[expect(
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects,
        reason = "Links validates endpoints. Counts and cursors are bounded by the existing edge array; offsets have len + 1 elements."
    )]
    fn reverse_where(&self, active: impl Fn(usize) -> bool) -> Self {
        let len = self.len();
        let mut offsets = vec![0usize; len + 1];
        for (parent, children) in self.iter().enumerate() {
            if active(parent) {
                for &child in children {
                    if active(child) {
                        offsets[child + 1] += 1;
                    }
                }
            }
        }
        for index in 0..len {
            offsets[index + 1] += offsets[index];
        }
        let mut cursors = offsets[..len].to_vec();
        let mut edges = vec![0; offsets[len]];
        for (parent, children) in self.iter().enumerate() {
            if active(parent) {
                for &child in children {
                    if active(child) {
                        edges[cursors[child]] = parent;
                        cursors[child] += 1;
                    }
                }
            }
        }
        Self { offsets, edges }
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "The private offsets always delimit the edge array."
    )]
    pub(super) fn iter(&self) -> impl Iterator<Item = &[usize]> {
        self.offsets
            .windows(2)
            .map(|bounds| &self.edges[bounds[0]..bounds[1]])
    }

    #[cfg(test)]
    pub(super) fn from_lists(
        lists: impl IntoIterator<Item = Vec<usize>>,
    ) -> Result<Self, PackingError> {
        let lists: Vec<_> = lists.into_iter().collect();
        let edges: Vec<_> = lists
            .iter()
            .enumerate()
            .flat_map(|(from, next)| next.iter().map(move |to| (from, *to)))
            .collect();
        Self::from_edges(lists.len(), &edges)
    }
}

impl Index<usize> for Links {
    type Output = [usize];

    #[expect(
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects,
        reason = "Callers use indices from this graph. Private offsets have len + 1 elements and delimit valid edge slices."
    )]
    fn index(&self, index: usize) -> &Self::Output {
        &self.edges[self.offsets[index]..self.offsets[index + 1]]
    }
}

pub(super) struct Graph {
    pub(super) parents: Links,
    pub(super) children: Links,
    pub(super) topological: Vec<usize>,
    pub(super) ancestors: Vec<PackageAggregate>,
}

impl Graph {
    #[expect(
        clippy::indexing_slicing,
        reason = "Unique captured owners define every index. Parent lookup checks endpoints before constructing both edge directions."
    )]
    pub(super) fn new(candidates: &[Candidate<'_>], max_ancestors: usize) -> Result<Self, Error> {
        let len = candidates.len();
        let mut positions = HashMap::with_capacity(len);
        let mut edge_count = 0usize;
        for (index, candidate) in candidates.iter().enumerate() {
            if positions.insert(candidate.hash(), index).is_some() {
                return Err(PackingError::Projection.into());
            }
            edge_count = edge_count
                .checked_add(candidate.accepted.parents.len())
                .ok_or(PackingError::Arithmetic)?;
        }
        let mut offsets = Vec::with_capacity(len.saturating_add(1));
        offsets.push(0);
        let mut edges = Vec::with_capacity(edge_count);
        let mut indegree = Vec::with_capacity(len);
        for candidate in candidates {
            indegree.push(candidate.accepted.parents.len());
            for parent in &candidate.accepted.parents {
                edges.push(*positions.get(parent).ok_or(PackingError::Projection)?);
            }
            offsets.push(edges.len());
        }
        drop(positions);
        // Parent rows retain accepted.parents' hash order. Transposition visits
        // children by candidate index, matching the original child row order.
        let parents = Links { offsets, edges };
        let children = parents.reverse_where(|_| true);

        let mut ready: Vec<_> = indegree
            .iter()
            .enumerate()
            .filter_map(|(index, count)| (*count == 0).then_some(index))
            .collect();
        let mut topological = Vec::with_capacity(len);
        // Finish a ready branch before starting another. This is only a graph
        // evaluation order; package priority defines the emitted block order.
        while let Some(index) = ready.pop() {
            topological.push(index);
            for &child in &children[index] {
                indegree[child] = indegree[child]
                    .checked_sub(1)
                    .ok_or(PackingError::Projection)?;
                if indegree[child] == 0 {
                    ready.push(child);
                }
            }
        }
        if topological.len() != len {
            return Err(PackingError::CausalCycle.into());
        }
        // Release topology work buffers before allocating ancestor evaluation.
        drop(ready);
        drop(indegree);
        let ancestors = ancestor_totals(candidates, &parents, &topological, max_ancestors)?;
        Ok(Self {
            parents,
            children,
            topological,
            ancestors,
        })
    }
}

/// Evaluate totals after the whole causal graph has passed its cycle check.
/// One marked closure can survive unrelated single-parent evaluations, allowing
/// a later merge to reuse it without retaining every candidate's ancestor set.
#[expect(
    clippy::indexing_slicing,
    reason = "Graph construction validates every index and completes all parents before evaluating their child's totals."
)]
fn ancestor_totals(
    candidates: &[Candidate<'_>],
    parents: &Links,
    topological: &[usize],
    limit: usize,
) -> Result<Vec<PackageAggregate>, Error> {
    let len = candidates.len();
    let mut totals = vec![PackageAggregate::default(); len];
    let mut marks = vec![usize::MAX; len];
    let mut cached: Option<usize> = None;
    let mut stack = Vec::with_capacity(limit.min(len));
    // Every nonempty DAG starts with a root, so zero cannot admit its first
    // member. With a positive limit, singleton seeds fit the count bound.
    if limit == 0 && !topological.is_empty() {
        return Err(Reject::ExceededMaximumAncestorsCount.into());
    }
    // Each addition is one distinct member. Count precedes amount arithmetic.
    let include = |total: PackageAggregate, index: usize| -> Result<PackageAggregate, Error> {
        if total.entries >= limit {
            return Err(Reject::ExceededMaximumAncestorsCount.into());
        }
        total
            .checked_add(PackageAggregate::one(&candidates[index]))
            .ok_or_else(|| PackingError::Arithmetic.into())
    };
    for &index in topological {
        totals[index] = match parents[index] {
            [] => {
                marks[index] = index;
                cached = Some(index);
                PackageAggregate::one(&candidates[index])
            }
            [parent] => {
                let total = include(totals[parent], index)?;
                if cached == Some(parent) {
                    marks[index] = marks[parent];
                    cached = Some(index);
                }
                total
            }
            _ => {
                let (mut total, generation) =
                    match cached.filter(|tip| parents[index].contains(tip)) {
                        Some(tip) => (include(totals[tip], index)?, marks[tip]),
                        None => (PackageAggregate::one(&candidates[index]), index),
                    };
                marks[index] = generation;
                stack.extend(&parents[index]);
                while let Some(parent) = stack.pop() {
                    if marks[parent] == generation {
                        continue;
                    }
                    total = include(total, parent)?;
                    marks[parent] = generation;
                    stack.extend(&parents[parent]);
                }
                cached = Some(index);
                total
            }
        };
    }
    Ok(totals)
}
