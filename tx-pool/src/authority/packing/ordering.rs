//! Complete read-before-spend prerequisites for block selection.

use super::{
    CandidatePackingState, EvictionRank, Links, PackageAggregate, PackageOrderKey, PackingError,
    Selection, Status, TemplatePackingLimits, Traversal,
};
use ckb_types::prelude::*;
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
    /// Expand only a block-sized prefix of the complete prerequisite closure.
    /// Unselected readers are mandatory for spending, but are not added to the
    /// stored causal ancestry or its fee aggregates.
    #[expect(
        clippy::indexing_slicing,
        reason = "The compiled precedence graph supplies checked candidate indices and their topological positions."
    )]
    pub(super) fn collect_package(
        &self,
        selection: &Selection<'_>,
        index: usize,
        states: &[CandidatePackingState],
        limits: TemplatePackingLimits,
        traversal: &mut Traversal,
        package: &mut Vec<usize>,
    ) -> Result<Option<PackageAggregate>, PackingError> {
        package.clear();
        traversal.begin()?;
        traversal.stack.push(index);
        let mut aggregate = PackageAggregate::default();
        while let Some(member) = traversal.stack.pop() {
            if states[member] == CandidatePackingState::Selected
                || traversal.marks[member] == traversal.generation
            {
                continue;
            }
            if states[member] == CandidatePackingState::Ineligible {
                return Ok(None);
            }
            traversal.marks[member] = traversal.generation;
            aggregate = aggregate
                .checked_add(PackageAggregate::one(&selection.candidates[member]))
                .ok_or(PackingError::Arithmetic)?;
            if !aggregate.fits(limits) {
                return Ok(None);
            }
            package.push(member);
            traversal
                .stack
                .extend(self.parents.get(member).ok_or(PackingError::Projection)?);
        }
        package.sort_unstable_by_key(|member| self.positions[*member]);
        Ok(Some(aggregate))
    }
}

impl Selection<'_> {
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
                PackageOrderKey::new(index, &self.candidates[index], self.graph.ancestors[index])
            })?;
            if ordered.len() == active.iter().filter(|is_active| **is_active).count() {
                let mut edges = Vec::new();
                for (parent, children) in graph.iter().enumerate() {
                    if active[parent] {
                        edges.extend(
                            children
                                .iter()
                                .filter_map(|child| active[*child].then_some((*child, parent))),
                        );
                    }
                }
                let parents = Links::from_edges(len, &edges)?;
                let mut positions = vec![0usize; len];
                let mut proposed = vec![false; len];
                let mut eligible = Vec::new();
                for (position, index) in ordered.into_iter().enumerate() {
                    positions[index] = position;
                    proposed[index] = self.candidates[index].status == Status::Proposed
                        && parents
                            .get(index)
                            .ok_or(PackingError::Projection)?
                            .iter()
                            .all(|parent| proposed[*parent]);
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
            let bounded_fallback = cycle_round > MAX_CONDITIONAL_CYCLE_ROUNDS;
            let eviction = match &eviction {
                Some(ranks) => ranks,
                None => eviction.insert(self.eviction_ranks()?),
            };
            let mut roots = vec![false; active.len()];
            for component in cyclic {
                let chosen = Self::cycle_representative(
                    eviction,
                    &component,
                    bounded_fallback,
                    &self.graph.children,
                )?;
                if bounded_fallback {
                    for index in component {
                        if index != chosen {
                            *roots.get_mut(index).ok_or(PackingError::Projection)? = true;
                        }
                    }
                } else {
                    *roots.get_mut(chosen).ok_or(PackingError::Projection)? = true;
                }
            }
            drop_package_descendants(&mut active, roots, &self.graph.children)?;
        }
    }

    fn precedence_graph(&self) -> Result<Links, PackingError> {
        let mut spenders = HashMap::<&[u8], usize>::new();
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

    fn cycle_representative(
        eviction: &[EvictionRank],
        component: &[usize],
        strongest: bool,
        package_children: &Links,
    ) -> Result<usize, PackingError> {
        // The stored package graph is acyclic even when conditional ordering
        // is not. Drop a package leaf within this SCC so its ancestors remain;
        // the bounded fallback retains a package root for the same reason.
        // SCC members are sorted by strongly_connected_active. A package path
        // between two members cannot leave the SCC, so direct edges suffice.
        let mut eligible = vec![true; component.len()];
        for (position, parent) in component.iter().enumerate() {
            for child in package_children
                .get(*parent)
                .ok_or(PackingError::Projection)?
            {
                if let Ok(child_position) = component.binary_search(child) {
                    *eligible
                        .get_mut(if strongest { child_position } else { position })
                        .ok_or(PackingError::Projection)? = false;
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
            let selected_order = eviction.get(selected).ok_or(PackingError::Projection)?;
            let candidate_order = eviction.get(candidate).ok_or(PackingError::Projection)?;
            let replace = if strongest {
                candidate_order > selected_order
            } else {
                candidate_order < selected_order
            };
            if replace {
                selected = candidate;
            }
        }
        Ok(selected)
    }
}

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
        if !active
            .get(parent)
            .copied()
            .ok_or(PackingError::Projection)?
        {
            continue;
        }
        for child in next {
            if !active
                .get(*child)
                .copied()
                .ok_or(PackingError::Projection)?
            {
                continue;
            }
            let degree = indegree.get_mut(*child).ok_or(PackingError::Projection)?;
            *degree = degree.checked_add(1).ok_or(PackingError::Arithmetic)?;
        }
    }

    // Rank only ready entries, directly by package priority. Each entry becomes
    // ready once, so neither a separate ordinal table nor set deduplication is needed.
    let mut ready = Vec::new();
    for (index, is_active) in active.iter().copied().enumerate() {
        if is_active
            && indegree
                .get(index)
                .copied()
                .ok_or(PackingError::Projection)?
                == 0
        {
            ready.push((priority(index), Reverse(index)));
        }
    }
    let mut ready = BinaryHeap::from(ready);
    let mut ordered = Vec::with_capacity(active.iter().filter(|is_active| **is_active).count());
    while let Some((_priority, Reverse(index))) = ready.pop() {
        ordered.push(index);
        for child in children.get(index).ok_or(PackingError::Projection)? {
            if !active
                .get(*child)
                .copied()
                .ok_or(PackingError::Projection)?
            {
                continue;
            }
            let degree = indegree.get_mut(*child).ok_or(PackingError::Projection)?;
            *degree = degree.checked_sub(1).ok_or(PackingError::Projection)?;
            if *degree == 0 {
                ready.push((priority(*child), Reverse(*child)));
            }
        }
    }
    Ok(ordered)
}

