//! Conditional read-before-spend ordering for an already selected transaction set.

use super::{EvictionRank, MAX_CONDITIONAL_CYCLE_ROUNDS, PackingError, Selection};
use ckb_types::packed::{Byte32, OutPoint};
use std::collections::{BTreeSet, HashMap, HashSet};

impl Selection {
    pub(super) fn order_packed_indices(
        &self,
        selected: Vec<usize>,
        by_hash: &HashMap<Byte32, usize>,
    ) -> Result<Vec<usize>, PackingError> {
        if selected.len() < 2 {
            return Ok(selected);
        }

        let mut rank = Vec::with_capacity(self.candidates.len());
        rank.resize(self.candidates.len(), None);
        let mut active = Vec::with_capacity(self.candidates.len());
        active.resize(self.candidates.len(), false);
        for (position, index) in selected.iter().copied().enumerate() {
            let slot = rank.get_mut(index).ok_or(PackingError::Projection)?;
            if slot.replace(position).is_some() {
                return Err(PackingError::Projection);
            }
            *active.get_mut(index).ok_or(PackingError::Projection)? = true;
        }

        let mut cycle_round = 0usize;
        let mut eviction = None;
        loop {
            let graph = self.conditional_graph(&active, by_hash)?;
            let ordered = topological_active_order(&active, &rank, &graph.children)?;
            if ordered.len() == active.iter().filter(|is_active| **is_active).count() {
                return Ok(ordered);
            }

            let mut cyclic = strongly_connected_active(&active, &graph.children)?;
            cyclic.retain(|component| component.len() > 1);
            if cyclic.is_empty() {
                return Err(PackingError::Projection);
            }
            cycle_round = cycle_round.checked_add(1).ok_or(PackingError::Arithmetic)?;
            let bounded_fallback = cycle_round > MAX_CONDITIONAL_CYCLE_ROUNDS;
            let eviction = match &eviction {
                Some(ranks) => ranks,
                None => eviction.insert(self.eviction_ranks()),
            };
            let mut roots = Vec::with_capacity(active.len());
            roots.resize(active.len(), false);
            for component in cyclic {
                let chosen = Self::cycle_representative(
                    eviction,
                    &component,
                    bounded_fallback,
                    &graph.package_children,
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
            drop_package_descendants(&mut active, roots, &graph.package_children)?;
            if active.iter().filter(|is_active| **is_active).count() < 2 {
                let mut retained = Vec::with_capacity(1);
                retained.extend(
                    selected
                        .iter()
                        .copied()
                        .filter(|index| active.get(*index).is_some_and(|is_active| *is_active)),
                );
                return Ok(retained);
            }
        }
    }

    fn conditional_graph(
        &self,
        active: &[bool],
        by_hash: &HashMap<Byte32, usize>,
    ) -> Result<SelectedGraph, PackingError> {
        if active.len() != self.candidates.len() {
            return Err(PackingError::Projection);
        }
        let mut package_edges = Vec::new();
        let mut input_count = 0usize;
        let mut dependency_count = 0usize;
        for (child, candidate) in self.candidates.iter().enumerate() {
            if !active.get(child).copied().ok_or(PackingError::Projection)? {
                continue;
            }
            input_count = input_count
                .checked_add(candidate.accepted.transaction.transaction.inputs().len())
                .ok_or(PackingError::Arithmetic)?;
            dependency_count = dependency_count
                .checked_add(candidate.accepted.dependencies().count())
                .ok_or(PackingError::Arithmetic)?;
            for parent in candidate.accepted.parents.iter() {
                let parent = *by_hash.get(parent).ok_or(PackingError::Projection)?;
                if active.get(parent).is_some_and(|is_active| *is_active) {
                    package_edges.reserve(1);
                    package_edges.push((parent, child));
                }
            }
        }
        if dependency_count > self.dependency_edge_bound {
            return Err(PackingError::Projection);
        }
        let edge_capacity = package_edges
            .len()
            .checked_add(dependency_count)
            .ok_or(PackingError::Arithmetic)?;
        let mut edges = HashSet::with_capacity(edge_capacity);
        edges.extend(package_edges.iter().copied());

        let mut spenders = HashMap::<OutPoint, usize>::with_capacity(input_count);
        for (index, candidate) in self.candidates.iter().enumerate() {
            if !active.get(index).copied().ok_or(PackingError::Projection)? {
                continue;
            }
            for input in candidate.accepted.transaction.transaction.input_pts_iter() {
                if spenders.insert(input, index).is_some() {
                    return Err(PackingError::Projection);
                }
            }
        }
        for (reader, candidate) in self.candidates.iter().enumerate() {
            if !active
                .get(reader)
                .copied()
                .ok_or(PackingError::Projection)?
            {
                continue;
            }
            for dependency in candidate.accepted.dependencies() {
                if let Some(spender) = spenders.get(&dependency).copied()
                    && spender != reader
                {
                    edges.insert((reader, spender));
                }
            }
        }
        SelectedGraph::from_edges(active.len(), edges, package_edges)
    }

    fn cycle_representative(
        eviction: &[EvictionRank],
        component: &[usize],
        strongest: bool,
        package_children: &[Vec<usize>],
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

struct SelectedGraph {
    children: Vec<Vec<usize>>,
    package_children: Vec<Vec<usize>>,
}

impl SelectedGraph {
    fn from_edges(
        len: usize,
        edges: HashSet<(usize, usize)>,
        package_edges: Vec<(usize, usize)>,
    ) -> Result<Self, PackingError> {
        let mut child_counts = vec![0usize; len];
        for (parent, child) in &edges {
            if parent == child || *parent >= len || *child >= len {
                return Err(PackingError::Projection);
            }
            let count = child_counts
                .get_mut(*parent)
                .ok_or(PackingError::Projection)?;
            *count = count.checked_add(1).ok_or(PackingError::Arithmetic)?;
        }
        let mut package_counts = vec![0usize; len];
        for (parent, child) in &package_edges {
            if parent == child || *parent >= len || *child >= len {
                return Err(PackingError::Projection);
            }
            let count = package_counts
                .get_mut(*parent)
                .ok_or(PackingError::Projection)?;
            *count = count.checked_add(1).ok_or(PackingError::Arithmetic)?;
        }

        let mut children = Vec::with_capacity(len);
        let mut package_children = Vec::with_capacity(len);
        for index in 0..len {
            let next =
                Vec::with_capacity(*child_counts.get(index).ok_or(PackingError::Projection)?);
            children.push(next);
            let package_next =
                Vec::with_capacity(*package_counts.get(index).ok_or(PackingError::Projection)?);
            package_children.push(package_next);
        }
        for (parent, child) in edges {
            children
                .get_mut(parent)
                .ok_or(PackingError::Projection)?
                .push(child);
        }
        for (parent, child) in package_edges {
            package_children
                .get_mut(parent)
                .ok_or(PackingError::Projection)?
                .push(child);
        }
        for next in &mut children {
            next.sort_unstable();
        }
        for next in &mut package_children {
            next.sort_unstable();
        }
        Ok(Self {
            children,
            package_children,
        })
    }
}

fn topological_active_order(
    active: &[bool],
    rank: &[Option<usize>],
    children: &[Vec<usize>],
) -> Result<Vec<usize>, PackingError> {
    if active.len() != rank.len() || active.len() != children.len() {
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
            if !active.get(*child).is_some_and(|is_active| *is_active) {
                return Err(PackingError::Projection);
            }
            let degree = indegree.get_mut(*child).ok_or(PackingError::Projection)?;
            *degree = degree.checked_add(1).ok_or(PackingError::Arithmetic)?;
        }
    }

    let mut ready = BTreeSet::new();
    for (index, is_active) in active.iter().copied().enumerate() {
        if is_active
            && indegree
                .get(index)
                .copied()
                .ok_or(PackingError::Projection)?
                == 0
        {
            let position = rank
                .get(index)
                .and_then(|position| *position)
                .ok_or(PackingError::Projection)?;
            ready.insert((position, index));
        }
    }
    let mut ordered = Vec::with_capacity(active.iter().filter(|is_active| **is_active).count());
    while let Some((_position, index)) = ready.pop_first() {
        ordered.push(index);
        for child in children.get(index).ok_or(PackingError::Projection)? {
            let degree = indegree.get_mut(*child).ok_or(PackingError::Projection)?;
            *degree = degree.checked_sub(1).ok_or(PackingError::Projection)?;
            if *degree == 0 {
                let position = rank
                    .get(*child)
                    .and_then(|position| *position)
                    .ok_or(PackingError::Projection)?;
                ready.insert((position, *child));
            }
        }
    }
    Ok(ordered)
}

/// Iterative Kosaraju traversal; template input is attacker-shaped, so no
/// recursive stack growth is permitted.
fn strongly_connected_active(
    active: &[bool],
    children: &[Vec<usize>],
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
                if !active.get(*child).is_some_and(|is_active| *is_active) {
                    return Err(PackingError::Projection);
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
    package_children: &[Vec<usize>],
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
