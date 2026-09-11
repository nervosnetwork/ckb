//! One immutable causal graph compiled from the captured accepted owners.

use super::{Candidate, PackageAggregate, PackingError};
use crate::authority::model::Error;
use crate::error::Reject;
use std::collections::HashMap;

/// Compact adjacency. Offsets are private and always describe valid slices;
/// algorithms still check endpoint indices when accepting an external graph.
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

    pub(super) fn get(&self, index: usize) -> Option<&[usize]> {
        let start = *self.offsets.get(index)?;
        let end = *self.offsets.get(index.checked_add(1)?)?;
        self.edges.get(start..end)
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

    /// Graph algorithms' rejection tests also need malformed endpoint values.
    #[cfg(test)]
    pub(super) fn from_lists(lists: impl IntoIterator<Item = Vec<usize>>) -> Self {
        let mut offsets = vec![0];
        let mut edges = Vec::new();
        for list in lists {
            edges.extend(list);
            offsets.push(edges.len());
        }
        Self { offsets, edges }
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
                edges.push((*positions.get(parent).ok_or(Error::Stale)?, child));
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
            for &child in children.get(index).ok_or(PackingError::Projection)? {
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
            let incoming = parents.get(index).ok_or(PackingError::Projection)?;
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
                        stack.extend(parents.get(parent).ok_or(PackingError::Projection)?);
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
