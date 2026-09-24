//! Complete read-before-spend prerequisites for block selection and replay.

use super::{EvictionRank, Links, PackingError, Selection, Status, package_order_key};
use ckb_types::{core::TransactionView, prelude::*};
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap},
};

const MAX_CONDITIONAL_CYCLE_ROUNDS: usize = 64;

pub(super) struct Precedence {
    parents: Links,
    positions: Vec<usize>,
    pub(super) eligible: Vec<usize>,
}

impl Precedence {
    pub(super) fn prerequisites(&self, index: usize) -> &[usize] {
        &self.parents[index]
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "The packing run collects only indices of this precedence graph."
    )]
    pub(super) fn order_package(&self, package: &mut [usize]) {
        package.sort_unstable_by_key(|member| self.positions[*member]);
    }
}

impl Selection<'_> {
    /// Persist every accepted body in dependency order, regardless of proposal
    /// phase. Recovery must retain the same readers that block selection orders
    /// before a spender; raw cell deps cannot reconstruct expanded group members.
    #[expect(
        clippy::indexing_slicing,
        reason = "Both traversals contain only indices from the compiled candidate graph."
    )]
    pub(in crate::authority) fn replay_transactions(
        &self,
    ) -> Result<Vec<TransactionView>, PackingError> {
        let mut remaining = vec![true; self.candidates.len()];
        let mut ordered =
            topological_active_order(&remaining, &self.precedence_graph()?, |index| {
                Reverse(self.candidates[index].arrival)
            })?;
        for &index in &ordered {
            remaining[index] = false;
        }
        // A conditional cycle has no complete read-before-spend order. Retain
        // those bodies in causal order for fresh admission instead of applying
        // the block selector's eviction policy to the persistence snapshot.
        ordered.extend(
            self.graph
                .topological
                .iter()
                .copied()
                .filter(|index| remaining[*index]),
        );
        Ok(ordered
            .into_iter()
            .map(|index| self.candidates[index].owner.transaction.as_ref().clone())
            .collect())
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "The compiled precedence graph and activity mask share checked candidate indices."
    )]
    pub(super) fn precedence(&self) -> Result<Precedence, PackingError> {
        let len = self.candidates.len();
        let mut active = vec![true; len];
        let graph = self.precedence_graph()?;
        let mut cycle_round = 0usize;
        let mut eviction = None;
        loop {
            let ordered = topological_active_order(&active, &graph, |index| {
                package_order_key(index, &self.candidates[index], self.graph.ancestors[index])
            })?;
            if ordered.len() == active.iter().filter(|is_active| **is_active).count() {
                let parents = graph.reversed(&active)?;
                let mut positions = vec![0usize; len];
                let mut proposed = vec![false; len];
                let mut eligible = Vec::new();
                for (position, index) in ordered.into_iter().enumerate() {
                    positions[index] = position;
                    proposed[index] = self.candidates[index].status == Status::Proposed
                        && parents[index].iter().all(|parent| proposed[*parent]);
                    if proposed[index] {
                        eligible.push(index);
                    }
                }
                return Ok(Precedence {
                    parents,
                    positions,
                    eligible,
                });
            }
            let mut cyclic = strongly_connected_active(&active, &graph)?;
            cyclic.retain(|component| component.len() > 1);
            if cyclic.is_empty() {
                return Err(PackingError::Projection);
            }
            cycle_round = cycle_round.checked_add(1).ok_or(PackingError::Arithmetic)?;
            let eviction = match &eviction {
                Some(ranks) => ranks,
                None => eviction.insert(self.eviction_ranks()?),
            };
            let roots = self.cycle_drop_roots(eviction, cyclic, cycle_round)?;
            drop_package_descendants(&mut active, roots, &self.graph.children)?;
        }
    }

    fn precedence_graph(&self) -> Result<Links, PackingError> {
        let mut spenders = HashMap::<&[u8], usize>::with_capacity(self.candidates.len());
        for (index, candidate) in self.candidates.iter().enumerate() {
            for input in candidate
                .accepted
                .transaction
                .transaction
                .input_pts_reader_iter()
            {
                if spenders.insert(input.as_slice(), index).is_some() {
                    return Err(PackingError::Projection);
                }
            }
        }
        let mut edges = Vec::new();
        for (reader, candidate) in self.candidates.iter().enumerate() {
            // Admission closes the reader set when a spender enters the pool.
            // Every retained reader must precede it, including readers that do
            // not fit this block or have not reached the commit window yet.
            for dependency in candidate.accepted.transaction.related_dep_out_points() {
                if let Some(spender) = spenders.get(dependency.as_slice()).copied()
                    && spender != reader
                {
                    edges.push((reader, spender));
                }
            }
        }
        for (parent, children) in self.graph.children.iter().enumerate() {
            edges.extend(children.iter().map(|child| (parent, *child)));
        }
        edges.sort_unstable();
        edges.dedup();
        Links::from_edges(self.candidates.len(), &edges)
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "SCC members and eviction ranks belong to the same graph; binary_search returns a position in this component."
    )]
    fn cycle_drop_roots(
        &self,
        eviction: &[EvictionRank],
        components: Vec<Vec<usize>>,
        round: usize,
    ) -> Result<Vec<bool>, PackingError> {
        let bounded_fallback = round > MAX_CONDITIONAL_CYCLE_ROUNDS;
        let mut dropped = vec![false; self.candidates.len()];
        // The stored package graph is acyclic even when conditional ordering
        // is not. Drop a package leaf within this SCC so its ancestors remain;
        // the bounded fallback retains a package root for the same reason.
        // SCC members are sorted by strongly_connected_active. A package path
        // between two members cannot leave the SCC, so direct edges suffice.
        for component in components {
            debug_assert!(component.is_sorted(), "SCC membership uses binary search");
            let mut eligible = vec![true; component.len()];
            for (position, parent) in component.iter().enumerate() {
                for child in &self.graph.children[*parent] {
                    if let Ok(child_position) = component.binary_search(child) {
                        eligible[if bounded_fallback {
                            child_position
                        } else {
                            position
                        }] = false;
                    }
                }
            }
            let mut choices = component
                .iter()
                .copied()
                .zip(eligible)
                .filter_map(|(index, eligible)| eligible.then_some(index));
            let mut selected = choices.next().ok_or(PackingError::Projection)?;
            for candidate in choices {
                let selected_order = &eviction[selected];
                let candidate_order = &eviction[candidate];
                let replace = if bounded_fallback {
                    candidate_order > selected_order
                } else {
                    candidate_order < selected_order
                };
                if replace {
                    selected = candidate;
                }
            }
            if bounded_fallback {
                for index in component {
                    if index != selected {
                        dropped[index] = true;
                    }
                }
            } else {
                dropped[selected] = true;
            }
        }
        Ok(dropped)
    }
}

