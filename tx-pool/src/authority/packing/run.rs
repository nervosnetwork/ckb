//! One packing run owns candidate queues, block limits and ordered selection.

use super::ordering::Precedence;
use super::{
    Links, PackageAggregate, PackageOrderKey, PackingError, Selection, TemplatePackingLimits,
    Traversal, package_order_key,
};
use crate::component::entry::TxEntry;
use std::{
    borrow::Cow,
    cmp::Reverse,
    collections::{BTreeSet, BinaryHeap},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CandidatePackingState {
    Ineligible,
    Original,
    Modified,
    Failed,
    Selected,
}
impl CandidatePackingState {
    fn queued(self) -> bool {
        matches!(self, Self::Original | Self::Modified)
    }
}

/// Candidate membership, residual scores and descendant uses for one block.
pub(super) struct PackingRun<'selection, 'owner> {
    selection: &'selection Selection<'owner>,
    precedence: Precedence,
    limits: TemplatePackingLimits,
    aggregates: Cow<'selection, [PackageAggregate]>,
    states: Vec<CandidatePackingState>,
    original: BinaryHeap<PackageOrderKey<'selection>>,
    modified: BTreeSet<PackageOrderKey<'selection>>,
    live_children: Vec<usize>,
    minimum_bytes: usize,
    traversal: Traversal,
    package: Vec<usize>,
    adjustments: Vec<PackageAggregate>,
    changed: Vec<usize>,
}

#[expect(
    clippy::indexing_slicing,
    reason = "The compiled graph defines every candidate, edge and overlay index; all queues are populated from those indices."
)]
impl<'selection, 'owner> PackingRun<'selection, 'owner> {
    pub(super) fn new(
        selection: &'selection Selection<'owner>,
        limits: TemplatePackingLimits,
    ) -> Result<Option<Self>, PackingError> {
        let precedence = selection.precedence()?;
        let eligible = &precedence.eligible;
        if eligible.is_empty() {
            return Ok(None);
        }
        let len = selection.candidates.len();
        let aggregates = Cow::Borrowed(selection.graph.ancestors.as_slice());
        let mut states = vec![CandidatePackingState::Ineligible; len];
        let mut original = Vec::with_capacity(eligible.len());
        let mut minimum_bytes = usize::MAX;
        for &index in eligible {
            if aggregates[index].fits(limits) {
                minimum_bytes = minimum_bytes.min(selection.candidates[index].accepted.size);
                states[index] = CandidatePackingState::Original;
                original.push(package_order_key(
                    index,
                    &selection.candidates[index],
                    aggregates[index],
                ));
            }
        }
        if original.is_empty() {
            return Ok(None);
        }
        let mut live_children = vec![0usize; len];
        for &index in selection.graph.topological.iter().rev() {
            if states[index].queued() || live_children[index] != 0 {
                for &parent in &selection.graph.parents[index] {
                    live_children[parent] = live_children[parent]
                        .checked_add(1)
                        .ok_or(PackingError::Arithmetic)?;
                }
            }
        }
        Ok(Some(Self {
            selection,
            precedence,
            limits,
            aggregates,
            states,
            original: BinaryHeap::from(original),
            modified: BTreeSet::new(),
            live_children,
            minimum_bytes,
            traversal: Traversal::new(len),
            package: Vec::new(),
            adjustments: vec![PackageAggregate::default(); len],
            changed: Vec::new(),
        }))
    }

