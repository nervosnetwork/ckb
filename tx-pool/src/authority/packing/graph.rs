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
        clippy::arithmetic_side_effects,
        reason = "Links validates endpoints and this method checks the mask length. Every count, prefix sum and cursor is bounded by the existing edge array's length; offsets already have len + 1 elements."
    )]
    pub(super) fn reversed(&self, active: &[bool]) -> Result<Self, PackingError> {
        let len = self.len();
        if active.len() != len {
            return Err(PackingError::Projection);
        }
        let mut offsets = vec![0usize; len + 1];
        for (parent, children) in self.iter().enumerate() {
            if active[parent] {
                for &child in children {
                    if active[child] {
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
            if active[parent] {
                for &child in children {
                    if active[child] {
                        edges[cursors[child]] = parent;
                        cursors[child] += 1;
                    }
                }
            }
        }
        Ok(Self { offsets, edges })
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
        reason = "Unique captured owners define every index. Edges are checked at construction, and only topologically completed parents supply aggregates."
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
        let mut edges = Vec::with_capacity(edge_count);
        let mut indegree = Vec::with_capacity(len);
        for (child, candidate) in candidates.iter().enumerate() {
            indegree.push(candidate.accepted.parents.len());
            for parent in &candidate.accepted.parents {
                edges.push((
                    *positions.get(parent).ok_or(PackingError::Projection)?,
                    child,
                ));
            }
        }
        let children = Links::from_edges(len, &edges)?;
        for (parent, child) in &mut edges {
            std::mem::swap(parent, child);
        }
        let parents = Links::from_edges(len, &edges)?;
        drop(edges);
        drop(positions);

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
        drop(indegree);
        drop(ready);

        let mut ancestors = vec![PackageAggregate::default(); len];
        let mut marks = vec![usize::MAX; len];
        let mut stack = Vec::with_capacity(max_ancestors.min(len));
        // Marks describe exactly one completed ancestor closure. A child can
        // extend it without revisiting shared ancestors; other merges start a
        // fresh generation. No per-entry ancestor sets survive this traversal.
        let mut marked = None;
        for &index in &topological {
            let own = PackageAggregate::one(&candidates[index]);
            let incoming = &parents[index];
            let aggregate = match incoming {
                [] => {
                    marks[index] = index;
                    marked = Some(index);
                    own
                }
                [parent] => {
                    if ancestors[*parent].entries >= max_ancestors {
                        return Err(Reject::ExceededMaximumAncestorsCount.into());
                    }
                    if marked == Some(*parent) {
                        marks[index] = marks[*parent];
                        marked = Some(index);
                    }
                    ancestors[*parent]
                        .checked_add(own)
                        .ok_or(PackingError::Arithmetic)?
                }
                _ => {
                    // A merge needs a set union; summing parent aggregates
                    // would count their shared ancestors more than once.
                    let (mut aggregate, generation) =
                        match marked.filter(|parent| incoming.contains(parent)) {
                            Some(parent) => {
                                if ancestors[parent].entries >= max_ancestors {
                                    return Err(Reject::ExceededMaximumAncestorsCount.into());
                                }
                                (
                                    ancestors[parent]
                                        .checked_add(own)
                                        .ok_or(PackingError::Arithmetic)?,
                                    marks[parent],
                                )
                            }
                            None => (own, index),
                        };
                    marks[index] = generation;
                    stack.extend(incoming);
                    while let Some(parent) = stack.pop() {
                        if marks[parent] == generation {
                            continue;
                        }
                        if aggregate.entries >= max_ancestors {
                            return Err(Reject::ExceededMaximumAncestorsCount.into());
                        }
                        marks[parent] = generation;
                        aggregate = aggregate
                            .checked_add(PackageAggregate::one(&candidates[parent]))
                            .ok_or(PackingError::Arithmetic)?;
                        stack.extend(&parents[parent]);
                    }
                    marked = Some(index);
                    aggregate
                }
            };
            if aggregate.entries > max_ancestors {
                return Err(Reject::ExceededMaximumAncestorsCount.into());
            }
            ancestors[index] = aggregate;
        }
        Ok(Self {
            parents,
            children,
            topological,
            ancestors,
        })
    }
}