#[expect(
    clippy::indexing_slicing,
    reason = "Links validates every endpoint; the activity mask is checked against its node count before traversal."
)]
fn topological_active_order<K: Ord>(
    active: &[bool],
    children: &Links,
    priority: impl Fn(usize) -> K,
) -> Result<Vec<usize>, PackingError> {
    if active.len() != children.len() {
        return Err(PackingError::Projection);
    }
    let mut indegree = vec![0usize; active.len()];
    for (parent, next) in children.iter().enumerate() {
        if !active[parent] {
            continue;
        }
        for child in next {
            if !active[*child] {
                continue;
            }
            let degree = &mut indegree[*child];
            *degree = degree.checked_add(1).ok_or(PackingError::Arithmetic)?;
        }
    }

    // Rank only ready entries, directly by package priority. Each entry becomes
    // ready once, so neither a separate ordinal table nor set deduplication is needed.
    let mut ready = Vec::new();
    for (index, is_active) in active.iter().copied().enumerate() {
        if is_active && indegree[index] == 0 {
            ready.push((priority(index), Reverse(index)));
        }
    }
    let mut ready = BinaryHeap::from(ready);
    let mut ordered = Vec::with_capacity(active.iter().filter(|is_active| **is_active).count());
    while let Some((_priority, Reverse(index))) = ready.pop() {
        ordered.push(index);
        for child in &children[index] {
            if !active[*child] {
                continue;
            }
            let degree = &mut indegree[*child];
            *degree = degree.checked_sub(1).ok_or(PackingError::Projection)?;
            if *degree == 0 {
                ready.push((priority(*child), Reverse(*child)));
            }
        }
    }
    Ok(ordered)
}

