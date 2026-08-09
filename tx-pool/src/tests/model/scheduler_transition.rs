//! Set-transition and owner-ring relations for the derived scheduler.

use super::scheduler_quotient::{
    SchedulerRefinementCursors, SchedulerRefinementEntry, SchedulerRefinementOwner,
    SchedulerRefinementVerifyOrder,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SchedulerProjectionChange {
    pub(crate) transaction: u8,
    pub(crate) expected: Option<SchedulerRefinementEntry>,
    pub(crate) after: Option<SchedulerRefinementEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SchedulerSetProjection {
    entries: BTreeMap<u8, SchedulerRefinementEntry>,
    verify_order: SchedulerRefinementVerifyOrder,
    cursors: SchedulerRefinementCursors,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SchedulerProjectionError {
    DuplicateTransaction(u8),
    IdentityMismatch(u8),
    ExistingEntryMismatch(u8),
    ZeroSerializedBytes(u8),
}

impl SchedulerSetProjection {
    pub(crate) fn new(
        entries: impl IntoIterator<Item = SchedulerRefinementEntry>,
        verify_order: SchedulerRefinementVerifyOrder,
        cursors: SchedulerRefinementCursors,
    ) -> Result<Self, SchedulerProjectionError> {
        let mut projected = BTreeMap::new();
        for entry in entries {
            if entry.bytes == 0 {
                return Err(SchedulerProjectionError::ZeroSerializedBytes(
                    entry.transaction,
                ));
            }
            if projected.insert(entry.transaction, entry).is_some() {
                return Err(SchedulerProjectionError::DuplicateTransaction(
                    entry.transaction,
                ));
            }
        }
        Ok(Self {
            entries: projected,
            verify_order,
            cursors,
        })
    }

    pub(crate) fn entries(&self) -> &BTreeMap<u8, SchedulerRefinementEntry> {
        &self.entries
    }

    pub(crate) const fn verify_order(&self) -> SchedulerRefinementVerifyOrder {
        self.verify_order
    }

    pub(crate) const fn cursors(&self) -> SchedulerRefinementCursors {
        self.cursors
    }

    pub(crate) fn plan_changes(
        &self,
        changes: &[SchedulerProjectionChange],
        cursors: SchedulerRefinementCursors,
    ) -> Result<Self, SchedulerProjectionError> {
        let mut changed = BTreeSet::new();
        for change in changes {
            if !changed.insert(change.transaction) {
                return Err(SchedulerProjectionError::DuplicateTransaction(
                    change.transaction,
                ));
            }
            if change
                .expected
                .is_some_and(|entry| entry.transaction != change.transaction)
                || change
                    .after
                    .is_some_and(|entry| entry.transaction != change.transaction)
            {
                return Err(SchedulerProjectionError::IdentityMismatch(
                    change.transaction,
                ));
            }
            if self.entries.get(&change.transaction).copied() != change.expected {
                return Err(SchedulerProjectionError::ExistingEntryMismatch(
                    change.transaction,
                ));
            }
            if let Some(after) = change.after
                && after.bytes == 0
            {
                return Err(SchedulerProjectionError::ZeroSerializedBytes(
                    change.transaction,
                ));
            }
        }

        let mut entries = self.entries.clone();
        for change in changes {
            match change.after {
                Some(after) => {
                    entries.insert(change.transaction, after);
                }
                None => {
                    entries.remove(&change.transaction);
                }
            }
        }
        Ok(Self {
            entries,
            verify_order: self.verify_order,
            cursors,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SchedulerOwnerPopulation {
    pub(crate) all: BTreeSet<SchedulerRefinementOwner>,
    pub(crate) small: BTreeSet<SchedulerRefinementOwner>,
}

impl SchedulerOwnerPopulation {
    pub(crate) fn new(
        all: impl IntoIterator<Item = SchedulerRefinementOwner>,
        small: impl IntoIterator<Item = SchedulerRefinementOwner>,
    ) -> Option<Self> {
        let all = all.into_iter().collect::<BTreeSet<_>>();
        let small = small.into_iter().collect::<BTreeSet<_>>();
        small.is_subset(&all).then_some(Self { all, small })
    }

    fn selected(&self, small_only: bool) -> &BTreeSet<SchedulerRefinementOwner> {
        if small_only { &self.small } else { &self.all }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SchedulerOwnerRing {
    committed: SchedulerOwnerPopulation,
    overlay: SchedulerOwnerPopulation,
    cursor: Option<SchedulerRefinementOwner>,
}

impl SchedulerOwnerRing {
    pub(crate) const fn new(
        committed: SchedulerOwnerPopulation,
        overlay: SchedulerOwnerPopulation,
        cursor: Option<SchedulerRefinementOwner>,
    ) -> Self {
        Self {
            committed,
            overlay,
            cursor,
        }
    }

    pub(crate) fn owner_bound(&self, small_only: bool) -> Option<usize> {
        self.committed
            .selected(small_only)
            .len()
            .checked_add(self.overlay.selected(small_only).len())
    }

    pub(crate) fn overlay_owner_is_eligible(
        &self,
        small_only: bool,
        owner: SchedulerRefinementOwner,
    ) -> bool {
        self.committed.selected(small_only).contains(&owner)
            || self.overlay.selected(small_only).contains(&owner)
    }

    pub(crate) fn first_available(
        &self,
        small_only: bool,
        blocked: &BTreeSet<SchedulerRefinementOwner>,
    ) -> Option<SchedulerRefinementOwner> {
        if self.cursor.is_none()
            && self.overlay_owner_is_eligible(small_only, SchedulerRefinementOwner::Trusted)
            && !blocked.contains(&SchedulerRefinementOwner::Trusted)
        {
            return Some(SchedulerRefinementOwner::Trusted);
        }
        let mut cursor = self.cursor;
        for _ in 0..self.owner_bound(small_only)? {
            let owner = self.next_owner(small_only, cursor)?;
            if !blocked.contains(&owner) {
                return Some(owner);
            }
            cursor = Some(owner);
        }
        None
    }

    fn next_owner(
        &self,
        small_only: bool,
        cursor: Option<SchedulerRefinementOwner>,
    ) -> Option<SchedulerRefinementOwner> {
        let committed = self.committed.selected(small_only);
        let overlay = self.overlay.selected(small_only);
        let choose = |left: Option<SchedulerRefinementOwner>,
                      right: Option<SchedulerRefinementOwner>| {
            match (left, right) {
                (Some(left), Some(right)) => Some(std::cmp::min(left, right)),
                (Some(owner), None) | (None, Some(owner)) => Some(owner),
                (None, None) => None,
            }
        };
        let next = cursor.and_then(|cursor| {
            choose(
                committed
                    .range((
                        std::ops::Bound::Excluded(cursor),
                        std::ops::Bound::Unbounded,
                    ))
                    .next()
                    .copied(),
                overlay
                    .range((
                        std::ops::Bound::Excluded(cursor),
                        std::ops::Bound::Unbounded,
                    ))
                    .next()
                    .copied(),
            )
        });
        next.or_else(|| choose(committed.first().copied(), overlay.first().copied()))
    }
}