    pub(super) fn pack(
        mut self,
        max_consecutive_failures: usize,
    ) -> Result<Vec<TxEntry>, PackingError> {
        let mut selected = Vec::with_capacity(self.precedence.eligible.len());
        let mut selected_bytes = 0usize;
        let mut selected_cycles = 0u64;
        let mut consecutive_failures = 0usize;
        while let Some((index, aggregate)) = self.next_candidate()? {
            let remaining = TemplatePackingLimits::new(
                self.limits
                    .serialized_bytes
                    .checked_sub(selected_bytes)
                    .ok_or(PackingError::Projection)?,
                self.limits
                    .cycles
                    .checked_sub(selected_cycles)
                    .ok_or(PackingError::Projection)?,
            );
            // Preserve projected arithmetic rejection even for synthetic limits.
            selected_bytes
                .checked_add(aggregate.serialized_bytes)
                .ok_or(PackingError::Arithmetic)?;
            selected_cycles
                .checked_add(aggregate.cycles)
                .ok_or(PackingError::Arithmetic)?;
            let Some(actual) = self.collect_package(index, remaining)? else {
                self.finish_candidate(index, CandidatePackingState::Failed)?;
                self.package.clear();
                consecutive_failures = consecutive_failures
                    .checked_add(1)
                    .ok_or(PackingError::Arithmetic)?;
                // Unfitting spend packages must not prevent their independently
                // fitting readers from making the first progress in this block.
                if consecutive_failures > max_consecutive_failures && !selected.is_empty() {
                    break;
                }
                continue;
            };
            // Finish the entire package before visiting its remaining consumers.
            for position in (0..self.package.len()).rev() {
                self.finish_candidate(self.package[position], CandidatePackingState::Selected)?;
            }
            selected.extend_from_slice(&self.package);
            selected_bytes = selected_bytes
                .checked_add(actual.serialized_bytes)
                .ok_or(PackingError::Arithmetic)?;
            selected_cycles = selected_cycles
                .checked_add(actual.cycles)
                .ok_or(PackingError::Arithmetic)?;
            consecutive_failures = 0;
            // Every remaining package contains a queued candidate's own bytes,
            // even after selected ancestors are subtracted. Keep checked-add
            // errors observable when synthetic limits approach integer bounds.
            if self.limits.serialized_bytes.saturating_sub(selected_bytes) < self.minimum_bytes
                && selected_bytes
                    .checked_add(self.limits.serialized_bytes)
                    .is_some()
                && selected_cycles.checked_add(self.limits.cycles).is_some()
            {
                break;
            }
            self.reprice_descendants()?;
        }
        // Each package was deduplicated, excluded selected entries and checked
        // against the remaining limits before these immutable owners were added.
        Ok(selected
            .into_iter()
            .map(|index| self.selection.candidates[index].accepted.projection())
            .collect())
    }

    /// Expand only a block-sized prefix of the complete prerequisite closure.
    /// Failed roots can still be required by another package. Unselected readers
    /// are mandatory for spending but do not enter causal fee aggregates.
    fn collect_package(
        &mut self,
        index: usize,
        limits: TemplatePackingLimits,
    ) -> Result<Option<PackageAggregate>, PackingError> {
        self.package.clear();
        let mut pass = self.traversal.begin()?;
        pass.stack.push(index);
        let mut aggregate = PackageAggregate::default();
        while let Some(member) = pass.stack.pop() {
            if self.states[member] == CandidatePackingState::Selected || !pass.visit(member) {
                continue;
            }
            if self.states[member] == CandidatePackingState::Ineligible {
                return Ok(None);
            }
            aggregate = aggregate
                .checked_add(PackageAggregate::one(&self.selection.candidates[member]))
                .ok_or(PackingError::Arithmetic)?;
            if !aggregate.fits(limits) {
                return Ok(None);
            }
            self.package.push(member);
            pass.stack.extend(self.precedence.prerequisites(member));
        }
        self.precedence.order_package(&mut self.package);
        Ok(Some(aggregate))
    }