/// Iterative Kosaraju traversal; template input is attacker-shaped, so no
/// recursive stack growth is permitted.
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
        if !active.get(start).copied().ok_or(PackingError::Projection)?
            || visited
                .get(start)
                .copied()
                .ok_or(PackingError::Projection)?
        {
            continue;
        }
        stack.push((start, false));
        while let Some((index, expanded)) = stack.pop() {
            if expanded {
                finish.push(index);
                continue;
            }
            let seen = visited.get_mut(index).ok_or(PackingError::Projection)?;
            if *seen {
                continue;
            }
            *seen = true;
            stack.push((index, true));
            for child in children
                .get(index)
                .ok_or(PackingError::Projection)?
                .iter()
                .rev()
            {
                if !active
                    .get(*child)
                    .copied()
                    .ok_or(PackingError::Projection)?
                {
                    continue;
                }
                if !visited
                    .get(*child)
                    .copied()
                    .ok_or(PackingError::Projection)?
                {
                    stack.push((*child, false));
                }
            }
        }
    }

    let mut parent_counts = vec![0usize; active.len()];
    for (parent, next) in children.iter().enumerate() {
        if !active
            .get(parent)
            .copied()
            .ok_or(PackingError::Projection)?
        {
            continue;
        }
        for child in next {
            if !active
                .get(*child)
                .copied()
                .ok_or(PackingError::Projection)?
            {
                continue;
            }
            let count = parent_counts
                .get_mut(*child)
                .ok_or(PackingError::Projection)?;
            *count = count.checked_add(1).ok_or(PackingError::Arithmetic)?;
        }
    }
    let mut parents = Vec::with_capacity(active.len());
    for count in parent_counts {
        let row = Vec::with_capacity(count);
        parents.push(row);
    }
    for (parent, next) in children.iter().enumerate() {
        if !active
            .get(parent)
            .copied()
            .ok_or(PackingError::Projection)?
        {
            continue;
        }
        for child in next {
            if !active
                .get(*child)
                .copied()
                .ok_or(PackingError::Projection)?
            {
                continue;
            }
            parents
                .get_mut(*child)
                .ok_or(PackingError::Projection)?
                .push(parent);
        }
    }
    for previous in &mut parents {
        previous.sort_unstable();
    }

    visited.fill(false);
    let mut components = Vec::with_capacity(active.iter().filter(|is_active| **is_active).count());
    stack.clear();
    for start in finish.into_iter().rev() {
        if visited
            .get(start)
            .copied()
            .ok_or(PackingError::Projection)?
        {
            continue;
        }
        *visited.get_mut(start).ok_or(PackingError::Projection)? = true;
        stack.push((start, false));
        let mut component = Vec::new();
        while let Some((index, _)) = stack.pop() {
            component.reserve(1);
            component.push(index);
            for parent in parents
                .get(index)
                .ok_or(PackingError::Projection)?
                .iter()
                .rev()
            {
                let seen = visited.get_mut(*parent).ok_or(PackingError::Projection)?;
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
            if !active.get(index).copied().ok_or(PackingError::Projection)? {
                return Err(PackingError::Projection);
            }
            stack.push(index);
        }
    }
    if stack.is_empty() {
        return Err(PackingError::Projection);
    }
    while let Some(index) = stack.pop() {
        for child in package_children
            .get(index)
            .ok_or(PackingError::Projection)?
        {
            if !active
                .get(*child)
                .copied()
                .ok_or(PackingError::Projection)?
            {
                continue;
            }
            let child_dropped = dropped.get_mut(*child).ok_or(PackingError::Projection)?;
            if !*child_dropped {
                *child_dropped = true;
                stack.push(*child);
            }
        }
    }
    for (index, is_dropped) in dropped.into_iter().enumerate() {
        if is_dropped {
            *active.get_mut(index).ok_or(PackingError::Projection)? = false;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/packing_ordering.rs"]
mod tests;