/// Iterative Kosaraju traversal; template input is attacker-shaped, so no
/// recursive stack growth is permitted. Each component has sorted, unique
/// indices so cycle_drop_roots can test package edges by binary search.
#[expect(
    clippy::indexing_slicing,
    reason = "Links validates every endpoint; the activity mask is checked against its node count before traversal."
)]
fn strongly_connected_active(
    active: &[bool],
    children: &Links,
) -> Result<Vec<Vec<usize>>, PackingError> {
    if active.len() != children.len() {
        return Err(PackingError::Projection);
    }
    let mut visited = Vec::with_capacity(active.len());
    visited.resize(active.len(), false);
    let mut finish = Vec::with_capacity(active.len());
    let stack_capacity = active
        .len()
        .checked_mul(2)
        .ok_or(PackingError::Arithmetic)?;
    let mut stack = Vec::with_capacity(stack_capacity);
    for start in 0..active.len() {
        if !active[start] || visited[start] {
            continue;
        }
        stack.push((start, false));
        while let Some((index, expanded)) = stack.pop() {
            if expanded {
                finish.push(index);
                continue;
            }
            let seen = &mut visited[index];
            if *seen {
                continue;
            }
            *seen = true;
            stack.push((index, true));
            for child in children[index].iter().rev() {
                if !active[*child] {
                    continue;
                }
                if !visited[*child] {
                    stack.push((*child, false));
                }
            }
        }
    }

    let parents = children.reversed(active)?;

    visited.fill(false);
    let mut components = Vec::with_capacity(active.iter().filter(|is_active| **is_active).count());
    stack.clear();
    for start in finish.into_iter().rev() {
        if visited[start] {
            continue;
        }
        visited[start] = true;
        stack.push((start, false));
        let mut component = Vec::new();
        while let Some((index, _)) = stack.pop() {
            component.reserve(1);
            component.push(index);
            for parent in parents[index].iter().rev() {
                let seen = &mut visited[*parent];
                if !*seen {
                    *seen = true;
                    stack.push((*parent, false));
                }
            }
        }
        component.sort_unstable();
        components.push(component);
    }
    Ok(components)
}

#[expect(
    clippy::indexing_slicing,
    reason = "Links validates every endpoint; both masks are checked against its node count before traversal."
)]
fn drop_package_descendants(
    active: &mut [bool],
    mut dropped: Vec<bool>,
    package_children: &Links,
) -> Result<(), PackingError> {
    if active.len() != dropped.len() || active.len() != package_children.len() {
        return Err(PackingError::Projection);
    }
    let mut stack = Vec::with_capacity(active.len());
    for (index, is_dropped) in dropped.iter().copied().enumerate() {
        if is_dropped {
            if !active[index] {
                return Err(PackingError::Projection);
            }
            stack.push(index);
        }
    }
    if stack.is_empty() {
        return Err(PackingError::Projection);
    }
    while let Some(index) = stack.pop() {
        for child in &package_children[index] {
            if !active[*child] {
                continue;
            }
            let child_dropped = &mut dropped[*child];
            if !*child_dropped {
                *child_dropped = true;
                stack.push(*child);
            }
        }
    }
    for (index, is_dropped) in dropped.into_iter().enumerate() {
        if is_dropped {
            active[index] = false;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/packing_ordering.rs"]
mod tests;