    /// Inspect the highest live score. Its queue use remains until settlement,
    /// so collection needs no intermediate candidate state.
    fn next_candidate(&mut self) -> Result<Option<(usize, PackageAggregate)>, PackingError> {
        while self.original.peek().is_some_and(|(_, Reverse(index))| {
            self.states[*index] != CandidatePackingState::Original
        }) {
            self.original.pop();
        }
        let key = match (self.original.peek(), self.modified.last()) {
            (None, None) => return Ok(None),
            (Some(initial), Some(updated)) if updated > initial => updated,
            (None, Some(updated)) => updated,
            (Some(initial), _) => initial,
        };
        let (_, Reverse(index)) = key;
        let index = *index;
        let aggregate = self.aggregates[index];
        if !self.states[index].queued()
            || key != &package_order_key(index, &self.selection.candidates[index], aggregate)
        {
            return Err(PackingError::Projection);
        }
        Ok(Some((index, aggregate)))
    }

    fn finish_candidate(
        &mut self,
        index: usize,
        state: CandidatePackingState,
    ) -> Result<(), PackingError> {
        let previous = self.states[index];
        if previous == CandidatePackingState::Modified
            && !self.modified.remove(&package_order_key(
                index,
                &self.selection.candidates[index],
                self.aggregates[index],
            ))
        {
            return Err(PackingError::Projection);
        }
        self.states[index] = state;
        if previous.queued() {
            retire_candidate(
                index,
                &self.states,
                &mut self.live_children,
                &self.selection.graph.parents,
                &mut self.traversal.stack,
            )?;
        }
        Ok(())
    }

    /// Subtract selected members once per live descendant, then replace each
    /// affected queue score after all package members have been counted.
    fn reprice_descendants(&mut self) -> Result<(), PackingError> {
        for &member in &self.package {
            let delta = PackageAggregate::one(&self.selection.candidates[member]);
            let mut pass = self.traversal.begin()?;
            pass.stack.extend(&self.selection.graph.children[member]);
            while let Some(descendant) = pass.stack.pop() {
                if (!self.states[descendant].queued() && self.live_children[descendant] == 0)
                    || !pass.visit(descendant)
                {
                    continue;
                }
                pass.stack
                    .extend(&self.selection.graph.children[descendant]);
                if self.states[descendant].queued() {
                    if self.adjustments[descendant].entries == 0 {
                        self.changed.push(descendant);
                    }
                    self.adjustments[descendant] = self.adjustments[descendant]
                        .checked_add(delta)
                        .ok_or(PackingError::Arithmetic)?;
                }
            }
        }
        for descendant in self.changed.drain(..) {
            let previous = self.aggregates[descendant];
            if self.states[descendant] == CandidatePackingState::Modified
                && !self.modified.remove(&package_order_key(
                    descendant,
                    &self.selection.candidates[descendant],
                    previous,
                ))
            {
                return Err(PackingError::Projection);
            }
            let remaining = previous
                .checked_sub(self.adjustments[descendant])
                .ok_or(PackingError::Projection)?;
            self.adjustments[descendant] = PackageAggregate::default();
            self.aggregates.to_mut()[descendant] = remaining;
            self.states[descendant] = CandidatePackingState::Modified;
            if !self.modified.insert(package_order_key(
                descendant,
                &self.selection.candidates[descendant],
                remaining,
            )) {
                return Err(PackingError::Projection);
            }
        }
        Ok(())
    }
}

/// Called only when a queued candidate permanently loses its own
/// queue use. Child uses are monotone, so every retiring edge is visited once.
#[expect(
    clippy::indexing_slicing,
    reason = "All parents and overlay indices belong to the same compiled graph."
)]
fn retire_candidate(
    index: usize,
    states: &[CandidatePackingState],
    live_children: &mut [usize],
    parents: &Links,
    stack: &mut Vec<usize>,
) -> Result<(), PackingError> {
    if live_children[index] != 0 {
        return Ok(());
    }
    stack.clear();
    stack.push(index);
    while let Some(retired) = stack.pop() {
        for &parent in &parents[retired] {
            live_children[parent] = live_children[parent]
                .checked_sub(1)
                .ok_or(PackingError::Projection)?;
            if live_children[parent] == 0 && !states[parent].queued() {
                stack.push(parent);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/packing_run.rs"]
mod tests;
