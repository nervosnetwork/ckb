use super::scheduler::{
    DependencyIngressPublication, PublishedIngressVisibility, StagedIngressVisibility,
};
use super::shard::{ShardReadSupport, ShardWriteSupport, ShardedOwnerMap, ShardedOwnerWriteCut};
use super::state::{
    DependencyCut, DependencyKey, DependencyOrigin, KnownDependencies, MissingDependencies,
    ObservedDependencies, OwnedTx, PreAcceptedPhase, QueuedWork, RawTxHash,
};
use ckb_util::parking_lot::Mutex;
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Bound::{Excluded, Unbounded},
    sync::atomic::{AtomicUsize, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::authority) enum DependencyConsumerPhase {
    Accepted,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::authority) enum DependencyRelationTarget {
    AcceptedConsumer,
    OtherConsumer,
    Waiter,
}

impl DependencyRelationTarget {
    fn consumer(phase: DependencyConsumerPhase) -> Self {
        match phase {
            DependencyConsumerPhase::Accepted => Self::AcceptedConsumer,
            DependencyConsumerPhase::Other => Self::OtherConsumer,
        }
    }

    fn consumer_phase(self) -> Option<DependencyConsumerPhase> {
        match self {
            Self::AcceptedConsumer => Some(DependencyConsumerPhase::Accepted),
            Self::OtherConsumer => Some(DependencyConsumerPhase::Other),
            Self::Waiter => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DependencyRelationAction {
    Insert,
    Retire,
}

#[derive(Clone, Copy)]
struct DependencyRelationStageEffect {
    staged_delta: isize,
    stable_delta: isize,
    physical_delta: isize,
}

#[derive(Debug)]
struct StagedDependencyRelationState {
    action: DependencyRelationAction,
    visibility: StagedIngressVisibility,
}

impl StagedDependencyRelationState {
    fn new(action: DependencyRelationAction, visibility: StagedIngressVisibility) -> Self {
        Self { action, visibility }
    }
}

#[derive(Clone, Debug)]
enum DependencyRelationCell {
    Stable,
    Staged(std::sync::Arc<StagedDependencyRelationState>),
}

#[derive(Default)]
struct DependencyVisibilityReceipt {
    observed: Vec<(StagedIngressVisibility, bool)>,
}

impl DependencyVisibilityReceipt {
    fn observe(&mut self, state: &DependencyRelationCell) -> Result<bool, DependencyError> {
        match state {
            DependencyRelationCell::Stable => Ok(true),
            DependencyRelationCell::Staged(staged) => {
                self.observed
                    .try_reserve(1)
                    .map_err(|_| DependencyError::Allocation)?;
                let published = staged.visibility.is_visible();
                self.observed.push((staged.visibility.clone(), published));
                Ok(match staged.action {
                    DependencyRelationAction::Insert => published,
                    DependencyRelationAction::Retire => !published,
                })
            }
        }
    }

    fn is_current(&self) -> bool {
        self.observed
            .iter()
            .all(|(visibility, expected)| visibility.is_visible() == *expected)
    }

    fn observe_control<'value, T>(
        &mut self,
        cell: &'value DependencyControlCell<T>,
    ) -> Result<Option<&'value T>, DependencyError> {
        match cell {
            DependencyControlCell::Stable(value) => Ok(Some(value)),
            DependencyControlCell::Staged(staged) => {
                self.observed
                    .try_reserve(1)
                    .map_err(|_| DependencyError::Allocation)?;
                let published = staged.visibility.is_visible();
                self.observed.push((staged.visibility.clone(), published));
                Ok(if published {
                    staged.after.as_ref()
                } else {
                    staged.before.as_ref()
                })
            }
        }
    }
}

impl DependencyRelationCell {
    fn logical_is_visible(&self) -> bool {
        match self {
            Self::Stable => true,
            Self::Staged(staged) => match staged.action {
                DependencyRelationAction::Insert => staged.visibility.is_visible(),
                DependencyRelationAction::Retire => !staged.visibility.is_visible(),
            },
        }
    }

    /// A hidden insertion may become visible at its owner's publication cut,
    /// so an interleaved loss must retain maintenance evidence for it. A
    /// published retirement cannot become visible again and is excluded.
    fn may_become_visible(&self) -> bool {
        match self {
            Self::Stable => true,
            Self::Staged(staged) => match staged.action {
                DependencyRelationAction::Insert => true,
                DependencyRelationAction::Retire => !staged.visibility.is_visible(),
            },
        }
    }

    fn may_become_visible_for_stage(&self, visibility: &StagedIngressVisibility) -> bool {
        match self {
            Self::Stable => true,
            Self::Staged(staged) if staged.visibility.same_stage(visibility) => {
                staged.action == DependencyRelationAction::Insert
            }
            Self::Staged(_) => self.may_become_visible(),
        }
    }

    fn is_owned_stage(
        &self,
        action: DependencyRelationAction,
        visibility: &StagedIngressVisibility,
    ) -> bool {
        matches!(
            self,
            Self::Staged(staged)
                if staged.action == action && staged.visibility.same_stage(visibility)
        )
    }

    fn is_exact_stage(&self, staged: &std::sync::Arc<StagedDependencyRelationState>) -> bool {
        matches!(self, Self::Staged(current) if std::sync::Arc::ptr_eq(current, staged))
    }
}

#[derive(Clone)]
struct ObservedDependencyRelation {
    owner: RawTxHash,
    state: DependencyRelationCell,
    visible: bool,
}

#[derive(Clone, Copy)]
enum DependencyConsumerObservationKind {
    Accepted,
    General,
}

/// Opaque, bounded receipt for one dependency-consumer policy read. The
/// physical relation representation stays sealed in this module; consumers
/// may only extend the exact routed cut and revalidate this receipt.
pub(in crate::authority) struct ObservedDependencyConsumerRead {
    key: DependencyKey,
    kind: DependencyConsumerObservationKind,
    accepted: Vec<ObservedDependencyRelation>,
    other: Vec<ObservedDependencyRelation>,
    accepted_over_limit: Option<usize>,
}

pub(in crate::authority) enum ObservedAcceptedConsumers {
    Within {
        visible: Option<BTreeSet<RawTxHash>>,
        receipt: ObservedDependencyConsumerRead,
    },
    OverLimit(ObservedDependencyConsumerRead),
}

impl ObservedDependencyConsumerRead {
    pub(in crate::authority) fn extend_read_support(
        &self,
        entries: &ShardedOwnerMap,
        support: &mut ShardReadSupport,
    ) {
        support.insert(dependency_relation_shard(entries, &self.key));
    }

    pub(in crate::authority) fn is_fresh(
        &self,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
    ) -> bool {
        let shard = cut.projection_shard(dependency_relation_shard(entries, &self.key));
        let row = shard
            .dependency_relations
            .get(&self.key.origin())
            .and_then(|origin| origin.key(&self.key));
        let accepted = row.map(|row| &row.consumers.accepted);
        if let Some(limit) = self.accepted_over_limit {
            return accepted.is_some_and(|accepted| accepted.proves_visible_over_limit(limit));
        }
        if !DependencyRelationSet::matches_observation(accepted, &self.accepted) {
            return false;
        }
        match self.kind {
            DependencyConsumerObservationKind::Accepted => true,
            DependencyConsumerObservationKind::General => {
                let other = row.map(|row| &row.consumers.other);
                DependencyRelationSet::matches_observation(other, &self.other)
            }
        }
    }

    pub(in crate::authority) fn is_fresh_before_stage(
        &self,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
        visibility: &StagedIngressVisibility,
    ) -> bool {
        let shard = cut.projection_shard(dependency_relation_shard(entries, &self.key));
        let row = shard
            .dependency_relations
            .get(&self.key.origin())
            .and_then(|origin| origin.key(&self.key));
        let accepted = row.map(|row| &row.consumers.accepted);
        if let Some(limit) = self.accepted_over_limit {
            return accepted.is_some_and(|accepted| accepted.proves_visible_over_limit(limit));
        }
        if !DependencyRelationSet::matches_observation_before_stage(
            accepted,
            &self.accepted,
            visibility,
        ) {
            return false;
        }
        match self.kind {
            DependencyConsumerObservationKind::Accepted => true,
            DependencyConsumerObservationKind::General => {
                let other = row.map(|row| &row.consumers.other);
                DependencyRelationSet::matches_observation_before_stage(
                    other,
                    &self.other,
                    visibility,
                )
            }
        }
    }
}

/// One physical phase partition. The staged count is a checked, O(1) bound on
/// the only entries a logical point read may need to skip before finding a
/// stable member. It is derived and mutated with the entries under the same
/// shard lock; it owns no second relation fact.
#[derive(Clone, Debug, Default)]
pub(in crate::authority) struct DependencyRelationSet {
    entries: BTreeMap<RawTxHash, DependencyRelationCell>,
    staged: usize,
}

impl DependencyRelationSet {
    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn physical_len(&self) -> usize {
        self.entries.len()
    }

    fn staged_len(&self) -> usize {
        self.staged
    }

    fn proves_visible_over_limit(&self, limit: usize) -> bool {
        limit
            .checked_add(self.staged)
            .is_some_and(|physical_limit| self.entries.len() > physical_limit)
    }

    fn capture_bounded(
        &self,
        limit: usize,
    ) -> Result<(Vec<ObservedDependencyRelation>, Vec<RawTxHash>), DependencyError> {
        let physical_limit = limit
            .checked_add(self.staged)
            .ok_or(DependencyError::Projection)?;
        if self.entries.len() > physical_limit {
            return Err(DependencyError::Fanout);
        }
        let mut observed = Vec::new();
        observed
            .try_reserve_exact(self.entries.len())
            .map_err(|_| DependencyError::Allocation)?;
        let mut visible = Vec::new();
        visible
            .try_reserve_exact(self.entries.len().min(limit))
            .map_err(|_| DependencyError::Allocation)?;
        for (owner, state) in &self.entries {
            let logical = state.logical_is_visible();
            observed.push(ObservedDependencyRelation {
                owner: owner.clone(),
                state: state.clone(),
                visible: logical,
            });
            if logical {
                if visible.len() == limit {
                    return Err(DependencyError::Fanout);
                }
                visible.push(owner.clone());
            }
        }
        if observed
            .iter()
            .any(|expected| expected.state.logical_is_visible() != expected.visible)
        {
            return Err(DependencyError::Stale);
        }
        Ok((observed, visible))
    }

    fn matches_observation(
        current: Option<&Self>,
        expected: &[ObservedDependencyRelation],
    ) -> bool {
        let Some(current) = current else {
            return expected.is_empty();
        };
        current.entries.len() == expected.len()
            && current
                .entries
                .iter()
                .zip(expected)
                .all(|((owner, state), expected)| {
                    owner == &expected.owner
                        && match (state, &expected.state) {
                            (DependencyRelationCell::Stable, DependencyRelationCell::Stable) => {
                                true
                            }
                            (
                                DependencyRelationCell::Staged(current),
                                DependencyRelationCell::Staged(expected_cell),
                            ) => {
                                std::sync::Arc::ptr_eq(current, expected_cell)
                                    && state.logical_is_visible() == expected.visible
                            }
                            _ => false,
                        }
                })
    }

    /// Revalidate a policy receipt captured before one exact hidden stage was
    /// installed. Rows owned by that stage are interpreted at their pre-stage
    /// logical value; foreign staged rows retain exact Arc identity and
    /// visibility currentness.
    fn matches_observation_before_stage(
        current: Option<&Self>,
        expected: &[ObservedDependencyRelation],
        visibility: &StagedIngressVisibility,
    ) -> bool {
        let Some(current) = current else {
            return expected.is_empty();
        };
        let mut expected = expected.iter().peekable();
        for (owner, state) in &current.entries {
            let next_expected = expected.peek().copied();
            if next_expected.is_some_and(|expected| expected.owner < *owner) {
                return false;
            }
            match state {
                DependencyRelationCell::Staged(staged)
                    if staged.visibility.same_stage(visibility) =>
                {
                    match staged.action {
                        DependencyRelationAction::Insert => {
                            if next_expected.is_some_and(|expected| expected.owner == *owner) {
                                if next_expected.is_some_and(|expected| expected.visible) {
                                    return false;
                                }
                                expected.next();
                            }
                        }
                        DependencyRelationAction::Retire => {
                            if !next_expected.is_some_and(|expected| {
                                expected.owner == *owner && expected.visible
                            }) {
                                return false;
                            }
                            expected.next();
                        }
                    }
                }
                _ => {
                    let Some(expected_relation) = next_expected else {
                        return false;
                    };
                    let state_changed = match (state, &expected_relation.state) {
                        (DependencyRelationCell::Stable, DependencyRelationCell::Stable) => false,
                        (
                            DependencyRelationCell::Staged(current),
                            DependencyRelationCell::Staged(captured),
                        ) => {
                            !std::sync::Arc::ptr_eq(current, captured)
                                || state.logical_is_visible() != expected_relation.visible
                        }
                        _ => true,
                    };
                    if expected_relation.owner != *owner || state_changed {
                        return false;
                    }
                    expected.next();
                }
            }
        }
        expected.next().is_none()
    }

    #[cfg(test)]
    fn stable_insert(&mut self, owner: RawTxHash) -> bool {
        self.entries
            .insert(owner, DependencyRelationCell::Stable)
            .is_none()
    }

    #[cfg(test)]
    fn contains_physical(&self, owner: &RawTxHash) -> bool {
        self.entries.contains_key(owner)
    }

    fn contains_visible(&self, owner: &RawTxHash) -> bool {
        self.entries
            .get(owner)
            .is_some_and(|state| state.logical_is_visible())
    }

    #[cfg(test)]
    fn iter_visible(&self) -> impl Iterator<Item = &RawTxHash> {
        self.entries
            .iter()
            .filter_map(|(owner, state)| state.logical_is_visible().then_some(owner))
    }

    fn observe_contains_visible(
        &self,
        owner: &RawTxHash,
        receipt: &mut DependencyVisibilityReceipt,
    ) -> Result<bool, DependencyError> {
        self.entries
            .get(owner)
            .map_or(Ok(false), |state| receipt.observe(state))
    }

    fn extend_visible_bounded(
        &self,
        target: &mut BTreeSet<RawTxHash>,
        limit: usize,
    ) -> Result<(), DependencyError> {
        let visit_limit = limit
            .checked_add(self.staged)
            .and_then(|count| count.checked_add(1))
            .ok_or(DependencyError::Projection)?;
        for (owner, state) in self.entries.iter().take(visit_limit) {
            if state.logical_is_visible() {
                target.insert(owner.clone());
                if target.len() > limit {
                    return Err(DependencyError::Fanout);
                }
            }
        }
        if self.entries.len() > visit_limit {
            return Err(DependencyError::Fanout);
        }
        Ok(())
    }

    fn observe_has_visible(
        &self,
        receipt: &mut DependencyVisibilityReceipt,
    ) -> Result<bool, DependencyError> {
        let visit_limit = self
            .staged
            .checked_add(1)
            .ok_or(DependencyError::Projection)?;
        for state in self.entries.values().take(visit_limit) {
            if receipt.observe(state)? {
                return Ok(true);
            }
        }
        if self.entries.len() != self.staged {
            return Err(DependencyError::Projection);
        }
        Ok(false)
    }

    fn has_visible_bounded(&self) -> Result<bool, DependencyError> {
        let visit_limit = self
            .staged
            .checked_add(1)
            .ok_or(DependencyError::Projection)?;
        for state in self.entries.values().take(visit_limit) {
            if state.logical_is_visible() {
                return Ok(true);
            }
        }
        if self.entries.len() != self.staged {
            return Err(DependencyError::Projection);
        }
        Ok(false)
    }

    fn has_potential_visible_for_stage_bounded(
        &self,
        visibility: &StagedIngressVisibility,
    ) -> Result<bool, DependencyError> {
        let visit_limit = self
            .staged
            .checked_add(1)
            .ok_or(DependencyError::Projection)?;
        for state in self.entries.values().take(visit_limit) {
            if state.may_become_visible_for_stage(visibility) {
                return Ok(true);
            }
        }
        if self.entries.len() != self.staged {
            return Err(DependencyError::Projection);
        }
        Ok(false)
    }

    fn observe_first_visible_after(
        &self,
        cursor: Option<&RawTxHash>,
        receipt: &mut DependencyVisibilityReceipt,
    ) -> Result<Option<RawTxHash>, DependencyError> {
        let visit_limit = self
            .staged
            .checked_add(1)
            .ok_or(DependencyError::Projection)?;
        let mut visited = 0usize;
        let mut inspect = |owner: &RawTxHash,
                           state: &DependencyRelationCell|
         -> Result<Option<RawTxHash>, DependencyError> {
            visited = visited.checked_add(1).ok_or(DependencyError::Projection)?;
            if receipt.observe(state)? {
                Ok(Some(owner.clone()))
            } else {
                Ok(None)
            }
        };
        match cursor {
            Some(cursor) => {
                for (owner, state) in self
                    .entries
                    .range((Excluded(cursor), Unbounded))
                    .take(visit_limit)
                {
                    if let Some(owner) = inspect(owner, state)? {
                        return Ok(Some(owner));
                    }
                }
            }
            None => {
                for (owner, state) in self.entries.iter().take(visit_limit) {
                    if let Some(owner) = inspect(owner, state)? {
                        return Ok(Some(owner));
                    }
                }
            }
        }
        if visited > self.staged {
            return Err(DependencyError::Projection);
        }
        Ok(None)
    }

    fn first_visible_after_bounded(
        &self,
        cursor: Option<&RawTxHash>,
    ) -> Result<Option<RawTxHash>, DependencyError> {
        let visit_limit = self
            .staged
            .checked_add(1)
            .ok_or(DependencyError::Projection)?;
        let mut visited = 0usize;
        match cursor {
            Some(cursor) => {
                for (owner, state) in self
                    .entries
                    .range((Excluded(cursor), Unbounded))
                    .take(visit_limit)
                {
                    visited = visited.checked_add(1).ok_or(DependencyError::Projection)?;
                    if state.logical_is_visible() {
                        return Ok(Some(owner.clone()));
                    }
                }
            }
            None => {
                for (owner, state) in self.entries.iter().take(visit_limit) {
                    visited = visited.checked_add(1).ok_or(DependencyError::Projection)?;
                    if state.logical_is_visible() {
                        return Ok(Some(owner.clone()));
                    }
                }
            }
        }
        if visited > self.staged {
            return Err(DependencyError::Projection);
        }
        Ok(None)
    }

    #[cfg(test)]
    fn stage(
        &mut self,
        owner: RawTxHash,
        action: DependencyRelationAction,
        visibility: &StagedIngressVisibility,
    ) -> Result<(), DependencyStageError> {
        let staged_cell = std::sync::Arc::new(StagedDependencyRelationState::new(
            action,
            visibility.clone(),
        ));
        self.stage_exact(owner, action, staged_cell)
    }

    #[cfg(test)]
    fn stage_exact(
        &mut self,
        owner: RawTxHash,
        action: DependencyRelationAction,
        staged_cell: std::sync::Arc<StagedDependencyRelationState>,
    ) -> Result<(), DependencyStageError> {
        match self.entries.get_mut(&owner) {
            None if action == DependencyRelationAction::Insert => {
                let next = self
                    .staged
                    .checked_add(1)
                    .ok_or(DependencyStageError::Projection)?;
                self.entries
                    .insert(owner, DependencyRelationCell::Staged(staged_cell));
                self.staged = next;
                Ok(())
            }
            Some(current)
                if matches!(current, DependencyRelationCell::Stable)
                    && action == DependencyRelationAction::Retire =>
            {
                let next = self
                    .staged
                    .checked_add(1)
                    .ok_or(DependencyStageError::Projection)?;
                *current = DependencyRelationCell::Staged(staged_cell);
                self.staged = next;
                Ok(())
            }
            None | Some(_) => Err(DependencyStageError::Stale),
        }
    }

    fn stage_effect(
        &self,
        owner: &RawTxHash,
        action: DependencyRelationAction,
    ) -> Result<DependencyRelationStageEffect, DependencyStageError> {
        match self.entries.get(owner) {
            None if action == DependencyRelationAction::Insert => {
                Ok(DependencyRelationStageEffect {
                    staged_delta: 1,
                    stable_delta: 0,
                    physical_delta: 1,
                })
            }
            Some(current)
                if matches!(current, DependencyRelationCell::Stable)
                    && action == DependencyRelationAction::Retire =>
            {
                Ok(DependencyRelationStageEffect {
                    staged_delta: 1,
                    stable_delta: -1,
                    physical_delta: 0,
                })
            }
            None | Some(_) => Err(DependencyStageError::Stale),
        }
    }

    fn apply_stage_prechecked(
        &mut self,
        owner: RawTxHash,
        action: DependencyRelationAction,
        staged_cell: std::sync::Arc<StagedDependencyRelationState>,
    ) -> bool {
        match self.entries.get_mut(&owner) {
            None if action == DependencyRelationAction::Insert => {
                let Some(next) = self.staged.checked_add(1) else {
                    return false;
                };
                self.entries
                    .insert(owner, DependencyRelationCell::Staged(staged_cell));
                self.staged = next;
                true
            }
            Some(current)
                if matches!(current, DependencyRelationCell::Stable)
                    && action == DependencyRelationAction::Retire =>
            {
                let Some(next) = self.staged.checked_add(1) else {
                    return false;
                };
                *current = DependencyRelationCell::Staged(staged_cell);
                self.staged = next;
                true
            }
            None | Some(_) => false,
        }
    }

    fn owns_stage(
        &self,
        owner: &RawTxHash,
        action: DependencyRelationAction,
        visibility: &StagedIngressVisibility,
    ) -> bool {
        self.entries
            .get(owner)
            .is_some_and(|state| state.is_owned_stage(action, visibility))
    }

    fn owns_exact_stage(
        &self,
        owner: &RawTxHash,
        staged: &std::sync::Arc<StagedDependencyRelationState>,
    ) -> bool {
        self.entries
            .get(owner)
            .is_some_and(|cell| cell.is_exact_stage(staged))
    }

    fn finish_owned_stage(
        &mut self,
        owner: &RawTxHash,
        action: DependencyRelationAction,
        visibility: &StagedIngressVisibility,
    ) -> Result<bool, DependencyStageError> {
        if !self.owns_stage(owner, action, visibility) {
            return Ok(false);
        }
        let next_staged = self
            .staged
            .checked_sub(1)
            .ok_or(DependencyStageError::Projection)?;
        let committed = visibility.is_visible();
        match (action, committed) {
            (DependencyRelationAction::Insert, true)
            | (DependencyRelationAction::Retire, false) => {
                if let Some(state) = self.entries.get_mut(owner) {
                    *state = DependencyRelationCell::Stable;
                }
            }
            (DependencyRelationAction::Insert, false)
            | (DependencyRelationAction::Retire, true) => {
                self.entries.remove(owner);
            }
        }
        self.staged = next_staged;
        Ok(true)
    }

    /// Normalize only the exact immutable staged cell installed by one
    /// capability. Every staged point is exclusive until this synchronous
    /// cleanup; pointer inequality is therefore a structural fault.
    fn finish_exact_stage(
        &mut self,
        owner: &RawTxHash,
        staged: &std::sync::Arc<StagedDependencyRelationState>,
    ) -> Result<bool, DependencyStageError> {
        if !self.owns_exact_stage(owner, staged) {
            return Ok(false);
        }
        let next_staged = self
            .staged
            .checked_sub(1)
            .ok_or(DependencyStageError::Projection)?;
        match (staged.action, staged.visibility.is_visible()) {
            (DependencyRelationAction::Insert, true)
            | (DependencyRelationAction::Retire, false) => {
                if let Some(cell) = self.entries.get_mut(owner) {
                    *cell = DependencyRelationCell::Stable;
                }
            }
            (DependencyRelationAction::Insert, false)
            | (DependencyRelationAction::Retire, true) => {
                self.entries.remove(owner);
            }
        }
        self.staged = next_staged;
        Ok(true)
    }
}

#[derive(Clone, Debug, Default)]
pub(in crate::authority) struct DependencyConsumerRow {
    accepted: DependencyRelationSet,
    other: DependencyRelationSet,
}

#[derive(Clone, Debug, Default)]
struct DependencyKeyRelationRow {
    consumers: DependencyConsumerRow,
    waiters: DependencyRelationSet,
}

enum DependencyRelationFinish {
    Foreign,
    Finished,
}

impl DependencyKeyRelationRow {
    fn is_empty(&self) -> bool {
        self.consumers.is_empty() && self.waiters.is_empty()
    }
}

/// One authority for every relation whose dependency key has this origin.
/// A key owns its Accepted, Other and sparse Waiter targets directly; the
/// bounded transitional directory accelerates logical origin reads but owns
/// no relation state.
#[derive(Clone, Debug, Default)]
pub(in crate::authority) struct DependencyOriginRow {
    keys: BTreeMap<DependencyKey, DependencyKeyRelationRow>,
    transitional: usize,
}

impl DependencyOriginRow {
    fn physical_len(&self) -> usize {
        self.keys.len()
    }

    fn key(&self, key: &DependencyKey) -> Option<&DependencyKeyRelationRow> {
        self.keys.get(key)
    }

    fn key_mut_or_default(&mut self, key: DependencyKey) -> &mut DependencyKeyRelationRow {
        self.keys.entry(key).or_default()
    }

    #[cfg(test)]
    fn stable_insert(
        &mut self,
        key: DependencyKey,
        target: DependencyRelationTarget,
        owner: RawTxHash,
    ) -> bool {
        let row = self.key_mut_or_default(key);
        match target.consumer_phase() {
            Some(phase) => row.consumers.members_mut(phase).stable_insert(owner),
            None => row.waiters.stable_insert(owner),
        }
    }

    fn finish_exact_relation(
        &mut self,
        relation: &StagedDependencyRelation,
    ) -> Result<DependencyRelationFinish, DependencyStageError> {
        let Some(staged) = relation.staged_cell.as_ref() else {
            return Err(DependencyStageError::Projection);
        };
        let Some(row) = self.keys.get_mut(&relation.point.key) else {
            return Ok(DependencyRelationFinish::Foreign);
        };
        let set = match relation.point.target.consumer_phase() {
            Some(phase) => row.consumers.members_mut(phase),
            None => &mut row.waiters,
        };
        if !set.finish_exact_stage(&relation.point.owner, staged)? {
            return Ok(DependencyRelationFinish::Foreign);
        }
        if row.is_empty() {
            self.keys.remove(&relation.point.key);
        }
        Ok(DependencyRelationFinish::Finished)
    }

    fn finish_owned_relation(
        &mut self,
        relation: &StagedDependencyRelation,
        visibility: &StagedIngressVisibility,
    ) -> Result<DependencyRelationFinish, DependencyStageError> {
        let Some(row) = self.keys.get_mut(&relation.point.key) else {
            return Ok(DependencyRelationFinish::Foreign);
        };
        let set = match relation.point.target.consumer_phase() {
            Some(phase) => row.consumers.members_mut(phase),
            None => &mut row.waiters,
        };
        if !set.finish_owned_stage(&relation.point.owner, relation.action, visibility)? {
            return Ok(DependencyRelationFinish::Foreign);
        }
        if row.is_empty() {
            self.keys.remove(&relation.point.key);
        }
        Ok(DependencyRelationFinish::Finished)
    }

    fn key_is_transitional(&self, key: &DependencyKey) -> Result<bool, DependencyError> {
        let transitional = self.keys.get(key).is_some_and(|row| {
            row.consumers
                .physical_len()
                .is_some_and(|physical| physical != 0 && row.consumers.stable_len() == Some(0))
        });
        Ok(transitional)
    }

    fn transitional_len(&self) -> usize {
        self.transitional
    }

    fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

fn dependency_origin_shard(entries: &ShardedOwnerMap, origin: &DependencyOrigin) -> usize {
    entries.layout.router.shard(b"dependency/relation", origin)
}

fn dependency_relation_shard(entries: &ShardedOwnerMap, key: &DependencyKey) -> usize {
    dependency_origin_shard(entries, &key.origin())
}

impl DependencyConsumerRow {
    fn members(&self, phase: DependencyConsumerPhase) -> &DependencyRelationSet {
        match phase {
            DependencyConsumerPhase::Accepted => &self.accepted,
            DependencyConsumerPhase::Other => &self.other,
        }
    }

    fn members_mut(&mut self, phase: DependencyConsumerPhase) -> &mut DependencyRelationSet {
        match phase {
            DependencyConsumerPhase::Accepted => &mut self.accepted,
            DependencyConsumerPhase::Other => &mut self.other,
        }
    }

    fn is_empty(&self) -> bool {
        self.accepted.is_empty() && self.other.is_empty()
    }

    fn physical_len(&self) -> Option<usize> {
        self.accepted
            .physical_len()
            .checked_add(self.other.physical_len())
    }

    fn stable_len(&self) -> Option<usize> {
        self.accepted
            .physical_len()
            .checked_sub(self.accepted.staged_len())?
            .checked_add(
                self.other
                    .physical_len()
                    .checked_sub(self.other.staged_len())?,
            )
    }

    #[cfg(test)]
    fn iter_visible(&self) -> impl Iterator<Item = &RawTxHash> {
        self.accepted
            .iter_visible()
            .chain(self.other.iter_visible())
    }

    fn visible_members_bounded(
        &self,
        limit: usize,
    ) -> Result<Option<BTreeSet<RawTxHash>>, DependencyError> {
        let mut visible = BTreeSet::new();
        self.accepted.extend_visible_bounded(&mut visible, limit)?;
        self.other.extend_visible_bounded(&mut visible, limit)?;
        Ok((!visible.is_empty()).then_some(visible))
    }

    fn visible_phase(
        &self,
        owner: &RawTxHash,
    ) -> Result<Option<DependencyConsumerPhase>, DependencyError> {
        match (
            self.accepted.contains_visible(owner),
            self.other.contains_visible(owner),
        ) {
            (true, false) => Ok(Some(DependencyConsumerPhase::Accepted)),
            (false, true) => Ok(Some(DependencyConsumerPhase::Other)),
            (false, false) => Ok(None),
            (true, true) => Err(DependencyError::Projection),
        }
    }

    fn observe_visible_phase(
        &self,
        owner: &RawTxHash,
        receipt: &mut DependencyVisibilityReceipt,
    ) -> Result<Option<DependencyConsumerPhase>, DependencyError> {
        match (
            self.accepted.observe_contains_visible(owner, receipt)?,
            self.other.observe_contains_visible(owner, receipt)?,
        ) {
            (true, false) => Ok(Some(DependencyConsumerPhase::Accepted)),
            (false, true) => Ok(Some(DependencyConsumerPhase::Other)),
            (false, false) => Ok(None),
            (true, true) => Err(DependencyError::Projection),
        }
    }

    fn observe_has_visible(
        &self,
        receipt: &mut DependencyVisibilityReceipt,
    ) -> Result<bool, DependencyError> {
        Ok(self.accepted.observe_has_visible(receipt)?
            || self.other.observe_has_visible(receipt)?)
    }

    fn has_visible_bounded(&self) -> Result<bool, DependencyError> {
        Ok(self.accepted.has_visible_bounded()? || self.other.has_visible_bounded()?)
    }

    fn has_potential_visible_for_stage_bounded(
        &self,
        visibility: &StagedIngressVisibility,
    ) -> Result<bool, DependencyError> {
        Ok(self
            .accepted
            .has_potential_visible_for_stage_bounded(visibility)?
            || self
                .other
                .has_potential_visible_for_stage_bounded(visibility)?)
    }

    fn observe_first_visible_after(
        &self,
        cursor: Option<&RawTxHash>,
        receipt: &mut DependencyVisibilityReceipt,
    ) -> Result<Option<RawTxHash>, DependencyError> {
        let accepted = self.accepted.observe_first_visible_after(cursor, receipt)?;
        let other = self.other.observe_first_visible_after(cursor, receipt)?;
        Ok(match (accepted, other) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, right) => left.or(right),
        })
    }

    fn first_visible_after_bounded(
        &self,
        cursor: Option<&RawTxHash>,
    ) -> Result<Option<RawTxHash>, DependencyError> {
        let accepted = self.accepted.first_visible_after_bounded(cursor)?;
        let other = self.other.first_visible_after_bounded(cursor)?;
        Ok(match (accepted, other) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, right) => left.or(right),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) struct DependencyLevel {
    last_change: DependencyCut,
    last_definitive_loss: Option<DependencyCut>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::authority) struct UnindexedDependencyLevel {
    last_change: Option<DependencyCut>,
    last_definitive_loss: Option<DependencyCut>,
}

#[derive(Debug)]
pub(in crate::authority) struct StagedDependencyControlState<T> {
    before: Option<T>,
    after: Option<T>,
    visibility: StagedIngressVisibility,
}

#[derive(Clone, Debug)]
pub(in crate::authority) enum DependencyControlCell<T> {
    Stable(T),
    Staged(std::sync::Arc<StagedDependencyControlState<T>>),
}

impl<T> DependencyControlCell<T> {
    pub(in crate::authority) fn logical(&self) -> Option<&T> {
        match self {
            Self::Stable(value) => Some(value),
            Self::Staged(staged) => {
                if staged.visibility.is_visible() {
                    staged.after.as_ref()
                } else {
                    staged.before.as_ref()
                }
            }
        }
    }

    fn is_exact_stage(&self, staged: &std::sync::Arc<StagedDependencyControlState<T>>) -> bool {
        matches!(self, Self::Staged(current) if std::sync::Arc::ptr_eq(current, staged))
    }

    fn into_stable(self) -> Option<T> {
        match self {
            Self::Stable(value) => Some(value),
            Self::Staged(_) => None,
        }
    }
}

impl<T: Clone> DependencyControlCell<T> {
    pub(in crate::authority) fn logical_cloned(&self) -> Option<T> {
        self.logical().cloned()
    }
}

struct StagedDependencyControl<T> {
    key: DependencyKey,
    staged_cell: std::sync::Arc<StagedDependencyControlState<T>>,
    stable_before: Option<T>,
    stable_after: Option<T>,
}

impl<T: Clone> StagedDependencyControl<T> {
    fn new(
        key: DependencyKey,
        before: Option<T>,
        after: Option<T>,
        visibility: &StagedIngressVisibility,
    ) -> Self {
        Self {
            key,
            staged_cell: std::sync::Arc::new(StagedDependencyControlState {
                before: before.clone(),
                after: after.clone(),
                visibility: visibility.clone(),
            }),
            stable_before: before,
            stable_after: after,
        }
    }

    fn normalized(&mut self) -> Option<T> {
        if self.staged_cell.visibility.is_visible() {
            self.stable_after.take()
        } else {
            self.stable_before.take()
        }
    }

    fn normalized_is_some(&self) -> bool {
        if self.staged_cell.visibility.is_visible() {
            self.stable_after.is_some()
        } else {
            self.stable_before.is_some()
        }
    }
}

fn control_cell_matches_before<T: PartialEq>(
    rows: &BTreeMap<DependencyKey, DependencyControlCell<T>>,
    staged: &StagedDependencyControl<T>,
) -> bool {
    match rows.get(&staged.key) {
        Some(DependencyControlCell::Stable(current)) => {
            Some(current) == staged.staged_cell.before.as_ref()
        }
        None => staged.staged_cell.before.is_none(),
        Some(DependencyControlCell::Staged(_)) => false,
    }
}

fn install_control_cell_prechecked<T>(
    rows: &mut BTreeMap<DependencyKey, DependencyControlCell<T>>,
    staged: &StagedDependencyControl<T>,
) {
    let cell = DependencyControlCell::Staged(std::sync::Arc::clone(&staged.staged_cell));
    if let Some(current) = rows.get_mut(&staged.key) {
        *current = cell;
    } else {
        rows.insert(staged.key.clone(), cell);
    }
}

enum DependencyControlCleanupStatus {
    Absent,
    Stable,
    OwnedSome,
    OwnedNone,
    Foreign,
}

fn dependency_control_cleanup_status<T: Clone>(
    rows: &BTreeMap<DependencyKey, DependencyControlCell<T>>,
    staged: &[StagedDependencyControl<T>],
    key: &DependencyKey,
) -> DependencyControlCleanupStatus {
    match rows.get(key) {
        None => DependencyControlCleanupStatus::Absent,
        Some(DependencyControlCell::Stable(_)) => DependencyControlCleanupStatus::Stable,
        Some(DependencyControlCell::Staged(current)) => staged
            .binary_search_by(|candidate| candidate.key.cmp(key))
            .ok()
            .and_then(|position| staged.get(position))
            .filter(|owned| std::sync::Arc::ptr_eq(current, &owned.staged_cell))
            .map_or(DependencyControlCleanupStatus::Foreign, |owned| {
                if owned.normalized_is_some() {
                    DependencyControlCleanupStatus::OwnedSome
                } else {
                    DependencyControlCleanupStatus::OwnedNone
                }
            }),
    }
}

#[expect(
    clippy::expect_used,
    reason = "same-cut cleanup preflight validates exact Arc ownership before the trusted no-allocation suffix"
)]
fn finish_exact_control_stage_prechecked<T: Clone>(
    rows: &mut BTreeMap<DependencyKey, DependencyControlCell<T>>,
    staged: &mut StagedDependencyControl<T>,
) {
    let normalized = staged.normalized();
    match normalized {
        Some(value) => {
            let current = rows
                .get_mut(&staged.key)
                .expect("same-cut preflight owns this exact staged control cell");
            debug_assert!(current.is_exact_stage(&staged.staged_cell));
            *current = DependencyControlCell::Stable(value);
        }
        None => {
            let current = rows
                .remove(&staged.key)
                .expect("same-cut preflight owns this exact staged control cell");
            debug_assert!(current.is_exact_stage(&staged.staged_cell));
        }
    }
}

#[cfg(test)]
fn insert_stable_control<T>(
    rows: &mut BTreeMap<DependencyKey, DependencyControlCell<T>>,
    key: DependencyKey,
    value: T,
) -> Result<Option<T>, ()> {
    match rows.get_mut(&key) {
        Some(DependencyControlCell::Stable(current)) => Ok(Some(std::mem::replace(current, value))),
        Some(DependencyControlCell::Staged(_)) => Err(()),
        None => {
            rows.insert(key, DependencyControlCell::Stable(value));
            Ok(None)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DirtyScope {
    ExistingWaiters,
    AllConsumers,
}

impl DirtyScope {
    fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::AllConsumers, Self::AllConsumers | Self::ExistingWaiters)
            | (Self::ExistingWaiters, Self::AllConsumers) => Self::AllConsumers,
            (Self::ExistingWaiters, Self::ExistingWaiters) => Self::ExistingWaiters,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) struct DirtyDependency {
    target: DependencyCut,
    scope: DirtyScope,
    cursor: Option<RawTxHash>,
    pending: Option<PendingDependency>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingDependency {
    target: DependencyCut,
    scope: DirtyScope,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DependencySlot {
    hash: RawTxHash,
    phase: DependencyConsumerPhase,
    dependencies: KnownDependencies,
    waiting: Option<ObservedDependencies>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DependencyError {
    Projection,
    Allocation,
    Stale,
    Fanout,
    SurvivingAcceptedConsumer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DependencyMaintenanceAction {
    Advance,
    Requeue,
}

struct DependencyEventChange {
    key: DependencyKey,
    expected_level: Option<DependencyLevel>,
    level: DependencyLevel,
    scope: DirtyScope,
}

struct DependencyOriginExpectation {
    origin: DependencyOrigin,
    keys: Option<BTreeSet<DependencyKey>>,
}

pub(super) struct DependencyEventPlan {
    changes: Vec<DependencyEventChange>,
    origins: Vec<DependencyOriginExpectation>,
}

enum DependencyMaintenanceStep {
    Advance {
        key: DependencyKey,
        expected: DirtyDependency,
        cursor: RawTxHash,
    },
    Complete {
        key: DependencyKey,
        expected: DirtyDependency,
    },
}

#[must_use = "a dependency maintenance successor must be carried by one authority Apply"]
pub(super) struct DependencyMaintenancePlan {
    step: DependencyMaintenanceStep,
}

#[derive(Clone, Debug)]
pub(super) struct DependencyMaintenanceTicket {
    key: DependencyKey,
    hash: Option<RawTxHash>,
    target: DependencyCut,
    scope: DirtyScope,
    last_definitive_loss: Option<DependencyCut>,
    expected: DirtyDependency,
}

#[derive(Default)]
pub(super) enum DependencyControlDelta {
    #[default]
    None,
    Event(DependencyEventPlan),
    Maintenance(DependencyMaintenancePlan),
}

#[derive(Default)]
pub(super) enum DependencyEntryControlDelta {
    #[default]
    None,
    Event(DependencyEventPlan),
}

impl From<DependencyEntryControlDelta> for DependencyControlDelta {
    fn from(control: DependencyEntryControlDelta) -> Self {
        match control {
            DependencyEntryControlDelta::None => Self::None,
            DependencyEntryControlDelta::Event(event) => Self::Event(event),
        }
    }
}

pub(super) struct DependencyDelta {
    before: Option<DependencySlot>,
    after: Option<DependencySlot>,
    observed: Option<DependencySlot>,
    control: DependencyEntryControlDelta,
}

#[derive(Default)]
pub(super) struct DependencyBatchDelta {
    removed: Vec<DependencySlot>,
    added: Vec<DependencySlot>,
    observed: Vec<DependencySlot>,
    unchanged: Vec<DependencySlot>,
    relation_changes: Vec<StagedDependencyRelation>,
    settlement_evidence: Vec<SettlementDependencyEvidence>,
    control: DependencyControlDelta,
    prestate: DependencyBatchPrestate,
}

#[derive(Default)]
struct DependencyBatchPrestate {
    relations: Vec<DependencyRelationPointPrestate>,
    keys: Vec<DependencyKeyPrestate>,
    owner_origins: Vec<DependencyOriginAnyPrestate>,
    unindexed: Vec<(usize, UnindexedDependencyLevel)>,
}

pub(super) struct SettlementDependencyEvidence {
    owner: RawTxHash,
    keys: Vec<SettlementDependencyKeyEvidence>,
}

struct SettlementDependencyKeyEvidence {
    key: DependencyKey,
    level: Option<DependencyLevel>,
    dirty: Option<DirtyDependency>,
    unindexed: UnindexedDependencyLevel,
    owner_phase: Option<DependencyConsumerPhase>,
}

enum SettlementDependencyEndpoint<'slot> {
    Retained(&'slot DependencySlot),
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct DependencyRelationPoint {
    key: DependencyKey,
    target: DependencyRelationTarget,
    owner: RawTxHash,
}

struct DependencyRelationPointPrestate {
    point: DependencyRelationPoint,
    visible: bool,
}

enum DependencyControlKeyPrestate {
    Event {
        // Accepted consumers participate in the administrative closure and
        // therefore remain a Plan-time semantic precondition. Other
        // consumers and waiters are operational fanout: the staged binder
        // rebases them from one exact cut and carries the required absence
        // facts to the final Apply cut.
        has_accepted_consumers: bool,
    },
    Maintenance {
        scope: DirtyScope,
        next: Option<RawTxHash>,
    },
}

struct DependencyKeyPrestate {
    key: DependencyKey,
    level: Option<DependencyLevel>,
    dirty: Option<DirtyDependency>,
    control: Option<DependencyControlKeyPrestate>,
}

struct DependencyOriginAnyPrestate {
    origin: DependencyOrigin,
    any: bool,
}

impl DependencyBatchPrestate {
    fn capture(
        frontier: &DependencyFrontier,
        delta: &DependencyBatchDelta,
    ) -> Result<Self, DependencyError> {
        let entries = &frontier.entries;
        let mut keys = Vec::new();
        let mut relations = Vec::new();
        let slot_count = delta
            .removed
            .len()
            .checked_add(delta.added.len())
            .and_then(|count| count.checked_add(delta.observed.len()))
            .ok_or(DependencyError::Projection)?;
        keys.try_reserve(slot_count)
            .map_err(|_| DependencyError::Allocation)?;
        for slot in delta
            .removed
            .iter()
            .chain(&delta.added)
            .chain(&delta.observed)
        {
            keys.try_reserve(slot.dependencies.len())
                .map_err(|_| DependencyError::Allocation)?;
            keys.extend(slot.dependencies.keys().iter().cloned());
            relations
                .try_reserve(slot.dependencies.len())
                .map_err(|_| DependencyError::Allocation)?;
            relations.extend(slot.dependencies.keys().iter().cloned().map(|key| {
                DependencyRelationPoint {
                    key,
                    target: DependencyRelationTarget::consumer(slot.phase),
                    owner: slot.hash.clone(),
                }
            }));
            if let Some(waiting) = &slot.waiting {
                keys.try_reserve(waiting.keys().len())
                    .map_err(|_| DependencyError::Allocation)?;
                keys.extend(waiting.keys().cloned());
                relations
                    .try_reserve(waiting.keys().len())
                    .map_err(|_| DependencyError::Allocation)?;
                relations.extend(waiting.keys().cloned().map(|key| DependencyRelationPoint {
                    key,
                    target: DependencyRelationTarget::Waiter,
                    owner: slot.hash.clone(),
                }));
            }
        }
        relations.sort_unstable();
        relations.dedup();
        if let DependencyControlDelta::Event(event) = &delta.control {
            keys.try_reserve(event.changes.len())
                .map_err(|_| DependencyError::Allocation)?;
            keys.extend(event.changes.iter().map(|change| change.key.clone()));
        } else if let DependencyControlDelta::Maintenance(maintenance) = &delta.control {
            keys.try_reserve_exact(1)
                .map_err(|_| DependencyError::Allocation)?;
            keys.push(maintenance.key().clone());
        }
        keys.sort_unstable();
        keys.dedup();

        let mut owner_origins = Vec::new();
        owner_origins
            .try_reserve_exact(slot_count)
            .map_err(|_| DependencyError::Allocation)?;
        owner_origins.extend(
            delta
                .removed
                .iter()
                .chain(&delta.added)
                .chain(&delta.observed)
                .map(|slot| DependencyOrigin::Transaction(slot.hash.clone())),
        );
        owner_origins.sort_unstable();
        owner_origins.dedup();

        let mut read_support = ShardReadSupport::default();
        for key in &keys {
            read_support.insert(dependency_relation_shard(entries, key));
            read_support.insert(entries.layout.router.shard(b"dependency/level", key));
            read_support.insert(entries.layout.router.shard(b"dependency/unindexed", key));
        }
        for origin in &owner_origins {
            read_support.insert(dependency_origin_shard(entries, origin));
        }
        if let DependencyControlDelta::Event(event) = &delta.control {
            for expected in &event.origins {
                read_support.insert(dependency_origin_shard(entries, &expected.origin));
            }
        }
        let cut = entries.mixed_cut(read_support, ShardWriteSupport::default());
        let mut visibility = DependencyVisibilityReceipt::default();

        let mut relation_witnesses = Vec::new();
        relation_witnesses
            .try_reserve_exact(relations.len())
            .map_err(|_| DependencyError::Allocation)?;
        for point in relations {
            let visible = dependency_relation_point_is_visible_in_cut(
                entries,
                &cut,
                &point,
                Some(&mut visibility),
            )?;
            relation_witnesses.push(DependencyRelationPointPrestate { point, visible });
        }

        let mut key_witnesses = Vec::new();
        key_witnesses
            .try_reserve_exact(keys.len())
            .map_err(|_| DependencyError::Allocation)?;
        let mut unindexed_shards = Vec::new();
        unindexed_shards
            .try_reserve_exact(keys.len())
            .map_err(|_| DependencyError::Allocation)?;

        for key in keys {
            let level_shard = entries.layout.router.shard(b"dependency/level", &key);
            let level_row = cut.projection_shard(level_shard);
            let level = match level_row.dependency_levels.get(&key) {
                Some(cell) => visibility.observe_control(cell)?.copied(),
                None => None,
            };
            let dirty = match level_row.dependency_dirty.get(&key) {
                Some(cell) => visibility.observe_control(cell)?.cloned(),
                None => None,
            };
            unindexed_shards.push(entries.layout.router.shard(b"dependency/unindexed", &key));
            let control = match &delta.control {
                DependencyControlDelta::Event(event) => event
                    .changes
                    .iter()
                    .find(|change| change.key == key)
                    .map(|_| {
                        let has_accepted_consumers =
                            dependency_has_visible_accepted_consumers_observed_in_cut(
                                entries,
                                &cut,
                                &key,
                                &mut visibility,
                            )?;
                        Ok(DependencyControlKeyPrestate::Event {
                            has_accepted_consumers,
                        })
                    })
                    .transpose()?,
                DependencyControlDelta::Maintenance(maintenance) if maintenance.key() == &key => {
                    Some(DependencyControlKeyPrestate::Maintenance {
                        scope: maintenance.expected().scope,
                        next: dependency_next_visible_owner_observed_in_cut(
                            entries,
                            &cut,
                            &key,
                            maintenance.expected().scope,
                            maintenance.expected().cursor.as_ref(),
                            &mut visibility,
                        )?,
                    })
                }
                DependencyControlDelta::None | DependencyControlDelta::Maintenance(_) => None,
            };
            key_witnesses.push(DependencyKeyPrestate {
                key,
                level,
                dirty,
                control,
            });
        }

        let mut owner_origin_witnesses = Vec::new();
        owner_origin_witnesses
            .try_reserve_exact(owner_origins.len())
            .map_err(|_| DependencyError::Allocation)?;
        for origin in owner_origins {
            let any = dependency_origin_has_visible_key_observed_in_cut(
                entries,
                &cut,
                &origin,
                &mut visibility,
            )?;
            owner_origin_witnesses.push(DependencyOriginAnyPrestate { origin, any });
        }

        unindexed_shards.sort_unstable();
        unindexed_shards.dedup();
        let mut unindexed = Vec::new();
        unindexed
            .try_reserve_exact(unindexed_shards.len())
            .map_err(|_| DependencyError::Allocation)?;
        for shard in unindexed_shards {
            let level = cut.projection_shard(shard).dependency_unindexed;
            unindexed.push((shard, level));
        }

        if let DependencyControlDelta::Event(event) = &delta.control {
            for expected in &event.origins {
                if !dependency_origin_matches_observed_in_cut(
                    entries,
                    &cut,
                    &expected.origin,
                    expected.keys.as_ref(),
                    &mut visibility,
                )? {
                    return Err(DependencyError::Stale);
                }
            }
        }
        if !visibility.is_current() {
            return Err(DependencyError::Stale);
        }

        if let DependencyControlDelta::Event(event) = &delta.control {
            for change in &event.changes {
                let position = key_witnesses
                    .binary_search_by(|candidate| candidate.key.cmp(&change.key))
                    .map_err(|_| DependencyError::Projection)?;
                if key_witnesses
                    .get(position)
                    .is_none_or(|observed| observed.level != change.expected_level)
                {
                    return Err(DependencyError::Stale);
                }
            }
        } else if let DependencyControlDelta::Maintenance(maintenance) = &delta.control {
            let position = key_witnesses
                .binary_search_by(|candidate| candidate.key.cmp(maintenance.key()))
                .map_err(|_| DependencyError::Projection)?;
            if key_witnesses
                .get(position)
                .is_none_or(|observed| observed.dirty.as_ref() != Some(maintenance.expected()))
            {
                return Err(DependencyError::Stale);
            }
        }

        Ok(Self {
            relations: relation_witnesses,
            keys: key_witnesses,
            owner_origins: owner_origin_witnesses,
            unindexed,
        })
    }

    fn semantic_rows_are_fresh(
        &self,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
        control: &DependencyControlDelta,
    ) -> bool {
        self.relations.iter().all(|expected| {
            dependency_relation_point_is_visible_in_cut(entries, cut, &expected.point, None)
                .is_ok_and(|visible| visible == expected.visible)
        }) && self.keys.iter().all(|expected| {
            let level_shard = entries
                .layout
                .router
                .shard(b"dependency/level", &expected.key);
            let level_row = cut.projection_shard(level_shard);
            level_row
                .dependency_levels
                .get(&expected.key)
                .and_then(DependencyControlCell::logical)
                .copied()
                == expected.level
                && level_row
                    .dependency_dirty
                    .get(&expected.key)
                    .and_then(DependencyControlCell::logical)
                    == expected.dirty.as_ref()
                && dependency_control_key_prestate_is_fresh(entries, cut, expected)
        }) && dependency_event_origins_are_fresh(entries, cut, control)
            && self.unindexed.iter().all(|(shard, expected)| {
                cut.projection_shard(*shard).dependency_unindexed == *expected
            })
    }

    fn ready_phase_rows_are_fresh_after_stage(
        &self,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
    ) -> bool {
        self.keys.iter().all(|expected| {
            let level_shard = entries
                .layout
                .router
                .shard(b"dependency/level", &expected.key);
            let level_row = cut.projection_shard(level_shard);
            level_row
                .dependency_levels
                .get(&expected.key)
                .and_then(DependencyControlCell::logical)
                .copied()
                == expected.level
                && level_row
                    .dependency_dirty
                    .get(&expected.key)
                    .and_then(DependencyControlCell::logical)
                    == expected.dirty.as_ref()
                && expected.control.is_none()
        }) && self
            .unindexed
            .iter()
            .all(|(shard, expected)| cut.projection_shard(*shard).dependency_unindexed == *expected)
    }

    /// Event publication retains only the semantic facts that selected its
    /// control transition. Exact relation edges were already installed by the
    /// stage cut and are owned by the move-only staged carrier; unrelated
    /// edges on the same key must remain free to commute. Accepted closure,
    /// explicit origin expectations, level/dirty state and unindexed loss
    /// evidence are still revalidated here because they can change the Event
    /// branch itself.
    fn event_rows_are_fresh_after_stage(
        &self,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
        control: &DependencyControlDelta,
        visibility: &StagedIngressVisibility,
    ) -> bool {
        if !matches!(control, DependencyControlDelta::Event(_)) {
            return false;
        }
        self.keys.iter().all(|expected| {
            let level_shard = entries
                .layout
                .router
                .shard(b"dependency/level", &expected.key);
            let level_row = cut.projection_shard(level_shard);
            level_row
                .dependency_levels
                .get(&expected.key)
                .and_then(DependencyControlCell::logical)
                .copied()
                == expected.level
                && level_row
                    .dependency_dirty
                    .get(&expected.key)
                    .and_then(DependencyControlCell::logical)
                    == expected.dirty.as_ref()
                && dependency_control_key_prestate_is_fresh(entries, cut, expected)
        }) && dependency_event_origins_are_fresh(entries, cut, control)
            && self.unindexed.iter().all(|(shard, expected)| {
                cut.projection_shard(*shard).dependency_unindexed == *expected
            })
            && self.owner_origins.iter().all(|expected| {
                dependency_origin_has_potential_visible_for_stage_in_cut(
                    entries,
                    cut,
                    &expected.origin,
                    visibility,
                )
                .is_ok_and(|any| any == expected.any)
            })
    }

    fn owner_origins_are_fresh(
        &self,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
    ) -> bool {
        self.owner_origins.iter().all(|expected| {
            dependency_origin_has_visible_key_in_cut(entries, cut, &expected.origin)
                .is_ok_and(|any| any == expected.any)
        })
    }

    fn is_fresh(
        &self,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
        control: &DependencyControlDelta,
    ) -> bool {
        self.semantic_rows_are_fresh(entries, cut, control)
            && self.owner_origins_are_fresh(entries, cut)
    }

    /// Other-consumer/waiter growth is a legal Event rebase before Bind. The
    /// exact Bind cut replaces only that occupancy witness; Accepted closure,
    /// explicit origin sets, levels, dirty rows, unindexed evidence and all
    /// Maintenance OCC remain Plan-bound.
    fn is_fresh_before_event_rebase(
        &self,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
        control: &DependencyControlDelta,
    ) -> bool {
        self.semantic_rows_are_fresh(entries, cut, control)
            && (matches!(control, DependencyControlDelta::Event(_))
                || self.owner_origins_are_fresh(entries, cut))
    }

    fn rebase_event_owner_origins(
        &mut self,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
        control: &DependencyControlDelta,
        visibility: &StagedIngressVisibility,
    ) -> Result<(), DependencyStageError> {
        if !matches!(control, DependencyControlDelta::Event(_)) {
            return Ok(());
        }
        for expected in &mut self.owner_origins {
            expected.any = dependency_origin_has_potential_visible_for_stage_in_cut(
                entries,
                cut,
                &expected.origin,
                visibility,
            )?;
        }
        Ok(())
    }

    fn is_fresh_after_stage(
        &self,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
        control: &DependencyControlDelta,
        visibility: &StagedIngressVisibility,
    ) -> bool {
        match control {
            DependencyControlDelta::None => self.is_fresh(entries, cut, control),
            DependencyControlDelta::Event(_) => {
                self.event_rows_are_fresh_after_stage(entries, cut, control, visibility)
            }
            // Maintenance consumes population order and exhaustion. Preserve
            // its complete Plan-time OCC unchanged.
            DependencyControlDelta::Maintenance(_) => self.is_fresh(entries, cut, control),
        }
    }
}

fn dependency_relation_point_is_visible_in_cut(
    entries: &ShardedOwnerMap,
    cut: &ShardedOwnerWriteCut<'_>,
    point: &DependencyRelationPoint,
    receipt: Option<&mut DependencyVisibilityReceipt>,
) -> Result<bool, DependencyError> {
    let shard = cut.projection_shard(dependency_relation_shard(entries, &point.key));
    let row = shard
        .dependency_relations
        .get(&point.key.origin())
        .and_then(|origin| origin.key(&point.key));
    let set = match point.target.consumer_phase() {
        Some(phase) => row.map(|row| row.consumers.members(phase)),
        None => row.map(|row| &row.waiters),
    };
    let Some(set) = set else {
        return Ok(false);
    };
    match receipt {
        Some(receipt) => set.observe_contains_visible(&point.owner, receipt),
        None => Ok(set.contains_visible(&point.owner)),
    }
}

fn dependency_has_visible_accepted_consumers_observed_in_cut(
    entries: &ShardedOwnerMap,
    cut: &ShardedOwnerWriteCut<'_>,
    key: &DependencyKey,
    receipt: &mut DependencyVisibilityReceipt,
) -> Result<bool, DependencyError> {
    cut.projection_shard(dependency_relation_shard(entries, key))
        .dependency_relations
        .get(&key.origin())
        .and_then(|origin| origin.key(key))
        .map_or(Ok(false), |row| {
            row.consumers.accepted.observe_has_visible(receipt)
        })
}

fn dependency_next_visible_owner_observed_in_cut(
    entries: &ShardedOwnerMap,
    cut: &ShardedOwnerWriteCut<'_>,
    key: &DependencyKey,
    scope: DirtyScope,
    cursor: Option<&RawTxHash>,
    receipt: &mut DependencyVisibilityReceipt,
) -> Result<Option<RawTxHash>, DependencyError> {
    let shard = cut.projection_shard(dependency_relation_shard(entries, key));
    let row = shard
        .dependency_relations
        .get(&key.origin())
        .and_then(|origin| origin.key(key));
    match scope {
        DirtyScope::AllConsumers => row.map_or(Ok(None), |row| {
            row.consumers.observe_first_visible_after(cursor, receipt)
        }),
        DirtyScope::ExistingWaiters => row.map_or(Ok(None), |row| {
            row.waiters.observe_first_visible_after(cursor, receipt)
        }),
    }
}

fn dependency_next_visible_owner_in_cut(
    entries: &ShardedOwnerMap,
    cut: &ShardedOwnerWriteCut<'_>,
    key: &DependencyKey,
    scope: DirtyScope,
    cursor: Option<&RawTxHash>,
) -> Result<Option<RawTxHash>, DependencyError> {
    let shard = cut.projection_shard(dependency_relation_shard(entries, key));
    let row = shard
        .dependency_relations
        .get(&key.origin())
        .and_then(|origin| origin.key(key));
    match scope {
        DirtyScope::AllConsumers => row.map_or(Ok(None), |row| {
            row.consumers.first_visible_after_bounded(cursor)
        }),
        DirtyScope::ExistingWaiters => row.map_or(Ok(None), |row| {
            row.waiters.first_visible_after_bounded(cursor)
        }),
    }
}

fn dependency_origin_key_is_visible_observed(
    origin: &DependencyOriginRow,
    key: &DependencyKey,
    row: &DependencyKeyRelationRow,
    receipt: &mut DependencyVisibilityReceipt,
) -> Result<bool, DependencyError> {
    let stable = row
        .consumers
        .stable_len()
        .ok_or(DependencyError::Projection)?;
    let physical = row
        .consumers
        .physical_len()
        .ok_or(DependencyError::Projection)?;
    let transitional = origin.key_is_transitional(key)?;
    if stable != 0 && !transitional {
        Ok(true)
    } else if stable == 0 && physical != 0 && transitional {
        row.consumers.observe_has_visible(receipt)
    } else if physical == 0 && row.waiters.is_empty() && !transitional {
        Ok(false)
    } else {
        Err(DependencyError::Projection)
    }
}

fn dependency_origin_key_is_visible(
    origin: &DependencyOriginRow,
    key: &DependencyKey,
    row: &DependencyKeyRelationRow,
) -> Result<bool, DependencyError> {
    let stable = row
        .consumers
        .stable_len()
        .ok_or(DependencyError::Projection)?;
    let physical = row
        .consumers
        .physical_len()
        .ok_or(DependencyError::Projection)?;
    let transitional = origin.key_is_transitional(key)?;
    if stable != 0 && !transitional {
        Ok(true)
    } else if stable == 0 && physical != 0 && transitional {
        row.consumers.has_visible_bounded()
    } else if physical == 0 && row.waiters.is_empty() && !transitional {
        Ok(false)
    } else {
        Err(DependencyError::Projection)
    }
}

fn dependency_origin_key_has_potential_visible_for_stage(
    origin: &DependencyOriginRow,
    key: &DependencyKey,
    row: &DependencyKeyRelationRow,
    visibility: &StagedIngressVisibility,
) -> Result<bool, DependencyError> {
    let stable = row
        .consumers
        .stable_len()
        .ok_or(DependencyError::Projection)?;
    let physical = row
        .consumers
        .physical_len()
        .ok_or(DependencyError::Projection)?;
    let transitional = origin.key_is_transitional(key)?;
    if stable != 0 && !transitional {
        Ok(true)
    } else if stable == 0 && physical != 0 && transitional {
        row.consumers
            .has_potential_visible_for_stage_bounded(visibility)
    } else if physical == 0 && row.waiters.is_empty() && !transitional {
        Ok(false)
    } else {
        Err(DependencyError::Projection)
    }
}

fn dependency_origin_has_visible_key_observed_in_cut(
    entries: &ShardedOwnerMap,
    cut: &ShardedOwnerWriteCut<'_>,
    origin: &DependencyOrigin,
    receipt: &mut DependencyVisibilityReceipt,
) -> Result<bool, DependencyError> {
    let shard = cut.projection_shard(dependency_origin_shard(entries, origin));
    let Some(row) = shard.dependency_relations.get(origin) else {
        return Ok(false);
    };
    let visit_limit = row
        .transitional_len()
        .checked_add(1)
        .ok_or(DependencyError::Projection)?;
    for (key, key_row) in row.keys.iter().take(visit_limit) {
        if dependency_origin_key_is_visible_observed(row, key, key_row, receipt)? {
            return Ok(true);
        }
    }
    if row.physical_len() != row.transitional_len() {
        return Err(DependencyError::Projection);
    }
    Ok(false)
}

fn dependency_origin_has_visible_key_in_cut(
    entries: &ShardedOwnerMap,
    cut: &ShardedOwnerWriteCut<'_>,
    origin: &DependencyOrigin,
) -> Result<bool, DependencyError> {
    let shard = cut.projection_shard(dependency_origin_shard(entries, origin));
    let Some(row) = shard.dependency_relations.get(origin) else {
        return Ok(false);
    };
    let visit_limit = row
        .transitional_len()
        .checked_add(1)
        .ok_or(DependencyError::Projection)?;
    for (key, key_row) in row.keys.iter().take(visit_limit) {
        if dependency_origin_key_is_visible(row, key, key_row)? {
            return Ok(true);
        }
    }
    if row.physical_len() != row.transitional_len() {
        return Err(DependencyError::Projection);
    }
    Ok(false)
}

fn dependency_origin_has_potential_visible_for_stage_in_cut(
    entries: &ShardedOwnerMap,
    cut: &ShardedOwnerWriteCut<'_>,
    origin: &DependencyOrigin,
    visibility: &StagedIngressVisibility,
) -> Result<bool, DependencyStageError> {
    let shard = cut.projection_shard(dependency_origin_shard(entries, origin));
    let Some(row) = shard.dependency_relations.get(origin) else {
        return Ok(false);
    };
    let visit_limit = row
        .transitional_len()
        .checked_add(1)
        .ok_or(DependencyStageError::Projection)?;
    for (key, key_row) in row.keys.iter().take(visit_limit) {
        if dependency_origin_key_has_potential_visible_for_stage(row, key, key_row, visibility)
            .map_err(|_| DependencyStageError::Projection)?
        {
            return Ok(true);
        }
    }
    if row.physical_len() != row.transitional_len() {
        return Err(DependencyStageError::Projection);
    }
    Ok(false)
}

fn dependency_origin_matches_observed_in_cut(
    entries: &ShardedOwnerMap,
    cut: &ShardedOwnerWriteCut<'_>,
    origin: &DependencyOrigin,
    expected: Option<&BTreeSet<DependencyKey>>,
    receipt: &mut DependencyVisibilityReceipt,
) -> Result<bool, DependencyError> {
    let shard = cut.projection_shard(dependency_origin_shard(entries, origin));
    let row = shard.dependency_relations.get(origin);
    let physical = row.map_or(0, DependencyOriginRow::physical_len);
    let transitional = row.map_or(0, DependencyOriginRow::transitional_len);
    let expected_len = expected.map_or(0, BTreeSet::len);
    let visit_limit = expected_len
        .checked_add(transitional)
        .and_then(|count| count.checked_add(1))
        .ok_or(DependencyError::Projection)?;
    let mut expected = expected.into_iter().flatten();
    let mut visited = 0usize;
    if let Some(row) = row {
        for (key, key_row) in row.keys.iter().take(visit_limit) {
            visited = visited.checked_add(1).ok_or(DependencyError::Projection)?;
            if dependency_origin_key_is_visible_observed(row, key, key_row, receipt)?
                && expected.next() != Some(key)
            {
                return Ok(false);
            }
        }
    }
    Ok(visited == physical && expected.next().is_none())
}

fn dependency_origin_matches_in_cut(
    entries: &ShardedOwnerMap,
    cut: &ShardedOwnerWriteCut<'_>,
    origin: &DependencyOrigin,
    expected: Option<&BTreeSet<DependencyKey>>,
) -> Result<bool, DependencyError> {
    let shard = cut.projection_shard(dependency_origin_shard(entries, origin));
    let row = shard.dependency_relations.get(origin);
    let physical = row.map_or(0, DependencyOriginRow::physical_len);
    let transitional = row.map_or(0, DependencyOriginRow::transitional_len);
    let expected_len = expected.map_or(0, BTreeSet::len);
    let visit_limit = expected_len
        .checked_add(transitional)
        .and_then(|count| count.checked_add(1))
        .ok_or(DependencyError::Projection)?;
    let mut expected = expected.into_iter().flatten();
    let mut visited = 0usize;
    if let Some(row) = row {
        for (key, key_row) in row.keys.iter().take(visit_limit) {
            visited = visited.checked_add(1).ok_or(DependencyError::Projection)?;
            if dependency_origin_key_is_visible(row, key, key_row)? && expected.next() != Some(key)
            {
                return Ok(false);
            }
        }
    }
    Ok(visited == physical && expected.next().is_none())
}

fn dependency_control_key_prestate_is_fresh(
    entries: &ShardedOwnerMap,
    cut: &ShardedOwnerWriteCut<'_>,
    expected: &DependencyKeyPrestate,
) -> bool {
    match &expected.control {
        None => true,
        Some(DependencyControlKeyPrestate::Event {
            has_accepted_consumers,
        }) => {
            dependency_has_visible_accepted_consumers_in_cut(entries, cut, &expected.key)
                == *has_accepted_consumers
        }
        Some(DependencyControlKeyPrestate::Maintenance { scope, next }) => {
            dependency_next_visible_owner_in_cut(
                entries,
                cut,
                &expected.key,
                *scope,
                expected
                    .dirty
                    .as_ref()
                    .and_then(|dirty| dirty.cursor.as_ref()),
            )
            .is_ok_and(|current| &current == next)
        }
    }
}

fn dependency_event_origins_are_fresh(
    entries: &ShardedOwnerMap,
    cut: &ShardedOwnerWriteCut<'_>,
    control: &DependencyControlDelta,
) -> bool {
    let DependencyControlDelta::Event(event) = control else {
        return true;
    };
    event.origins.iter().all(|expected| {
        dependency_origin_matches_in_cut(entries, cut, &expected.origin, expected.keys.as_ref())
            .unwrap_or(false)
    })
}

impl SettlementDependencyEvidence {
    fn key(&self, key: &DependencyKey) -> Option<&SettlementDependencyKeyEvidence> {
        self.keys
            .binary_search_by(|candidate| candidate.key.cmp(key))
            .ok()
            .and_then(|position| self.keys.get(position))
    }

    fn all_observed_dependencies_available(&self, observed: &ObservedDependencies) -> bool {
        observed.keys().all(|key| {
            self.key(key)
                .and_then(|evidence| evidence.level)
                .is_some_and(|level| {
                    observed.dependency_cut() < level.last_change
                        && level
                            .last_definitive_loss
                            .is_none_or(|loss| loss < level.last_change)
                })
        })
    }

    pub(super) fn proof_is_current(
        &self,
        dependencies: &KnownDependencies,
        cut: DependencyCut,
    ) -> bool {
        dependencies.keys().iter().all(|key| {
            self.key(key).is_some_and(|evidence| {
                evidence
                    .level
                    .and_then(|level| level.last_definitive_loss)
                    .is_none_or(|loss| loss <= cut)
            })
        })
    }

    pub(super) fn resolution_is_current(
        &self,
        baseline: &KnownDependencies,
        resolved: &KnownDependencies,
        cut: DependencyCut,
    ) -> bool {
        self.proof_is_current(resolved, cut)
            && resolved.keys().iter().all(|key| {
                baseline.contains(key)
                    || self.key(key).is_some_and(|evidence| {
                        evidence
                            .unindexed
                            .last_definitive_loss
                            .is_none_or(|loss| loss <= cut)
                    })
            })
    }

    pub(super) fn missing_result_is_current(
        &self,
        baseline: &KnownDependencies,
        dependencies: &KnownDependencies,
        missing: &MissingDependencies,
        cut: DependencyCut,
    ) -> bool {
        self.resolution_is_current(baseline, dependencies, cut)
            && self.missing_observation_is_current(baseline, missing, cut)
    }

    fn missing_observation_is_current(
        &self,
        baseline: &KnownDependencies,
        missing: &MissingDependencies,
        cut: DependencyCut,
    ) -> bool {
        self.proof_is_current(baseline, cut)
            && missing.keys().iter().all(|key| {
                self.key(key).is_some_and(|evidence| {
                    evidence.level.is_none_or(|level| {
                        level.last_change <= cut
                            && level.last_definitive_loss.is_none_or(|loss| loss <= cut)
                    })
                })
            })
            && missing.keys().iter().all(|key| {
                baseline.contains(key)
                    || self.key(key).is_some_and(|evidence| {
                        evidence
                            .unindexed
                            .last_change
                            .is_none_or(|change| change <= cut)
                    })
            })
    }

    fn extend_sharded_read_support(
        &self,
        entries: &ShardedOwnerMap,
        support: &mut ShardReadSupport,
    ) {
        for expected in &self.keys {
            for shard in [
                dependency_relation_shard(entries, &expected.key),
                entries
                    .layout
                    .router
                    .shard(b"dependency/level", &expected.key),
                entries
                    .layout
                    .router
                    .shard(b"dependency/unindexed", &expected.key),
            ] {
                support.insert(shard);
            }
        }
    }

    fn extend_ready_phase_read_support(
        &self,
        entries: &ShardedOwnerMap,
        support: &mut ShardReadSupport,
    ) {
        for expected in &self.keys {
            for shard in [
                entries
                    .layout
                    .router
                    .shard(b"dependency/level", &expected.key),
                entries
                    .layout
                    .router
                    .shard(b"dependency/unindexed", &expected.key),
            ] {
                support.insert(shard);
            }
        }
    }

    fn is_fresh_after_ready_phase_stage(
        &self,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
    ) -> bool {
        self.keys.iter().all(|expected| {
            let level_row = cut.projection_shard(
                entries
                    .layout
                    .router
                    .shard(b"dependency/level", &expected.key),
            );
            let unindexed = cut
                .projection_shard(
                    entries
                        .layout
                        .router
                        .shard(b"dependency/unindexed", &expected.key),
                )
                .dependency_unindexed;
            level_row
                .dependency_levels
                .get(&expected.key)
                .and_then(DependencyControlCell::logical)
                .copied()
                == expected.level
                && level_row
                    .dependency_dirty
                    .get(&expected.key)
                    .and_then(DependencyControlCell::logical)
                    == expected.dirty.as_ref()
                && unindexed == expected.unindexed
        })
    }

    fn is_fresh(&self, entries: &ShardedOwnerMap, cut: &ShardedOwnerWriteCut<'_>) -> bool {
        self.keys.iter().all(|expected| {
            let consumer_shard = dependency_relation_shard(entries, &expected.key);
            let shard = cut.projection_shard(consumer_shard);
            let origin = shard.dependency_relations.get(&expected.key.origin());
            let relation = origin.and_then(|origin| origin.key(&expected.key));
            let owner_matches = relation
                .map_or(Ok(None), |row| row.consumers.visible_phase(&self.owner))
                .is_ok_and(|phase| phase == expected.owner_phase);
            let origin_matches = expected.owner_phase.is_none()
                || origin.zip(relation).is_some_and(|(origin, row)| {
                    dependency_origin_key_is_visible(origin, &expected.key, row).unwrap_or(false)
                });
            let level_row = cut.projection_shard(
                entries
                    .layout
                    .router
                    .shard(b"dependency/level", &expected.key),
            );
            let unindexed = cut
                .projection_shard(
                    entries
                        .layout
                        .router
                        .shard(b"dependency/unindexed", &expected.key),
                )
                .dependency_unindexed;
            owner_matches
                && origin_matches
                && level_row
                    .dependency_levels
                    .get(&expected.key)
                    .and_then(DependencyControlCell::logical)
                    .copied()
                    == expected.level
                && level_row
                    .dependency_dirty
                    .get(&expected.key)
                    .and_then(DependencyControlCell::logical)
                    == expected.dirty.as_ref()
                && unindexed == expected.unindexed
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DependencyStageError {
    Stale,
    Projection,
    Capacity,
    Allocation,
}

/// Linear terminal of one staged dependency batch. `Activated` is the exact
/// receipt for at least one hidden dirty row becoming visible; `Poisoned`
/// records that the generation-wide dependency health latch was tripped while
/// finalizing or rolling the batch back. Deliberately not `Clone` or `Copy`.
#[derive(Debug, PartialEq, Eq)]
#[must_use = "dependency finalization must reach committed wake/fault publication"]
pub(super) enum DependencyFinalization {
    Quiet,
    Activated,
    Poisoned,
}

/// The source delta is owned until its final-cut writes have been applied.
/// Visibility remains the sole publication authority, so finalization checks
/// this state against the shared token instead of creating a second marker.
enum StagedDependencyState {
    Staged(Box<DependencyBatchDelta>),
    Activating,
    RowsActivated { maintenance_activated: bool },
    Terminal,
}

/// Move-only capability for one symmetric dependency relation transition.
/// Every changed relation entry owns its stage direction and the scheduler's
/// shared visibility token in the one physical relation authority.
#[must_use = "a staged dependency batch must be explicitly finalized or rolled back by Drop"]
pub(super) struct StagedDependencyBatch {
    entries: ShardedOwnerMap,
    maintenance: std::sync::Arc<DependencyMaintenanceState>,
    generation_nonce: std::sync::Arc<()>,
    stage_bank: std::sync::Arc<DependencyStageBank>,
    bank_permit: Option<DependencyStageBankPermit>,
    state: StagedDependencyState,
    staged_relations: Vec<StagedDependencyRelation>,
    staged_levels: Vec<StagedDependencyControl<DependencyLevel>>,
    staged_dirty: Vec<StagedDependencyControl<DirtyDependency>>,
    fanout_absence: Vec<StagedDependencyFanoutAbsence>,
    unindexed: Vec<StagedUnindexedContribution>,
    control_cursor: MaintenanceCursorTail,
    cleanup: DependencyCleanupScratch,
    visibility: StagedIngressVisibility,
    publication: Option<DependencyIngressPublication>,
}

/// A dependency stage whose final-cut rows have been applied but whose shared
/// visibility receipt has not yet been consumed. Hidden stages cannot be
/// published directly; every normal caller must cross this owned state.
pub(super) struct RowsActivatedDependencyBatch(StagedDependencyBatch);

/// A dependency stage bound to its exact publication fact and ready for
/// synchronous terminal cleanup.
pub(super) struct PublishedDependencyBatch(StagedDependencyBatch);

/// Sealed dependency half of the shared retained-ingress fast path. The type
/// can only be constructed after proving that the batch contains no Event,
/// Maintenance, settlement, Waiting, Retire or Accepted relation semantics.
/// Its exact hidden edge receipts therefore replace dependency shard locks at
/// the final owner cut without weakening the general staged dependency path.
#[must_use = "a scheduler-sealed retained dependency stage must publish or roll back"]
pub(super) struct SchedulerSealedRetainedDependency(StagedDependencyBatch);

/// Exact owner-local phase transition used by shared Ready settlement. The
/// hidden relation cells replace only owner relation/origin freshness; level,
/// dirty and unindexed evidence remain in the final read cut.
#[must_use = "a sealed Ready dependency transition must publish or roll back"]
pub(super) struct SealedReadyPhaseDependency {
    stage: StagedDependencyBatch,
}

#[derive(Clone)]
struct StagedDependencyRelation {
    point: DependencyRelationPoint,
    action: DependencyRelationAction,
    staged_cell: Option<std::sync::Arc<StagedDependencyRelationState>>,
}

impl StagedDependencyRelation {
    fn receipt_is_hidden(&self) -> bool {
        self.staged_cell
            .as_ref()
            .is_some_and(|staged| !staged.visibility.is_visible())
    }
}

enum MaintenanceCursorTail {
    Unchanged,
    Set(Option<DependencyKey>),
    SetAfterCount(DependencyKey),
}

struct StagedUnindexedContribution {
    shard: usize,
    level: UnindexedDependencyLevel,
}

/// A negative fanout fact used by the control plan compiled at Bind. Positive
/// fanout may disappear before Apply because final cleanup safely folds an
/// orphan level back into unindexed evidence. Negative fanout cannot grow:
/// doing so would make the already-staged no-dirty/unindexed branch
/// insufficient, so the final cut must reject it as ordinary stale progress.
struct StagedDependencyFanoutAbsence {
    key: DependencyKey,
    no_consumers: bool,
    no_waiters: bool,
}

impl StagedDependencyFanoutAbsence {
    fn is_fresh(
        &self,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
        visibility: &StagedIngressVisibility,
    ) -> bool {
        (!self.no_consumers
            || dependency_has_consumers_for_stage_in_cut(entries, cut, &self.key, visibility)
                .is_ok_and(|present| !present))
            && (!self.no_waiters
                || dependency_has_waiters_for_stage_in_cut(entries, cut, &self.key, visibility)
                    .is_ok_and(|present| !present))
    }
}

struct StagedDependencyControlPlan {
    levels: Vec<StagedDependencyControl<DependencyLevel>>,
    dirty: Vec<StagedDependencyControl<DirtyDependency>>,
    fanout_absence: Vec<StagedDependencyFanoutAbsence>,
    unindexed: Vec<StagedUnindexedContribution>,
    cursor: MaintenanceCursorTail,
}

impl StagedDependencyControlPlan {
    fn try_for_key_count(count: usize) -> Result<Self, DependencyStageError> {
        let mut levels = Vec::new();
        levels
            .try_reserve_exact(count)
            .map_err(|_| DependencyStageError::Allocation)?;
        let mut dirty = Vec::new();
        dirty
            .try_reserve_exact(count)
            .map_err(|_| DependencyStageError::Allocation)?;
        let mut fanout_absence = Vec::new();
        fanout_absence
            .try_reserve_exact(count)
            .map_err(|_| DependencyStageError::Allocation)?;
        let mut unindexed = Vec::new();
        unindexed
            .try_reserve_exact(
                count
                    .checked_mul(2)
                    .ok_or(DependencyStageError::Projection)?,
            )
            .map_err(|_| DependencyStageError::Allocation)?;
        Ok(Self {
            levels,
            dirty,
            fanout_absence,
            unindexed,
            cursor: MaintenanceCursorTail::Unchanged,
        })
    }

    fn push_unindexed(
        &mut self,
        entries: &ShardedOwnerMap,
        key: &DependencyKey,
        level: DependencyLevel,
    ) {
        self.unindexed.push(StagedUnindexedContribution {
            shard: entries.layout.router.shard(b"dependency/unindexed", key),
            level: UnindexedDependencyLevel {
                last_change: Some(level.last_change),
                last_definitive_loss: level.last_definitive_loss,
            },
        });
    }

    fn canonicalize_unindexed(&mut self) {
        self.unindexed.sort_unstable_by_key(|entry| entry.shard);
        self.unindexed.dedup_by(|next, current| {
            if current.shard == next.shard {
                let contribution = next.level;
                let current = &mut current.level;
                current.last_change = match (current.last_change, contribution.last_change) {
                    (Some(left), Some(right)) => Some(left.max(right)),
                    (left, right) => left.or(right),
                };
                current.last_definitive_loss = match (
                    current.last_definitive_loss,
                    contribution.last_definitive_loss,
                ) {
                    (Some(left), Some(right)) => Some(left.max(right)),
                    (left, right) => left.or(right),
                };
                true
            } else {
                false
            }
        });
    }
}

#[cfg(test)]
impl DependencySlot {
    fn extend_shard_support(&self, support: &mut super::shard_support::AuthorityShardSupport) {
        for key in self.dependencies.keys() {
            support.insert(b"dependency/consumer", key);
            support.insert(b"dependency/origin", &key.origin());
            support.insert(b"dependency/level", key);
        }
        if let Some(waiting) = &self.waiting {
            for key in waiting.keys() {
                support.insert(b"dependency/waiter", key);
                support.insert(b"dependency/origin", &key.origin());
                support.insert(b"dependency/level", key);
            }
        }
    }
}

#[cfg(test)]
impl DependencyBatchDelta {
    pub(in crate::authority) fn extend_shard_support(
        &self,
        support: &mut super::shard_support::AuthorityShardSupport,
        exclusive: &mut super::shard_support::ExclusiveSupport,
    ) {
        for slot in self.removed.iter().chain(&self.added).chain(&self.observed) {
            slot.extend_shard_support(support);
        }
        match &self.control {
            DependencyControlDelta::None => {}
            DependencyControlDelta::Event(event) => {
                for change in &event.changes {
                    support.insert(b"dependency/relation", &change.key.origin());
                    support.insert(b"dependency/level", &change.key);
                }
                exclusive.dependency_control = true;
            }
            DependencyControlDelta::Maintenance(maintenance) => {
                support.insert(b"dependency/relation", &maintenance.key().origin());
                support.insert(b"dependency/level", maintenance.key());
                exclusive.dependency_control = true;
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VacancyPolicy {
    ExistingOwnersOnly,
    PrimaryVacancyProven,
}

#[derive(Debug)]
pub(super) struct DependencyFrontier {
    entries: ShardedOwnerMap,
    maintenance: std::sync::Arc<DependencyMaintenanceState>,
    generation_nonce: std::sync::Arc<()>,
    stage_bank: std::sync::Arc<DependencyStageBank>,
}

#[derive(Debug, Default)]
struct DependencyMaintenanceState {
    cursor: Mutex<Option<DependencyKey>>,
    poisoned: std::sync::atomic::AtomicBool,
}

#[derive(Debug)]
struct DependencyStageBank {
    available: AtomicUsize,
    capacity: usize,
    generation_nonce: std::sync::Arc<()>,
}

impl DependencyStageBank {
    fn new(generation_nonce: std::sync::Arc<()>, capacity: usize) -> Self {
        Self {
            available: AtomicUsize::new(capacity),
            capacity,
            generation_nonce,
        }
    }

    fn try_acquire(
        self: &std::sync::Arc<Self>,
        units: usize,
    ) -> Result<DependencyStageBankPermit, DependencyStageError> {
        let units = if units == 0 { 1 } else { units };
        self.available
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |available| {
                available.checked_sub(units)
            })
            .map_err(|_| DependencyStageError::Capacity)?;
        Ok(DependencyStageBankPermit {
            bank: std::sync::Arc::clone(self),
            units,
        })
    }
}

struct DependencyStageBankPermit {
    bank: std::sync::Arc<DependencyStageBank>,
    units: usize,
}

impl DependencyStageBankPermit {
    fn try_grow(&mut self, additional: usize) -> Result<(), DependencyStageError> {
        let next = self
            .units
            .checked_add(additional)
            .ok_or(DependencyStageError::Capacity)?;
        self.bank
            .available
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |available| {
                available.checked_sub(additional)
            })
            .map_err(|_| DependencyStageError::Capacity)?;
        self.units = next;
        Ok(())
    }
}

impl Drop for DependencyStageBankPermit {
    fn drop(&mut self) {
        let previous = self.bank.available.fetch_add(self.units, Ordering::Release);
        debug_assert!(
            self.bank
                .capacity
                .checked_sub(self.units)
                .is_some_and(|remaining| previous <= remaining)
        );
    }
}

impl DependencyMaintenanceState {
    fn poison(&self) {
        self.poisoned.store(true, Ordering::Release);
    }

    fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Acquire)
    }
}

fn first_logical_dirty_key<'row>(
    rows: impl Iterator<
        Item = (
            &'row DependencyKey,
            &'row DependencyControlCell<DirtyDependency>,
        ),
    >,
    staged: usize,
    visibility: &mut DependencyVisibilityReceipt,
) -> Result<Option<DependencyKey>, DependencyError> {
    let visit_limit = staged.checked_add(1).ok_or(DependencyError::Projection)?;
    let mut visited = 0usize;
    for (key, cell) in rows.take(visit_limit) {
        visited = visited.checked_add(1).ok_or(DependencyError::Projection)?;
        if visibility.observe_control(cell)?.is_some() {
            return Ok(Some(key.clone()));
        }
    }
    if visited > staged {
        return Err(DependencyError::Projection);
    }
    Ok(None)
}

impl DependencyFrontier {
    pub(super) fn for_entries(entries: &ShardedOwnerMap, stage_capacity: usize) -> Self {
        let generation_nonce = std::sync::Arc::new(());
        Self {
            entries: entries.clone(),
            maintenance: std::sync::Arc::new(DependencyMaintenanceState::default()),
            generation_nonce: std::sync::Arc::clone(&generation_nonce),
            stage_bank: std::sync::Arc::new(DependencyStageBank::new(
                generation_nonce,
                stage_capacity,
            )),
        }
    }

    /// Rebind one already-built generation's cursor/count state to the stable
    /// live shard envelope after its complete generation payload has been
    /// swapped in. No dependency fact is copied: all rows moved with the
    /// generation payload under the fixed 64-shard cut.
    pub(super) fn rebind_entries(mut self, entries: &ShardedOwnerMap) -> Self {
        self.entries = entries.clone();
        self
    }

    fn shard<K: std::hash::Hash>(&self, domain: &'static [u8], key: &K) -> usize {
        self.entries.layout.router.shard(domain, key)
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "the sole router masks every result to the fixed 64-shard array"
    )]
    fn routed_shard(
        &self,
        shard: usize,
    ) -> &ckb_util::parking_lot::RwLock<super::shard::AuthorityShard> {
        &self.entries.layout.shards[shard]
    }

    /// Dirty control is the mutable state of one exact dependency key, so it
    /// is co-located with that key's level row instead of creating a global
    /// dependency authority. The sole cursor below is only a fairness hint;
    /// it owns no dependency fact and a stale value safely wraps.
    fn dirty(&self, key: &DependencyKey) -> Option<DirtyDependency> {
        self.routed_shard(self.shard(b"dependency/level", key))
            .read()
            .dependency_dirty
            .get(key)
            .and_then(DependencyControlCell::logical_cloned)
    }

    #[cfg(test)]
    fn dirty_insert(
        &self,
        key: DependencyKey,
        dirty: DirtyDependency,
    ) -> Result<Option<DirtyDependency>, DependencyError> {
        let mut shard = self
            .routed_shard(self.shard(b"dependency/level", &key))
            .write();
        insert_stable_control(&mut shard.dependency_dirty, key, dirty)
            .map_err(|()| DependencyError::Stale)
    }

    #[cfg(test)]
    fn dirty_is_empty(&self) -> bool {
        self.next_dirty_key().is_ok_and(|key| key.is_none())
    }

    fn next_dirty_key(&self) -> Result<Option<DependencyKey>, DependencyError> {
        if self.maintenance.is_poisoned() {
            return Err(DependencyError::Projection);
        }
        let cursor = self.maintenance.cursor.lock().clone();
        let mut after: Option<DependencyKey> = None;
        let mut first: Option<DependencyKey> = None;
        let mut visibility = DependencyVisibilityReceipt::default();
        for shard in self.entries.layout.shards.iter() {
            let shard = shard.read();
            if let Some(key) = first_logical_dirty_key(
                shard.dependency_dirty.iter(),
                shard.dependency_dirty_staged,
                &mut visibility,
            )? {
                first = Some(match first.take() {
                    Some(current) => current.min(key),
                    None => key,
                });
            }
            if let Some(cursor) = &cursor {
                let next = first_logical_dirty_key(
                    shard.dependency_dirty.range((Excluded(cursor), Unbounded)),
                    shard.dependency_dirty_staged,
                    &mut visibility,
                )?;
                if let Some(key) = next {
                    after = Some(match after.take() {
                        Some(current) => current.min(key),
                        None => key,
                    });
                }
            }
        }
        if !visibility.is_current() {
            return Err(DependencyError::Stale);
        }
        Ok(after.or(first))
    }

    fn with_consumers<T>(
        &self,
        key: &DependencyKey,
        read: impl FnOnce(Option<&DependencyConsumerRow>) -> T,
    ) -> T {
        let shard = self
            .routed_shard(dependency_relation_shard(&self.entries, key))
            .read();
        read(
            shard
                .dependency_relations
                .get(&key.origin())
                .and_then(|origin| origin.key(key))
                .map(|row| &row.consumers),
        )
    }

    fn with_waiters<T>(
        &self,
        key: &DependencyKey,
        read: impl FnOnce(Option<&DependencyRelationSet>) -> T,
    ) -> T {
        let shard = self
            .routed_shard(dependency_relation_shard(&self.entries, key))
            .read();
        read(
            shard
                .dependency_relations
                .get(&key.origin())
                .and_then(|origin| origin.key(key))
                .map(|row| &row.waiters),
        )
    }

    fn consumers(
        &self,
        key: &DependencyKey,
    ) -> Result<Option<BTreeSet<RawTxHash>>, DependencyError> {
        self.with_consumers(key, |owners| {
            owners.map_or(Ok(None), |owners| {
                owners.visible_members_bounded(crate::constants::MAX_POOL_MUTATION_CANDIDATES)
            })
        })
    }

    pub(super) fn observe_consumers_bounded(
        &self,
        key: DependencyKey,
        limit: usize,
    ) -> Result<(Option<BTreeSet<RawTxHash>>, ObservedDependencyConsumerRead), DependencyError>
    {
        let shard = self
            .routed_shard(dependency_relation_shard(&self.entries, &key))
            .read();
        let row = shard
            .dependency_relations
            .get(&key.origin())
            .and_then(|origin| origin.key(&key));
        let (accepted, accepted_visible) = row.map_or(Ok((Vec::new(), Vec::new())), |row| {
            row.consumers.accepted.capture_bounded(limit)
        })?;
        let (other, other_visible) = row.map_or(Ok((Vec::new(), Vec::new())), |row| {
            row.consumers.other.capture_bounded(limit)
        })?;
        let mut visible = BTreeSet::new();
        for owner in accepted_visible.into_iter().chain(other_visible) {
            if !visible.insert(owner) {
                return Err(DependencyError::Projection);
            }
            if visible.len() > limit {
                return Err(DependencyError::Fanout);
            }
        }
        Ok((
            (!visible.is_empty()).then_some(visible),
            ObservedDependencyConsumerRead {
                key,
                kind: DependencyConsumerObservationKind::General,
                accepted,
                other,
                accepted_over_limit: None,
            },
        ))
    }

    pub(super) fn observe_accepted_consumers_bounded_or_over_limit(
        &self,
        key: DependencyKey,
        limit: usize,
    ) -> Result<ObservedAcceptedConsumers, DependencyError> {
        let shard = self
            .routed_shard(dependency_relation_shard(&self.entries, &key))
            .read();
        let row = shard
            .dependency_relations
            .get(&key.origin())
            .and_then(|origin| origin.key(&key));
        if row.is_some_and(|row| row.consumers.accepted.proves_visible_over_limit(limit)) {
            return Ok(ObservedAcceptedConsumers::OverLimit(
                ObservedDependencyConsumerRead {
                    key,
                    kind: DependencyConsumerObservationKind::Accepted,
                    accepted: Vec::new(),
                    other: Vec::new(),
                    accepted_over_limit: Some(limit),
                },
            ));
        }
        let (accepted, visible) = row.map_or(Ok((Vec::new(), Vec::new())), |row| {
            row.consumers.accepted.capture_bounded(limit)
        })?;
        let visible = (!visible.is_empty()).then(|| visible.into_iter().collect());
        Ok(ObservedAcceptedConsumers::Within {
            visible,
            receipt: ObservedDependencyConsumerRead {
                key,
                kind: DependencyConsumerObservationKind::Accepted,
                accepted,
                other: Vec::new(),
                accepted_over_limit: None,
            },
        })
    }

    fn waiters(&self, key: &DependencyKey) -> Result<Option<BTreeSet<RawTxHash>>, DependencyError> {
        self.with_waiters(key, |owners| {
            let Some(owners) = owners else {
                return Ok(None);
            };
            let mut visible = BTreeSet::new();
            owners.extend_visible_bounded(
                &mut visible,
                crate::constants::MAX_POOL_MUTATION_CANDIDATES,
            )?;
            Ok((!visible.is_empty()).then_some(visible))
        })
    }

    fn next_visible_owner(
        &self,
        key: &DependencyKey,
        scope: DirtyScope,
        cursor: Option<&RawTxHash>,
    ) -> Result<Option<RawTxHash>, DependencyError> {
        let shard = self
            .routed_shard(dependency_relation_shard(&self.entries, key))
            .read();
        let row = shard
            .dependency_relations
            .get(&key.origin())
            .and_then(|origin| origin.key(key));
        match scope {
            DirtyScope::AllConsumers => row.map_or(Ok(None), |row| {
                row.consumers.first_visible_after_bounded(cursor)
            }),
            DirtyScope::ExistingWaiters => row.map_or(Ok(None), |row| {
                row.waiters.first_visible_after_bounded(cursor)
            }),
        }
    }

    fn level(&self, key: &DependencyKey) -> Option<DependencyLevel> {
        self.routed_shard(self.shard(b"dependency/level", key))
            .read()
            .dependency_levels
            .get(key)
            .and_then(DependencyControlCell::logical)
            .copied()
    }

    fn unindexed_level(&self, key: &DependencyKey) -> UnindexedDependencyLevel {
        self.routed_shard(self.shard(b"dependency/unindexed", key))
            .read()
            .dependency_unindexed
    }

    fn with_origin_keys<T>(
        &self,
        origin: &DependencyOrigin,
        read: impl FnOnce(Option<&DependencyOriginRow>) -> T,
    ) -> T {
        let shard = self
            .routed_shard(dependency_origin_shard(&self.entries, origin))
            .read();
        read(shard.dependency_relations.get(origin))
    }

    fn origin_keys(
        &self,
        origin: &DependencyOrigin,
    ) -> Result<Option<BTreeSet<DependencyKey>>, DependencyError> {
        self.with_origin_keys(origin, |row| {
            let Some(row) = row else {
                return Ok(None);
            };
            let limit = crate::constants::MAX_POOL_MUTATION_CANDIDATES;
            let visit_limit = limit
                .checked_add(row.transitional_len())
                .and_then(|count| count.checked_add(1))
                .ok_or(DependencyError::Projection)?;
            let mut visibility = DependencyVisibilityReceipt::default();
            let mut visible = BTreeSet::new();
            for (key, key_row) in row.keys.iter().take(visit_limit) {
                if dependency_origin_key_is_visible_observed(row, key, key_row, &mut visibility)? {
                    visible.insert(key.clone());
                    if visible.len() > limit {
                        return Err(DependencyError::Fanout);
                    }
                }
            }
            if row.physical_len() > visit_limit {
                return Err(DependencyError::Fanout);
            }
            if !visibility.is_current() {
                return Err(DependencyError::Stale);
            }
            Ok((!visible.is_empty()).then_some(visible))
        })
    }

    fn consumer_contains(&self, key: &DependencyKey, owner: &RawTxHash) -> bool {
        self.with_consumers(key, |owners| {
            owners.is_some_and(|owners| {
                owners.accepted.contains_visible(owner) || owners.other.contains_visible(owner)
            })
        })
    }

    fn waiter_contains(&self, key: &DependencyKey, owner: &RawTxHash) -> bool {
        self.with_waiters(key, |owners| {
            owners.is_some_and(|owners| owners.contains_visible(owner))
        })
    }

    fn origin_contains(&self, origin: &DependencyOrigin, key: &DependencyKey) -> bool {
        self.with_origin_keys(origin, |row| {
            row.and_then(|origin| origin.key(key).map(|key_row| (origin, key_row)))
                .is_some_and(|(origin, key_row)| {
                    dependency_origin_key_is_visible(origin, key, key_row).unwrap_or(false)
                })
        })
    }

    #[cfg(test)]
    fn replace_level(
        &self,
        key: DependencyKey,
        level: DependencyLevel,
    ) -> Result<Option<DependencyLevel>, DependencyError> {
        let mut shard = self
            .routed_shard(self.shard(b"dependency/level", &key))
            .write();
        insert_stable_control(&mut shard.dependency_levels, key, level)
            .map_err(|()| DependencyError::Stale)
    }
}

impl DependencyDelta {
    pub(super) fn with_control(mut self, control: DependencyEntryControlDelta) -> Self {
        self.control = control;
        self
    }

    pub(super) fn into_shared_batch(
        self,
        frontier: &DependencyFrontier,
        evidence: Option<SettlementDependencyEvidence>,
    ) -> Result<DependencyBatchDelta, DependencyError> {
        self.into_shared_batch_with_control(frontier, evidence, None)
    }

    pub(super) fn into_shared_maintenance_batch(
        self,
        frontier: &DependencyFrontier,
        maintenance: DependencyMaintenancePlan,
        evidence: Option<SettlementDependencyEvidence>,
    ) -> Result<DependencyBatchDelta, DependencyError> {
        self.into_shared_batch_with_control(
            frontier,
            evidence,
            Some(DependencyControlDelta::Maintenance(maintenance)),
        )
    }

    fn into_shared_batch_with_control(
        self,
        frontier: &DependencyFrontier,
        evidence: Option<SettlementDependencyEvidence>,
        control: Option<DependencyControlDelta>,
    ) -> Result<DependencyBatchDelta, DependencyError> {
        let Self {
            before,
            after,
            observed: unchanged,
            control: entry_control,
        } = self;
        let control = control.unwrap_or_else(|| entry_control.into());
        let mut removed = Vec::new();
        let mut added = Vec::new();
        let mut unchanged_slots = Vec::new();
        if before.is_some() {
            removed
                .try_reserve_exact(1)
                .map_err(|_| DependencyError::Allocation)?;
        }
        if after.is_some() {
            added
                .try_reserve_exact(1)
                .map_err(|_| DependencyError::Allocation)?;
        }
        if unchanged.is_some() {
            unchanged_slots
                .try_reserve_exact(1)
                .map_err(|_| DependencyError::Allocation)?;
        }
        if let Some(before) = before {
            removed.push(before);
        }
        if let Some(after) = after {
            added.push(after);
        }
        if let Some(unchanged) = unchanged {
            unchanged_slots.push(unchanged);
        }
        let delta = DependencyBatchDelta {
            removed,
            added,
            observed: Vec::new(),
            unchanged: unchanged_slots,
            relation_changes: Vec::new(),
            settlement_evidence: Vec::new(),
            control,
            prestate: DependencyBatchPrestate::default(),
        }
        .seal_prestate(frontier)?;
        delta.with_settlement_evidence(evidence, frontier)
    }
}

impl DependencyBatchDelta {
    fn seal_prestate(mut self, frontier: &DependencyFrontier) -> Result<Self, DependencyError> {
        if let DependencyControlDelta::Event(event) = &self.control
            && event
                .changes
                .array_windows::<2>()
                .any(|[left, right]| left.key >= right.key)
        {
            return Err(DependencyError::Projection);
        }
        self.relation_changes = self
            .compile_relation_changes()
            .map_err(|error| match error {
                DependencyStageError::Stale | DependencyStageError::Projection => {
                    DependencyError::Projection
                }
                DependencyStageError::Capacity | DependencyStageError::Allocation => {
                    DependencyError::Allocation
                }
            })?;
        self.prestate = DependencyBatchPrestate::capture(frontier, &self)?;
        Ok(self)
    }

    pub(super) fn with_control(
        mut self,
        control: DependencyControlDelta,
        frontier: &DependencyFrontier,
    ) -> Result<Self, DependencyError> {
        self.control = control;
        self.seal_prestate(frontier)
    }

    #[cfg(test)]
    pub(in crate::authority) fn prestate_is_fresh(
        &self,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
    ) -> bool {
        self.prestate.is_fresh(entries, cut, &self.control)
            && self
                .settlement_evidence
                .iter()
                .all(|evidence| evidence.is_fresh(entries, cut))
    }

    fn seal_settlement_evidence(
        mut self,
        mut evidence: Vec<SettlementDependencyEvidence>,
        frontier: &DependencyFrontier,
    ) -> Result<Self, DependencyError> {
        evidence.sort_unstable_by(|left, right| left.owner.cmp(&right.owner));
        if evidence
            .array_windows::<2>()
            .any(|[left, right]| left.owner == right.owner)
        {
            return Err(DependencyError::Projection);
        }
        self.observed
            .try_reserve(evidence.len())
            .map_err(|_| DependencyError::Allocation)?;
        for witness in &evidence {
            let already_bound = [&self.removed, &self.added, &self.observed]
                .into_iter()
                .any(|slots| {
                    slots
                        .binary_search_by(|slot| slot.hash.cmp(&witness.owner))
                        .is_ok()
                });
            if already_bound {
                continue;
            }
            let position = self
                .unchanged
                .binary_search_by(|slot| slot.hash.cmp(&witness.owner))
                .map_err(|_| DependencyError::Projection)?;
            self.observed.push(self.unchanged.remove(position));
            self.observed
                .sort_unstable_by(|left, right| left.hash.cmp(&right.hash));
        }
        for witness in &evidence {
            let removed = self
                .removed
                .binary_search_by(|slot| slot.hash.cmp(&witness.owner))
                .ok()
                .and_then(|position| self.removed.get(position));
            let added = self
                .added
                .binary_search_by(|slot| slot.hash.cmp(&witness.owner))
                .ok()
                .and_then(|position| self.added.get(position));
            let observed = self
                .observed
                .binary_search_by(|slot| slot.hash.cmp(&witness.owner))
                .ok()
                .and_then(|position| self.observed.get(position));
            let still_unchanged = self
                .unchanged
                .binary_search_by(|slot| slot.hash.cmp(&witness.owner))
                .is_ok();
            let before = removed.or(observed).ok_or(DependencyError::Projection)?;
            let endpoint = match (removed, added, observed, still_unchanged) {
                (Some(_), Some(after), None, false) => {
                    SettlementDependencyEndpoint::Retained(after)
                }
                (None, None, Some(after), false) => SettlementDependencyEndpoint::Retained(after),
                (Some(_), None, None, false) => SettlementDependencyEndpoint::Removed,
                _ => return Err(DependencyError::Projection),
            };
            if before.waiting.is_some()
                || witness.keys.iter().any(|key| {
                    key.owner_phase
                        != before
                            .dependencies
                            .contains(&key.key)
                            .then_some(before.phase)
                })
                || before
                    .dependencies
                    .keys()
                    .iter()
                    .any(|key| witness.key(key).is_none())
                || match endpoint {
                    SettlementDependencyEndpoint::Retained(after) => {
                        after
                            .dependencies
                            .keys()
                            .iter()
                            .any(|key| witness.key(key).is_none())
                            || after.waiting.as_ref().is_some_and(|waiting| {
                                waiting.keys().any(|key| witness.key(key).is_none())
                            })
                    }
                    SettlementDependencyEndpoint::Removed => false,
                }
            {
                return Err(DependencyError::Projection);
            }
        }
        self.settlement_evidence = evidence;
        self.seal_prestate(frontier)
    }

    pub(super) fn with_settlement_evidence(
        self,
        evidence: Option<SettlementDependencyEvidence>,
        frontier: &DependencyFrontier,
    ) -> Result<Self, DependencyError> {
        let Some(evidence) = evidence else {
            return Ok(self);
        };
        let mut evidence_set = Vec::new();
        evidence_set
            .try_reserve_exact(1)
            .map_err(|_| DependencyError::Allocation)?;
        evidence_set.push(evidence);
        self.seal_settlement_evidence(evidence_set, frontier)
    }

    /// Bind the availability evidence which promotes one replacement-history
    /// owner back to Recovery during dependency maintenance.
    ///
    /// Unlike compute settlement, this proof deliberately covers only the
    /// history entry's projected-final unavailable trigger set. Its complete
    /// retained dependency basis may contain surviving pool parents which are
    /// not wake triggers, while the Recovery owner initially returns to its
    /// declared (pre-resolution) dependency basis. Reusing the ordinary
    /// settlement binder would therefore either reject every history owner
    /// (`before.waiting`) or incorrectly require evidence for unrelated
    /// retained dependencies.
    pub(super) fn with_history_maintenance_evidence(
        mut self,
        evidence: SettlementDependencyEvidence,
        maintenance: &DependencyMaintenancePlan,
    ) -> Result<Self, DependencyError> {
        if !self.settlement_evidence.is_empty() {
            return Err(DependencyError::Projection);
        }
        let before = self
            .removed
            .binary_search_by(|slot| slot.hash.cmp(&evidence.owner))
            .ok()
            .and_then(|position| self.removed.get(position))
            .ok_or(DependencyError::Projection)?;
        let after = self
            .added
            .binary_search_by(|slot| slot.hash.cmp(&evidence.owner))
            .ok()
            .and_then(|position| self.added.get(position))
            .ok_or(DependencyError::Projection)?;
        let observed = before.waiting.as_ref().ok_or(DependencyError::Projection)?;
        if after.waiting.is_some()
            || !observed.contains(maintenance.key())
            || evidence.keys.len() != observed.keys().len()
            || !evidence
                .keys
                .iter()
                .zip(observed.keys())
                .all(|(witness, key)| {
                    witness.key == *key && witness.owner_phase == Some(before.phase)
                })
        {
            return Err(DependencyError::Projection);
        }
        self.settlement_evidence.push(evidence);
        Ok(self)
    }

    #[cfg(test)]
    pub(super) fn is_shared_primary_insertion_only(&self) -> bool {
        self.removed.is_empty()
            && self.observed.is_empty()
            && self.unchanged.is_empty()
            && matches!(self.control, DependencyControlDelta::None)
            && self.added.iter().all(|slot| slot.waiting.is_none())
    }

    pub(super) fn is_scheduler_sealed_retained_shape(&self) -> bool {
        self.removed.is_empty()
            && matches!(self.control, DependencyControlDelta::None)
            && self.settlement_evidence.is_empty()
            && (!self.added.is_empty() || !self.observed.is_empty() || !self.unchanged.is_empty())
            && self
                .added
                .iter()
                .chain(&self.observed)
                .chain(&self.unchanged)
                .all(|slot| slot.phase == DependencyConsumerPhase::Other && slot.waiting.is_none())
            && self.relation_changes.iter().all(|change| {
                change.action == DependencyRelationAction::Insert
                    && change.point.target == DependencyRelationTarget::OtherConsumer
            })
    }

    /// A Ready settlement changes only this owner's consumer phase from Other
    /// to Accepted while preserving the exact dependency key set. Its hidden
    /// relation cells therefore carry the owner-local freshness fact; level,
    /// dirty and unindexed evidence remain final-cut reads.
    pub(super) fn is_ready_phase_only_shape(&self) -> bool {
        if self.removed.is_empty()
            || self.removed.len() != self.added.len()
            || !self.observed.is_empty()
            || !self.unchanged.is_empty()
            || !matches!(self.control, DependencyControlDelta::None)
            || (!self.settlement_evidence.is_empty()
                && self.settlement_evidence.len() != self.removed.len())
        {
            return false;
        }
        let mut expected_relations = 0usize;
        for (before, after) in self.removed.iter().zip(&self.added) {
            if before.hash != after.hash
                || before.phase != DependencyConsumerPhase::Other
                || after.phase != DependencyConsumerPhase::Accepted
                || before.dependencies != after.dependencies
                || before.waiting.is_some()
                || after.waiting.is_some()
                || (!self.settlement_evidence.is_empty()
                    && self
                        .settlement_evidence
                        .binary_search_by(|evidence| evidence.owner.cmp(&before.hash))
                        .is_err())
            {
                return false;
            }
            let Some(next) = before.dependencies.len().checked_mul(2) else {
                return false;
            };
            let Some(total) = expected_relations.checked_add(next) else {
                return false;
            };
            expected_relations = total;
        }
        self.relation_changes.len() == expected_relations
            && self.relation_changes.iter().all(|change| {
                let Some(slot) = self
                    .removed
                    .binary_search_by(|slot| slot.hash.cmp(&change.point.owner))
                    .ok()
                    .and_then(|position| self.removed.get(position))
                else {
                    return false;
                };
                slot.dependencies.contains(&change.point.key)
                    && matches!(
                        (change.action, change.point.target),
                        (
                            DependencyRelationAction::Retire,
                            DependencyRelationTarget::OtherConsumer
                        ) | (
                            DependencyRelationAction::Insert,
                            DependencyRelationTarget::AcceptedConsumer
                        )
                    )
            })
    }

    #[cfg(test)]
    pub(in crate::authority) fn ready_phase_shape_for_foundation(&self) -> bool {
        self.is_ready_phase_only_shape()
    }

    fn relation_points(
        slots: &[DependencySlot],
    ) -> Result<Vec<DependencyRelationPoint>, DependencyStageError> {
        let capacity = slots.iter().try_fold(0usize, |count, slot| {
            count
                .checked_add(slot.dependencies.len())
                .and_then(|count| {
                    count.checked_add(
                        slot.waiting
                            .as_ref()
                            .map_or(0, |waiting| waiting.keys().len()),
                    )
                })
        });
        let mut points = Vec::new();
        points
            .try_reserve_exact(capacity.ok_or(DependencyStageError::Projection)?)
            .map_err(|_| DependencyStageError::Allocation)?;
        for slot in slots {
            points.extend(slot.dependencies.keys().iter().cloned().map(|key| {
                DependencyRelationPoint {
                    key,
                    target: DependencyRelationTarget::consumer(slot.phase),
                    owner: slot.hash.clone(),
                }
            }));
            if let Some(waiting) = &slot.waiting {
                points.extend(waiting.keys().cloned().map(|key| DependencyRelationPoint {
                    key,
                    target: DependencyRelationTarget::Waiter,
                    owner: slot.hash.clone(),
                }));
            }
        }
        points.sort_unstable();
        if points
            .array_windows::<2>()
            .any(|[left, right]| left == right)
        {
            return Err(DependencyStageError::Projection);
        }
        Ok(points)
    }

    fn compile_relation_changes(
        &self,
    ) -> Result<Vec<StagedDependencyRelation>, DependencyStageError> {
        let before = Self::relation_points(&self.removed)?;
        let after = Self::relation_points(&self.added)?;
        let capacity = before
            .len()
            .checked_add(after.len())
            .ok_or(DependencyStageError::Projection)?;
        let mut changes = Vec::new();
        changes
            .try_reserve_exact(capacity)
            .map_err(|_| DependencyStageError::Allocation)?;
        let mut before = before.iter().peekable();
        let mut after = after.iter().peekable();
        loop {
            match (before.peek().copied(), after.peek().copied()) {
                (Some(left), Some(right)) if left == right => {
                    before.next();
                    after.next();
                }
                (Some(left), Some(right)) if left < right => {
                    changes.push(StagedDependencyRelation {
                        point: left.clone(),
                        action: DependencyRelationAction::Retire,
                        staged_cell: None,
                    });
                    before.next();
                }
                (Some(_), Some(right)) => {
                    changes.push(StagedDependencyRelation {
                        point: right.clone(),
                        action: DependencyRelationAction::Insert,
                        staged_cell: None,
                    });
                    after.next();
                }
                (Some(left), None) => {
                    changes.push(StagedDependencyRelation {
                        point: left.clone(),
                        action: DependencyRelationAction::Retire,
                        staged_cell: None,
                    });
                    before.next();
                }
                (None, Some(right)) => {
                    changes.push(StagedDependencyRelation {
                        point: right.clone(),
                        action: DependencyRelationAction::Insert,
                        staged_cell: None,
                    });
                    after.next();
                }
                (None, None) => break,
            }
        }
        Ok(changes)
    }

    fn relation_stage_write_support(&self, entries: &ShardedOwnerMap) -> ShardWriteSupport {
        let mut support = ShardWriteSupport::default();
        for slot in self.removed.iter().chain(&self.added) {
            for key in slot.dependencies.keys() {
                support.insert(dependency_relation_shard(entries, key));
                support.insert(entries.layout.router.shard(b"dependency/level", key));
            }
            if let Some(waiting) = &slot.waiting {
                for key in waiting.keys() {
                    support.insert(dependency_relation_shard(entries, key));
                }
            }
        }
        for change in &self.relation_changes {
            if change.point.target.consumer_phase().is_some() {
                support.insert(
                    entries
                        .layout
                        .router
                        .shard(b"dependency/level", &change.point.key),
                );
            }
        }
        match &self.control {
            DependencyControlDelta::None => {}
            DependencyControlDelta::Event(event) => {
                for change in &event.changes {
                    support.insert(
                        entries
                            .layout
                            .router
                            .shard(b"dependency/level", &change.key),
                    );
                }
            }
            DependencyControlDelta::Maintenance(maintenance) => {
                support.insert(
                    entries
                        .layout
                        .router
                        .shard(b"dependency/level", maintenance.key()),
                );
            }
        }
        support
    }

    fn ready_phase_stage_write_support(&self, entries: &ShardedOwnerMap) -> ShardWriteSupport {
        let mut support = ShardWriteSupport::default();
        for change in &self.relation_changes {
            support.insert(dependency_relation_shard(entries, &change.point.key));
        }
        support
    }

    #[cfg(test)]
    pub(in crate::authority) fn relation_stage_write_support_for_foundation(
        &self,
        entries: &ShardedOwnerMap,
    ) -> ShardWriteSupport {
        if self.is_ready_phase_only_shape() {
            self.ready_phase_stage_write_support(entries)
        } else {
            self.relation_stage_write_support(entries)
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn has_consumer_phase_transition_for_foundation(&self) -> bool {
        self.relation_changes.iter().any(|change| {
            change.point.target.consumer_phase().is_some()
                && self.relation_changes.iter().any(|other| {
                    other.point.key == change.point.key
                        && other.point.owner == change.point.owner
                        && other.point.target != change.point.target
                        && other.point.target.consumer_phase().is_some()
                        && other.action != change.action
                })
        })
    }

    pub(in crate::authority) fn sharded_read_support(
        &self,
        entries: &ShardedOwnerMap,
    ) -> ShardReadSupport {
        let mut support = ShardReadSupport::default();
        // The prestate is the authority for every row consumed by freshness.
        // Folding support from it prevents a newly added witness kind from
        // silently escaping the mixed cut. Rows that are also mutated are
        // harmlessly dominated by the corresponding write support.
        for expected in &self.prestate.keys {
            support.insert(dependency_relation_shard(entries, &expected.key));
            support.insert(
                entries
                    .layout
                    .router
                    .shard(b"dependency/level", &expected.key),
            );
        }
        for expected in &self.prestate.owner_origins {
            support.insert(dependency_origin_shard(entries, &expected.origin));
        }
        if let DependencyControlDelta::Event(event) = &self.control {
            for expected in &event.origins {
                support.insert(dependency_origin_shard(entries, &expected.origin));
            }
        }
        for (shard, _) in &self.prestate.unindexed {
            support.insert(*shard);
        }
        for evidence in &self.settlement_evidence {
            evidence.extend_sharded_read_support(entries, &mut support);
        }
        support
    }

    pub(in crate::authority) fn ready_phase_final_read_support(
        &self,
        entries: &ShardedOwnerMap,
    ) -> ShardReadSupport {
        let mut support = ShardReadSupport::default();
        for expected in &self.prestate.keys {
            support.insert(
                entries
                    .layout
                    .router
                    .shard(b"dependency/level", &expected.key),
            );
        }
        for (shard, _) in &self.prestate.unindexed {
            support.insert(*shard);
        }
        for evidence in &self.settlement_evidence {
            evidence.extend_ready_phase_read_support(entries, &mut support);
        }
        support
    }

    fn ready_phase_prestate_is_fresh_after_stage(
        &self,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
    ) -> bool {
        self.prestate
            .ready_phase_rows_are_fresh_after_stage(entries, cut)
            && self
                .settlement_evidence
                .iter()
                .all(|evidence| evidence.is_fresh_after_ready_phase_stage(entries, cut))
    }

    fn final_read_support(&self, entries: &ShardedOwnerMap) -> ShardReadSupport {
        self.sharded_read_support(entries)
    }

    /// Conservative owner-commit writes after relation and level/dirty rows
    /// have already been installed behind the staged visibility token.
    /// Phase-only moves remain final-cut reads; only a key that may lose its
    /// last consumer, or a control event/maintenance step, can merge level
    /// evidence into its unindexed shard at publication.
    pub(in crate::authority) fn sharded_owner_commit_write_support(
        &self,
        entries: &ShardedOwnerMap,
    ) -> super::shard::ShardWriteSupport {
        let mut support = super::shard::ShardWriteSupport::default();
        let mut remaining = self.relation_changes.as_slice();
        while let Some((first, tail)) = remaining.split_first() {
            let key = &first.point.key;
            let same_key_len = tail.partition_point(|change| change.point.key == *key);
            let (same_key, rest) = tail.split_at(same_key_len);
            let mut retires_consumer = false;
            let mut inserts_consumer = false;
            for change in std::iter::once(first).chain(same_key) {
                if change.point.target.consumer_phase().is_some() {
                    retires_consumer |= change.action == DependencyRelationAction::Retire;
                    inserts_consumer |= change.action == DependencyRelationAction::Insert;
                }
            }
            if retires_consumer && !inserts_consumer {
                support.insert(entries.layout.router.shard(b"dependency/unindexed", key));
            }
            remaining = rest;
        }
        match &self.control {
            DependencyControlDelta::None => {}
            DependencyControlDelta::Event(event) => {
                for change in &event.changes {
                    support.insert(
                        entries
                            .layout
                            .router
                            .shard(b"dependency/unindexed", &change.key),
                    );
                }
            }
            DependencyControlDelta::Maintenance(maintenance) => {
                let key = maintenance.key();
                support.insert(entries.layout.router.shard(b"dependency/unindexed", key));
            }
        }
        support
    }

    /// Consume the source delta after its relation and control rows have been
    /// compiled into the one staged capability. Only non-row control effects
    /// remain here: unindexed level fences and the maintenance scan cursor.
    /// No allocation or ordinary failure is possible after owner mutation.
    fn activate_staged_control_rows(
        self,
        frontier: &DependencyFrontier,
        cut: &mut ShardedOwnerWriteCut<'_>,
        unindexed: &[StagedUnindexedContribution],
        control_cursor: MaintenanceCursorTail,
    ) -> MaintenanceCursorTail {
        // The staged carrier owns the compiled forms of every source field.
        // Destructuring here makes retirement of the old second apply engine
        // explicit rather than silently dropping a delta.
        let Self {
            removed: _,
            added: _,
            observed: _,
            unchanged: _,
            relation_changes: _,
            settlement_evidence: _,
            control,
            prestate: _,
        } = self;
        for contribution in unindexed {
            let current = &mut cut
                .projection_shard_mut(contribution.shard)
                .dependency_unindexed;
            current.last_change = match (current.last_change, contribution.level.last_change) {
                (Some(left), Some(right)) => Some(left.max(right)),
                (left, right) => left.or(right),
            };
            current.last_definitive_loss = match (
                current.last_definitive_loss,
                contribution.level.last_definitive_loss,
            ) {
                (Some(left), Some(right)) => Some(left.max(right)),
                (left, right) => left.or(right),
            };
        }

        match control {
            DependencyControlDelta::None | DependencyControlDelta::Event(_)
                if matches!(control_cursor, MaintenanceCursorTail::Unchanged) =>
            {
                MaintenanceCursorTail::Unchanged
            }
            DependencyControlDelta::Maintenance(maintenance) => {
                let key = maintenance.key().clone();
                if !matches!(
                    &control_cursor,
                    MaintenanceCursorTail::SetAfterCount(expected) if expected == &key
                ) {
                    frontier.maintenance.poison();
                }
                // The cursor is only a fairness hint. Keeping the completed
                // key conservatively wraps to any remaining fixed-shard row;
                // an empty frontier is determined from the rows themselves.
                MaintenanceCursorTail::Set(Some(key))
            }
            DependencyControlDelta::None | DependencyControlDelta::Event(_) => {
                frontier.maintenance.poison();
                MaintenanceCursorTail::Unchanged
            }
        }
    }
}

fn checked_add_signed(value: usize, delta: isize) -> Option<usize> {
    if delta >= 0 {
        value.checked_add(delta as usize)
    } else {
        value.checked_sub(delta.unsigned_abs())
    }
}

fn relation_set_for_target<'row>(
    shard: &'row super::shard::AuthorityShard,
    key: &DependencyKey,
    target: DependencyRelationTarget,
) -> Option<&'row DependencyRelationSet> {
    let row = shard
        .dependency_relations
        .get(&key.origin())
        .and_then(|origin| origin.key(key));
    match target.consumer_phase() {
        Some(phase) => row.map(|row| row.consumers.members(phase)),
        None => row.map(|row| &row.waiters),
    }
}

fn relation_set_for_target_mut<'row>(
    shard: &'row mut super::shard::AuthorityShard,
    key: &DependencyKey,
    target: DependencyRelationTarget,
) -> &'row mut DependencyRelationSet {
    let row = shard
        .dependency_relations
        .entry(key.origin())
        .or_default()
        .key_mut_or_default(key.clone());
    match target.consumer_phase() {
        Some(phase) => row.consumers.members_mut(phase),
        None => &mut row.waiters,
    }
}

#[derive(Clone)]
struct StagedDependencyOriginKeyTarget {
    origin: DependencyOrigin,
    key: DependencyKey,
    transitional: bool,
}

#[derive(Clone)]
struct StagedDependencyOriginTarget {
    origin: DependencyOrigin,
    before: usize,
    after: usize,
}

fn preflight_staged_dependency_relations(
    entries: &ShardedOwnerMap,
    cut: &ShardedOwnerWriteCut<'_>,
    changes: &[StagedDependencyRelation],
) -> Result<Vec<StagedDependencyOriginTarget>, DependencyStageError> {
    let mut consumer_effects: Vec<(DependencyKey, isize, isize)> = Vec::new();
    consumer_effects
        .try_reserve_exact(changes.len())
        .map_err(|_| DependencyStageError::Allocation)?;

    let mut remaining = changes;
    while let Some((first, tail)) = remaining.split_first() {
        let key = &first.point.key;
        let target = first.point.target;
        let same_point_len = tail
            .partition_point(|change| change.point.key == *key && change.point.target == target);
        let (same_point, rest) = tail.split_at(same_point_len);
        let shard = cut.projection_shard(dependency_relation_shard(entries, key));
        let set = relation_set_for_target(shard, key, target);
        let current_staged = set.map_or(0, DependencyRelationSet::staged_len);
        let current_physical = set.map_or(0, DependencyRelationSet::physical_len);
        let current_stable = current_physical
            .checked_sub(current_staged)
            .ok_or(DependencyStageError::Projection)?;
        let mut staged_delta = 0isize;
        let mut stable_delta = 0isize;
        let mut physical_delta = 0isize;
        for change in std::iter::once(first).chain(same_point) {
            let effect = match set {
                Some(set) => set.stage_effect(&change.point.owner, change.action)?,
                None if change.action == DependencyRelationAction::Insert => {
                    DependencyRelationStageEffect {
                        staged_delta: 1,
                        stable_delta: 0,
                        physical_delta: 1,
                    }
                }
                None => return Err(DependencyStageError::Stale),
            };
            staged_delta = staged_delta
                .checked_add(effect.staged_delta)
                .ok_or(DependencyStageError::Projection)?;
            stable_delta = stable_delta
                .checked_add(effect.stable_delta)
                .ok_or(DependencyStageError::Projection)?;
            physical_delta = physical_delta
                .checked_add(effect.physical_delta)
                .ok_or(DependencyStageError::Projection)?;
        }
        let next_staged = checked_add_signed(current_staged, staged_delta)
            .ok_or(DependencyStageError::Projection)?;
        let _next_stable = checked_add_signed(current_stable, stable_delta)
            .ok_or(DependencyStageError::Projection)?;
        let _next_physical = checked_add_signed(current_physical, physical_delta)
            .ok_or(DependencyStageError::Projection)?;
        let _ = next_staged;
        if target.consumer_phase().is_some() {
            if let Some((last_key, last_stable, last_physical)) = consumer_effects.last_mut()
                && last_key == key
            {
                *last_stable = last_stable
                    .checked_add(stable_delta)
                    .ok_or(DependencyStageError::Projection)?;
                *last_physical = last_physical
                    .checked_add(physical_delta)
                    .ok_or(DependencyStageError::Projection)?;
            } else {
                consumer_effects.push((key.clone(), stable_delta, physical_delta));
            }
        }
        remaining = rest;
    }

    let mut targets = Vec::new();
    targets
        .try_reserve_exact(consumer_effects.len())
        .map_err(|_| DependencyStageError::Allocation)?;
    for (key, stable_delta, physical_delta) in consumer_effects {
        let shard = cut.projection_shard(dependency_relation_shard(entries, &key));
        let origin = key.origin();
        let origin_row = shard.dependency_relations.get(&origin);
        let row = origin_row.and_then(|origin| origin.key(&key));
        let current_physical = row
            .map(|row| &row.consumers)
            .map_or(Some(0), DependencyConsumerRow::physical_len)
            .ok_or(DependencyStageError::Projection)?;
        let current_stable = row
            .map(|row| &row.consumers)
            .map_or(Some(0), DependencyConsumerRow::stable_len)
            .ok_or(DependencyStageError::Projection)?;
        let next_physical = checked_add_signed(current_physical, physical_delta)
            .ok_or(DependencyStageError::Projection)?;
        let next_stable = checked_add_signed(current_stable, stable_delta)
            .ok_or(DependencyStageError::Projection)?;
        let current_transitional = origin_row
            .map_or(Ok(false), |origin| origin.key_is_transitional(&key))
            .map_err(|_| DependencyStageError::Projection)?;
        if current_transitional != (current_physical != 0 && current_stable == 0) {
            return Err(DependencyStageError::Projection);
        }
        targets.push(StagedDependencyOriginKeyTarget {
            origin,
            key,
            transitional: next_physical != 0 && next_stable == 0,
        });
    }
    targets.sort_unstable_by(|left, right| {
        left.origin
            .cmp(&right.origin)
            .then_with(|| left.key.cmp(&right.key))
    });
    let mut origin_targets = Vec::new();
    origin_targets
        .try_reserve_exact(targets.len())
        .map_err(|_| DependencyStageError::Allocation)?;
    let mut remaining = targets.as_slice();
    while let Some((first, tail)) = remaining.split_first() {
        let origin = &first.origin;
        let same_origin_len = tail.partition_point(|target| target.origin == *origin);
        let (same_origin, rest) = tail.split_at(same_origin_len);
        let shard = cut.projection_shard(dependency_origin_shard(entries, origin));
        let row = shard.dependency_relations.get(origin);
        let before = row.map_or(0, DependencyOriginRow::transitional_len);
        let mut transitional = before;
        for target in std::iter::once(first).chain(same_origin) {
            let current = row
                .map_or(Ok(false), |row| row.key_is_transitional(&target.key))
                .map_err(|_| DependencyStageError::Projection)?;
            transitional = transitional
                .checked_sub(usize::from(current))
                .and_then(|count| count.checked_add(usize::from(target.transitional)))
                .ok_or(DependencyStageError::Projection)?;
        }
        origin_targets.push(StagedDependencyOriginTarget {
            origin: origin.clone(),
            before,
            after: transitional,
        });
        remaining = rest;
    }
    Ok(origin_targets)
}

fn set_staged_dependency_origins_prechecked(
    entries: &ShardedOwnerMap,
    cut: &mut ShardedOwnerWriteCut<'_>,
    targets: &[StagedDependencyOriginTarget],
    staged: bool,
) -> bool {
    if targets.iter().any(|target| {
        !cut.projection_shard(dependency_origin_shard(entries, &target.origin))
            .dependency_relations
            .contains_key(&target.origin)
    }) {
        return false;
    }
    for target in targets {
        let shard = cut.projection_shard_mut(dependency_origin_shard(entries, &target.origin));
        if let Some(row) = shard.dependency_relations.get_mut(&target.origin) {
            row.transitional = if staged { target.after } else { target.before };
        }
    }
    true
}

fn reserve_staged_dependency_origins(
    entries: &ShardedOwnerMap,
    cut: &mut ShardedOwnerWriteCut<'_>,
    targets: &[StagedDependencyOriginTarget],
) -> Result<Vec<DependencyOrigin>, DependencyStageError> {
    let mut prepared_new = Vec::new();
    prepared_new
        .try_reserve_exact(targets.len())
        .map_err(|_| DependencyStageError::Allocation)?;
    if targets
        .array_windows::<2>()
        .any(|[left, right]| left.origin == right.origin)
    {
        return Err(DependencyStageError::Projection);
    }
    for target in targets {
        let shard = cut.projection_shard(dependency_origin_shard(entries, &target.origin));
        if !shard.dependency_relations.contains_key(&target.origin) {
            if target.before != 0 {
                return Err(DependencyStageError::Projection);
            }
            prepared_new.push((target.origin.clone(), DependencyOriginRow::default()));
        }
    }
    let mut inserted = Vec::new();
    inserted
        .try_reserve_exact(prepared_new.len())
        .map_err(|_| DependencyStageError::Allocation)?;
    for (origin, row) in prepared_new {
        cut.projection_shard_mut(dependency_origin_shard(entries, &origin))
            .dependency_relations
            .insert(origin.clone(), row);
        inserted.push(origin);
    }
    Ok(inserted)
}

fn remove_empty_staged_origin_scaffolds(
    entries: &ShardedOwnerMap,
    cut: &mut ShardedOwnerWriteCut<'_>,
    origins: &[DependencyOrigin],
) {
    for origin in origins {
        let shard = cut.projection_shard_mut(dependency_origin_shard(entries, origin));
        if shard
            .dependency_relations
            .get(origin)
            .is_some_and(DependencyOriginRow::is_empty)
        {
            shard.dependency_relations.remove(origin);
        }
    }
}

fn rollback_staged_dependency_relations_in_cut(
    entries: &ShardedOwnerMap,
    cut: &mut ShardedOwnerWriteCut<'_>,
    staged_relations: &[StagedDependencyRelation],
    visibility: &StagedIngressVisibility,
    origin_targets: &[StagedDependencyOriginTarget],
    new_origin_scaffolds: &[DependencyOrigin],
) -> bool {
    let mut exact = set_staged_dependency_origins_prechecked(entries, cut, origin_targets, false);
    for staged in staged_relations.iter().rev() {
        let shard = cut.projection_shard_mut(dependency_relation_shard(entries, &staged.point.key));
        let origin = staged.point.key.origin();
        let finish = shard
            .dependency_relations
            .get_mut(&origin)
            .map_or(Ok(DependencyRelationFinish::Foreign), |row| {
                row.finish_owned_relation(staged, visibility)
            });
        exact &= matches!(finish, Ok(DependencyRelationFinish::Finished));
        if shard
            .dependency_relations
            .get(&origin)
            .is_some_and(DependencyOriginRow::is_empty)
        {
            shard.dependency_relations.remove(&origin);
        }
    }
    remove_empty_staged_origin_scaffolds(entries, cut, new_origin_scaffolds);
    exact
}

impl StagedDependencyBatch {
    #[cfg(test)]
    pub(super) fn stage_primary_insertions(
        frontier: &DependencyFrontier,
        delta: DependencyBatchDelta,
        visibility: StagedIngressVisibility,
    ) -> Result<Self, DependencyStageError> {
        if !delta.is_shared_primary_insertion_only() {
            return Err(DependencyStageError::Projection);
        }
        Self::stage_primary_replacements_with_visibility(frontier, delta, visibility)
    }

    pub(super) fn stage_primary_replacements(
        frontier: &DependencyFrontier,
        delta: DependencyBatchDelta,
    ) -> Result<Self, DependencyStageError> {
        let (visibility, publication) =
            StagedIngressVisibility::hidden_with_dependency_publication();
        Self::stage_primary_replacements_inner(
            frontier,
            delta,
            visibility,
            Some(publication),
            false,
        )
    }

    pub(super) fn stage_primary_replacements_with_visibility(
        frontier: &DependencyFrontier,
        delta: DependencyBatchDelta,
        visibility: StagedIngressVisibility,
    ) -> Result<Self, DependencyStageError> {
        Self::stage_primary_replacements_inner(frontier, delta, visibility, None, false)
    }

    pub(super) fn stage_ready_phase(
        frontier: &DependencyFrontier,
        delta: DependencyBatchDelta,
    ) -> Result<SealedReadyPhaseDependency, DependencyStageError> {
        if !delta.is_ready_phase_only_shape() {
            return Err(DependencyStageError::Projection);
        }
        let (visibility, publication) =
            StagedIngressVisibility::hidden_with_dependency_publication();
        let stage = Self::stage_primary_replacements_inner(
            frontier,
            delta,
            visibility,
            Some(publication),
            true,
        )?;
        let valid = matches!(&stage.state, StagedDependencyState::Staged(_))
            && stage.staged_levels.is_empty()
            && stage.staged_dirty.is_empty()
            && stage.fanout_absence.is_empty()
            && stage.unindexed.is_empty()
            && stage.publication.is_some()
            && stage
                .staged_relations
                .iter()
                .all(StagedDependencyRelation::receipt_is_hidden);
        if valid {
            Ok(SealedReadyPhaseDependency { stage })
        } else {
            Err(DependencyStageError::Projection)
        }
    }

    fn stage_primary_replacements_inner(
        frontier: &DependencyFrontier,
        mut delta: DependencyBatchDelta,
        visibility: StagedIngressVisibility,
        publication: Option<DependencyIngressPublication>,
        ready_phase_only: bool,
    ) -> Result<Self, DependencyStageError> {
        if frontier.maintenance.is_poisoned() {
            return Err(DependencyStageError::Projection);
        }
        let entries = frontier.entries.clone();
        if ready_phase_only && !delta.is_ready_phase_only_shape() {
            return Err(DependencyStageError::Projection);
        }
        let writes = if ready_phase_only {
            delta.ready_phase_stage_write_support(&entries)
        } else {
            delta.relation_stage_write_support(&entries)
        };
        let mut staged_relations = std::mem::take(&mut delta.relation_changes);
        let mut bank_permit = frontier.stage_bank.try_acquire(staged_relations.len())?;
        let cleanup_capacity = delta
            .prestate
            .keys
            .len()
            .checked_mul(2)
            .and_then(|count| count.checked_add(staged_relations.len()))
            .ok_or(DependencyStageError::Projection)?;
        let mut cleanup = DependencyCleanupScratch::try_for_relations(cleanup_capacity)?;
        for staged in &mut staged_relations {
            staged.staged_cell = Some(std::sync::Arc::new(StagedDependencyRelationState::new(
                staged.action,
                visibility.clone(),
            )));
        }
        let reads = delta.sharded_read_support(&entries);
        let mut cut = entries.mixed_cut(reads, writes);
        #[cfg(test)]
        entries.enter_shared_ingress_probe(
            super::shard::SharedIngressProbePhase::DependencyStageBeforeRows,
        );

        if !delta
            .prestate
            .is_fresh_before_event_rebase(&entries, &cut, &delta.control)
            || !delta
                .settlement_evidence
                .iter()
                .all(|evidence| evidence.is_fresh(&entries, &cut))
        {
            return Err(DependencyStageError::Stale);
        }
        let origin_targets =
            preflight_staged_dependency_relations(&entries, &cut, &staged_relations)?;
        let new_origin_scaffolds =
            reserve_staged_dependency_origins(&entries, &mut cut, &origin_targets)?;
        if !set_staged_dependency_origins_prechecked(&entries, &mut cut, &origin_targets, true) {
            remove_empty_staged_origin_scaffolds(&entries, &mut cut, &new_origin_scaffolds);
            return Err(DependencyStageError::Projection);
        }
        for (applied, staged) in staged_relations.iter().enumerate() {
            let shard =
                cut.projection_shard_mut(dependency_relation_shard(&entries, &staged.point.key));
            if !relation_set_for_target_mut(shard, &staged.point.key, staged.point.target)
                .apply_stage_prechecked(
                    staged.point.owner.clone(),
                    staged.action,
                    std::sync::Arc::clone(
                        staged
                            .staged_cell
                            .as_ref()
                            .ok_or(DependencyStageError::Projection)?,
                    ),
                )
            {
                let Some(applied_relations) = staged_relations.get(..applied) else {
                    frontier.maintenance.poison();
                    return Err(DependencyStageError::Projection);
                };
                if !rollback_staged_dependency_relations_in_cut(
                    &entries,
                    &mut cut,
                    applied_relations,
                    &visibility,
                    &origin_targets,
                    &new_origin_scaffolds,
                ) {
                    frontier.maintenance.poison();
                }
                return Err(DependencyStageError::Projection);
            }
        }
        if let Err(error) =
            delta
                .prestate
                .rebase_event_owner_origins(&entries, &cut, &delta.control, &visibility)
        {
            if !rollback_staged_dependency_relations_in_cut(
                &entries,
                &mut cut,
                &staged_relations,
                &visibility,
                &origin_targets,
                &new_origin_scaffolds,
            ) {
                frontier.maintenance.poison();
                return Err(DependencyStageError::Projection);
            }
            return Err(error);
        }
        let mut control = match compile_staged_dependency_control_plan(
            frontier,
            &delta,
            &staged_relations,
            &cut,
            &visibility,
        ) {
            Ok(control) => control,
            Err(error) => {
                if !rollback_staged_dependency_relations_in_cut(
                    &entries,
                    &mut cut,
                    &staged_relations,
                    &visibility,
                    &origin_targets,
                    &new_origin_scaffolds,
                ) {
                    frontier.maintenance.poison();
                    return Err(DependencyStageError::Projection);
                }
                return Err(error);
            }
        };
        if ready_phase_only
            && (!control.levels.is_empty()
                || !control.dirty.is_empty()
                || !control.fanout_absence.is_empty()
                || !control.unindexed.is_empty()
                || !matches!(control.cursor, MaintenanceCursorTail::Unchanged))
        {
            if !rollback_staged_dependency_relations_in_cut(
                &entries,
                &mut cut,
                &staged_relations,
                &visibility,
                &origin_targets,
                &new_origin_scaffolds,
            ) {
                frontier.maintenance.poison();
            }
            return Err(DependencyStageError::Projection);
        }
        cleanup.orphan_keys.extend(
            staged_relations
                .iter()
                .filter(|staged| staged.point.target.consumer_phase().is_some())
                .map(|staged| staged.point.key.clone()),
        );
        cleanup
            .orphan_keys
            .extend(control.levels.iter().map(|staged| staged.key.clone()));
        cleanup
            .orphan_keys
            .extend(control.dirty.iter().map(|staged| staged.key.clone()));
        cleanup.orphan_keys.sort_unstable();
        cleanup.orphan_keys.dedup();
        if let Err(error) =
            stage_dependency_control_plan(frontier, &mut cut, &mut bank_permit, &control)
        {
            if !rollback_staged_dependency_relations_in_cut(
                &entries,
                &mut cut,
                &staged_relations,
                &visibility,
                &origin_targets,
                &new_origin_scaffolds,
            ) {
                frontier.maintenance.poison();
                return Err(DependencyStageError::Projection);
            }
            return Err(error);
        }
        drop(cut);
        Ok(Self {
            entries,
            maintenance: std::sync::Arc::clone(&frontier.maintenance),
            generation_nonce: std::sync::Arc::clone(&frontier.generation_nonce),
            stage_bank: std::sync::Arc::clone(&frontier.stage_bank),
            bank_permit: Some(bank_permit),
            state: StagedDependencyState::Staged(Box::new(delta)),
            staged_relations,
            staged_levels: std::mem::take(&mut control.levels),
            staged_dirty: std::mem::take(&mut control.dirty),
            fanout_absence: std::mem::take(&mut control.fanout_absence),
            unindexed: std::mem::take(&mut control.unindexed),
            control_cursor: control.cursor,
            cleanup,
            visibility,
            publication,
        })
    }

    pub(super) fn seal_scheduler_retained(
        self,
    ) -> Result<SchedulerSealedRetainedDependency, DependencyStageError> {
        let valid = matches!(
            &self.state,
            StagedDependencyState::Staged(delta)
                if delta.is_scheduler_sealed_retained_shape()
        ) && self.staged_levels.is_empty()
            && self.staged_dirty.is_empty()
            && self.fanout_absence.is_empty()
            && self.unindexed.is_empty()
            && self.publication.is_none()
            && self
                .staged_relations
                .iter()
                .all(StagedDependencyRelation::receipt_is_hidden);
        if valid {
            Ok(SchedulerSealedRetainedDependency(self))
        } else {
            Err(DependencyStageError::Projection)
        }
    }

    pub(super) fn extend_final_read_support(&self, support: &mut ShardReadSupport) {
        if let StagedDependencyState::Staged(delta) = &self.state {
            support.include(delta.final_read_support(&self.entries));
            for expected in &self.fanout_absence {
                support.insert(dependency_relation_shard(&self.entries, &expected.key));
            }
        }
    }

    pub(super) fn extend_final_write_support(&self, support: &mut ShardWriteSupport) {
        if matches!(&self.state, StagedDependencyState::Staged(_)) {
            // Relation, level, and dirty rows were installed before the final
            // owner cut. Publication mutates only the exact unindexed shards
            // compiled into this linear carrier.
            for contribution in &self.unindexed {
                support.insert(contribution.shard);
            }
        }
    }

    pub(super) fn visibility(&self) -> &StagedIngressVisibility {
        &self.visibility
    }

    #[cfg(test)]
    pub(super) fn fanout_absence_observation_for_foundation(&self) -> (usize, bool) {
        let mut reads = ShardReadSupport::default();
        for expected in &self.fanout_absence {
            reads.insert(dependency_relation_shard(&self.entries, &expected.key));
        }
        let cut = self.entries.mixed_cut(reads, ShardWriteSupport::default());
        (
            self.fanout_absence.len(),
            self.fanout_absence
                .iter()
                .all(|expected| expected.is_fresh(&self.entries, &cut, &self.visibility)),
        )
    }

    #[cfg(test)]
    pub(super) fn fanout_absence_details_for_foundation(
        &self,
    ) -> Vec<(DependencyKey, bool, bool, bool, bool)> {
        let mut reads = ShardReadSupport::default();
        for expected in &self.fanout_absence {
            reads.insert(dependency_relation_shard(&self.entries, &expected.key));
        }
        let cut = self.entries.mixed_cut(reads, ShardWriteSupport::default());
        self.fanout_absence
            .iter()
            .map(|expected| {
                (
                    expected.key.clone(),
                    expected.no_consumers,
                    expected.no_waiters,
                    dependency_has_consumers_for_stage_in_cut(
                        &self.entries,
                        &cut,
                        &expected.key,
                        &self.visibility,
                    )
                    .unwrap_or(true),
                    dependency_has_waiters_for_stage_in_cut(
                        &self.entries,
                        &cut,
                        &expected.key,
                        &self.visibility,
                    )
                    .unwrap_or(true),
                )
            })
            .collect()
    }

    pub(super) fn prestate_is_fresh(&self, cut: &ShardedOwnerWriteCut<'_>) -> bool {
        let StagedDependencyState::Staged(delta) = &self.state else {
            return false;
        };
        !self.maintenance.is_poisoned()
            && delta.prestate.is_fresh_after_stage(
                &self.entries,
                cut,
                &delta.control,
                &self.visibility,
            )
            && delta
                .settlement_evidence
                .iter()
                .all(|evidence| evidence.is_fresh(&self.entries, cut))
            && self
                .fanout_absence
                .iter()
                .all(|expected| expected.is_fresh(&self.entries, cut, &self.visibility))
            && self.staged_relations.iter().all(|staged| {
                let shard = cut
                    .projection_shard(dependency_relation_shard(&self.entries, &staged.point.key));
                relation_set_for_target(shard, &staged.point.key, staged.point.target).is_some_and(
                    |set| set.owns_stage(&staged.point.owner, staged.action, &self.visibility),
                )
            })
            && self.staged_levels.iter().all(|staged| {
                let shard = self
                    .entries
                    .layout
                    .router
                    .shard(b"dependency/level", &staged.key);
                cut.projection_shard(shard)
                    .dependency_levels
                    .get(&staged.key)
                    .is_some_and(|current| current.is_exact_stage(&staged.staged_cell))
            })
            && self.staged_dirty.iter().all(|staged| {
                let shard = self
                    .entries
                    .layout
                    .router
                    .shard(b"dependency/level", &staged.key);
                cut.projection_shard(shard)
                    .dependency_dirty
                    .get(&staged.key)
                    .is_some_and(|current| current.is_exact_stage(&staged.staged_cell))
            })
    }

    /// Apply the source delta's precompiled final-cut writes. Visibility is
    /// deliberately left to the owning publication cut; a structural misuse
    /// poisons the generation without creating a post-owner failure branch.
    fn activate_rows_in_cut(&mut self, cut: &mut ShardedOwnerWriteCut<'_>) {
        let state = std::mem::replace(&mut self.state, StagedDependencyState::Activating);
        let StagedDependencyState::Staged(delta) = state else {
            self.maintenance.poison();
            return;
        };
        let frontier = DependencyFrontier {
            entries: self.entries.clone(),
            maintenance: std::sync::Arc::clone(&self.maintenance),
            generation_nonce: std::sync::Arc::clone(&self.generation_nonce),
            stage_bank: std::sync::Arc::clone(&self.stage_bank),
        };
        let maintenance_activated = self
            .staged_dirty
            .iter()
            .any(|staged| staged.stable_before.is_none() && staged.stable_after.is_some());
        self.control_cursor = delta.activate_staged_control_rows(
            &frontier,
            cut,
            &self.unindexed,
            std::mem::replace(&mut self.control_cursor, MaintenanceCursorTail::Unchanged),
        );
        self.state = StagedDependencyState::RowsActivated {
            maintenance_activated,
        };
    }

    pub(super) fn activate_in_cut(
        mut self,
        cut: &mut ShardedOwnerWriteCut<'_>,
    ) -> RowsActivatedDependencyBatch {
        self.activate_rows_in_cut(cut);
        RowsActivatedDependencyBatch(self)
    }

    /// Normalize every exact staged row after all owner/scheduler cuts have
    /// opened. This is the former Drop body expressed as a fallible internal
    /// operation; it allocates no new scratch and never owns publication.
    fn finish_rows(&mut self) -> Result<(), DependencyStageError> {
        let entries = &self.entries;
        if self.maintenance.is_poisoned()
            || !std::sync::Arc::ptr_eq(&self.generation_nonce, &self.stage_bank.generation_nonce)
            || self
                .bank_permit
                .as_ref()
                .is_none_or(|permit| !std::sync::Arc::ptr_eq(&permit.bank, &self.stage_bank))
        {
            return Err(DependencyStageError::Projection);
        }
        let mut support = super::shard::ShardWriteSupport::default();
        for staged in &self.staged_relations {
            support.insert(dependency_relation_shard(entries, &staged.point.key));
            if staged.point.target.consumer_phase().is_some() {
                support.insert(
                    entries
                        .layout
                        .router
                        .shard(b"dependency/level", &staged.point.key),
                );
                support.insert(
                    entries
                        .layout
                        .router
                        .shard(b"dependency/unindexed", &staged.point.key),
                );
            }
        }
        for staged in &self.staged_levels {
            support.insert(
                entries
                    .layout
                    .router
                    .shard(b"dependency/level", &staged.key),
            );
        }
        for staged in &self.staged_dirty {
            support.insert(
                entries
                    .layout
                    .router
                    .shard(b"dependency/level", &staged.key),
            );
        }
        for key in &self.cleanup.orphan_keys {
            support.insert(dependency_relation_shard(entries, key));
            support.insert(entries.layout.router.shard(b"dependency/level", key));
            support.insert(entries.layout.router.shard(b"dependency/unindexed", key));
        }
        let mut cut = entries.write_cut(support);
        plan_staged_dependency_cleanup(
            entries,
            &cut,
            &self.staged_relations,
            &self.visibility,
            &mut self.cleanup,
        )?;
        preflight_staged_dependency_cleanup(
            &mut self.cleanup,
            &self.staged_levels,
            &self.staged_dirty,
            entries,
            &cut,
        )?;
        // One complete preflight observes every exact control Arc and checked
        // target under this same locally owned cut. The following trusted
        // suffix performs no allocation and opens no competing shard cut.
        for staged in &mut self.staged_levels {
            let shard = entries
                .layout
                .router
                .shard(b"dependency/level", &staged.key);
            finish_exact_control_stage_prechecked(
                &mut cut.projection_shard_mut(shard).dependency_levels,
                staged,
            );
        }
        for staged in &mut self.staged_dirty {
            let shard = entries
                .layout
                .router
                .shard(b"dependency/level", &staged.key);
            let row = cut.projection_shard_mut(shard);
            finish_exact_control_stage_prechecked(&mut row.dependency_dirty, staged);
            row.dependency_dirty_staged = row
                .dependency_dirty_staged
                .checked_sub(1)
                .ok_or(DependencyStageError::Projection)?;
        }
        if !set_staged_dependency_cleanup_counts(entries, &mut cut, &self.cleanup.origin_targets) {
            return Err(DependencyStageError::Projection);
        }
        for staged in &self.staged_relations {
            let shard_index = dependency_relation_shard(entries, &staged.point.key);
            let shard = cut.projection_shard_mut(shard_index);
            let origin = staged.point.key.origin();
            let finish = match shard.dependency_relations.get_mut(&origin) {
                Some(row) => row.finish_exact_relation(staged),
                None => Ok(DependencyRelationFinish::Foreign),
            }?;
            match finish {
                DependencyRelationFinish::Foreign => {
                    return Err(DependencyStageError::Projection);
                }
                DependencyRelationFinish::Finished => {
                    if shard
                        .dependency_relations
                        .get(&origin)
                        .is_some_and(DependencyOriginRow::is_empty)
                    {
                        shard.dependency_relations.remove(&origin);
                    }
                }
            }
        }
        for key in &self.cleanup.eligible_orphan_keys {
            if !prune_stable_orphan_dependency_key_in_cut(entries, &mut cut, key) {
                return Err(DependencyStageError::Projection);
            }
        }
        Ok(())
    }

    fn finalize_inner(&mut self, drop_fallback: bool) -> DependencyFinalization {
        let state = std::mem::replace(&mut self.state, StagedDependencyState::Terminal);
        let visible = self.visibility.is_visible();
        let classification = match (&state, visible) {
            (StagedDependencyState::Staged(_), false) => Some((false, false)),
            (
                StagedDependencyState::RowsActivated {
                    maintenance_activated,
                },
                true,
            ) => Some((true, *maintenance_activated)),
            _ => None,
        };
        let mut poisoned = classification.is_none();
        if !poisoned && self.finish_rows().is_err() {
            poisoned = true;
        }
        // The bank must not become reusable until the exact cleanup cut has
        // completed or the generation has entered its absorbing poison state.
        drop(self.bank_permit.take());

        let outcome = if poisoned || self.maintenance.is_poisoned() {
            self.maintenance.poison();
            DependencyFinalization::Poisoned
        } else {
            let (published, maintenance_activated) = classification.unwrap_or((false, false));
            if published
                && let MaintenanceCursorTail::Set(cursor) =
                    std::mem::replace(&mut self.control_cursor, MaintenanceCursorTail::Unchanged)
            {
                *self.maintenance.cursor.lock() = cursor;
            }
            if published && maintenance_activated {
                DependencyFinalization::Activated
            } else {
                DependencyFinalization::Quiet
            }
        };
        drop(state);

        if drop_fallback && visible && !matches!(&outcome, DependencyFinalization::Poisoned) {
            // A published batch whose linear receipt was discarded can lose an
            // activation wake. Make the omission fail-stop even if row cleanup
            // itself was exact.
            self.maintenance.poison();
            DependencyFinalization::Poisoned
        } else {
            outcome
        }
    }

    fn finalize(mut self) -> DependencyFinalization {
        self.finalize_inner(false)
    }

    /// Publish one legacy-exclusive dependency stage after its owner,
    /// source-version and scheduler commits. The enclosing authority write
    /// lease excludes every shared relation writer between stage and this
    /// exact cut, so publication has no ordinary stale branch.
    pub(super) fn publish_exclusive(self) -> DependencyFinalization {
        let entries = self.entries.clone();
        let mut reads = ShardReadSupport::default();
        let mut writes = ShardWriteSupport::default();
        self.extend_final_read_support(&mut reads);
        self.extend_final_write_support(&mut writes);
        let mut cut = entries.mixed_cut(reads, writes);
        debug_assert!(self.prestate_is_fresh(&cut));
        let published = self.activate_in_cut(&mut cut).publish_owned();
        drop(cut);
        published.finalize()
    }
}

impl RowsActivatedDependencyBatch {
    pub(super) fn bind_published(
        self,
        published: PublishedIngressVisibility,
    ) -> PublishedDependencyBatch {
        if !published.same_stage(&self.0.visibility) {
            self.0.maintenance.poison();
        }
        PublishedDependencyBatch(self.0)
    }

    pub(super) fn publish_owned(mut self) -> PublishedDependencyBatch {
        let Some(publication) = self.0.publication.take() else {
            self.0.maintenance.poison();
            return PublishedDependencyBatch(self.0);
        };
        let published = publication.publish();
        self.bind_published(published)
    }
}

impl PublishedDependencyBatch {
    pub(super) fn finalize(self) -> DependencyFinalization {
        self.0.finalize()
    }
}

impl SchedulerSealedRetainedDependency {
    pub(super) fn visibility(&self) -> &StagedIngressVisibility {
        &self.0.visibility
    }

    pub(super) fn prestate_is_fresh(&self) -> bool {
        let staged = &self.0;
        matches!(
            &staged.state,
            StagedDependencyState::Staged(delta)
                if delta.is_scheduler_sealed_retained_shape()
        ) && !staged.maintenance.is_poisoned()
            && !staged.visibility.is_visible()
            && std::sync::Arc::ptr_eq(
                &staged.generation_nonce,
                &staged.stage_bank.generation_nonce,
            )
            && staged
                .bank_permit
                .as_ref()
                .is_some_and(|permit| std::sync::Arc::ptr_eq(&permit.bank, &staged.stage_bank))
            && staged.staged_levels.is_empty()
            && staged.staged_dirty.is_empty()
            && staged.fanout_absence.is_empty()
            && staged.unindexed.is_empty()
            && staged.publication.is_none()
            && staged
                .staged_relations
                .iter()
                .all(StagedDependencyRelation::receipt_is_hidden)
    }

    pub(super) fn activate_in_cut(
        self,
        cut: &mut ShardedOwnerWriteCut<'_>,
    ) -> RowsActivatedDependencyBatch {
        self.0.activate_in_cut(cut)
    }
}

impl SealedReadyPhaseDependency {
    pub(super) fn visibility(&self) -> &StagedIngressVisibility {
        &self.stage.visibility
    }

    pub(super) fn extend_final_read_support(&self, support: &mut ShardReadSupport) {
        if let StagedDependencyState::Staged(delta) = &self.stage.state {
            support.include(delta.ready_phase_final_read_support(&self.stage.entries));
        }
    }

    pub(super) fn prestate_is_fresh(&self, cut: &ShardedOwnerWriteCut<'_>) -> bool {
        let staged = &self.stage;
        matches!(
            &staged.state,
            StagedDependencyState::Staged(delta)
                if delta.ready_phase_prestate_is_fresh_after_stage(&staged.entries, cut)
        ) && !staged.maintenance.is_poisoned()
            && !staged.visibility.is_visible()
            && std::sync::Arc::ptr_eq(
                &staged.generation_nonce,
                &staged.stage_bank.generation_nonce,
            )
            && staged
                .bank_permit
                .as_ref()
                .is_some_and(|permit| std::sync::Arc::ptr_eq(&permit.bank, &staged.stage_bank))
            && staged.staged_levels.is_empty()
            && staged.staged_dirty.is_empty()
            && staged.fanout_absence.is_empty()
            && staged.unindexed.is_empty()
            && staged.publication.is_some()
            && staged
                .staged_relations
                .iter()
                .all(StagedDependencyRelation::receipt_is_hidden)
    }

    pub(super) fn activate_in_cut(
        self,
        cut: &mut ShardedOwnerWriteCut<'_>,
    ) -> RowsActivatedDependencyBatch {
        self.stage.activate_in_cut(cut)
    }
}

struct DependencyCleanupOriginTarget {
    origin: DependencyOrigin,
    transitional: usize,
}

struct DependencyCleanupScratch {
    key_effects: Vec<(DependencyKey, isize, isize)>,
    key_targets: Vec<(DependencyOrigin, usize, usize)>,
    origin_targets: Vec<DependencyCleanupOriginTarget>,
    touched_consumer_keys: Vec<DependencyKey>,
    orphan_keys: Vec<DependencyKey>,
    eligible_orphan_keys: Vec<DependencyKey>,
}

impl DependencyCleanupScratch {
    fn try_for_relations(count: usize) -> Result<Self, DependencyStageError> {
        let mut key_effects = Vec::new();
        let mut key_targets = Vec::new();
        let mut origin_targets = Vec::new();
        let mut touched_consumer_keys = Vec::new();
        let mut orphan_keys = Vec::new();
        let mut eligible_orphan_keys = Vec::new();
        for reservation in [
            key_effects.try_reserve_exact(count),
            key_targets.try_reserve_exact(count),
            origin_targets.try_reserve_exact(count),
            touched_consumer_keys.try_reserve_exact(count),
            orphan_keys.try_reserve_exact(count),
            eligible_orphan_keys.try_reserve_exact(count),
        ] {
            reservation.map_err(|_| DependencyStageError::Allocation)?;
        }
        Ok(Self {
            key_effects,
            key_targets,
            origin_targets,
            touched_consumer_keys,
            orphan_keys,
            eligible_orphan_keys,
        })
    }
}

fn plan_staged_dependency_cleanup(
    entries: &ShardedOwnerMap,
    cut: &ShardedOwnerWriteCut<'_>,
    relations: &[StagedDependencyRelation],
    visibility: &StagedIngressVisibility,
    scratch: &mut DependencyCleanupScratch,
) -> Result<(), DependencyStageError> {
    scratch.key_effects.clear();
    scratch.key_targets.clear();
    scratch.origin_targets.clear();
    scratch.touched_consumer_keys.clear();
    if relations
        .array_windows::<2>()
        .any(|[left, right]| left.point >= right.point)
    {
        return Err(DependencyStageError::Projection);
    }
    let committed = visibility.is_visible();
    let mut remaining = relations;
    while let Some((first, tail)) = remaining.split_first() {
        let key = &first.point.key;
        let target = first.point.target;
        let same_point_len = tail.partition_point(|relation| {
            relation.point.key == *key && relation.point.target == target
        });
        let (same_point, rest) = tail.split_at(same_point_len);
        let shard = cut.projection_shard(dependency_relation_shard(entries, key));
        let set = relation_set_for_target(shard, key, target);
        let all_owned = std::iter::once(first).chain(same_point).all(|relation| {
            relation.staged_cell.as_ref().is_some_and(|staged| {
                set.is_some_and(|set| set.owns_exact_stage(&relation.point.owner, staged))
            })
        });
        if !all_owned {
            // No staged edge may be replaced before its exact synchronous
            // cleanup. A foreign Arc is therefore a structural violation,
            // not a legal successor handoff.
            return Err(DependencyStageError::Projection);
        }
        let owned = same_point
            .len()
            .checked_add(1)
            .ok_or(DependencyStageError::Projection)?;
        if set.is_some_and(|set| set.staged_len() < owned) {
            return Err(DependencyStageError::Projection);
        }
        if target.consumer_phase().is_some() && owned != 0 {
            let mut stable_delta = 0isize;
            let mut physical_delta = 0isize;
            for relation in std::iter::once(first).chain(same_point) {
                let becomes_stable = matches!(
                    (relation.action, committed),
                    (DependencyRelationAction::Insert, true)
                        | (DependencyRelationAction::Retire, false)
                );
                if becomes_stable {
                    stable_delta = stable_delta
                        .checked_add(1)
                        .ok_or(DependencyStageError::Projection)?;
                } else {
                    physical_delta = physical_delta
                        .checked_sub(1)
                        .ok_or(DependencyStageError::Projection)?;
                }
            }
            if let Some((last_key, last_stable, last_physical)) = scratch.key_effects.last_mut()
                && last_key == key
            {
                *last_stable = last_stable
                    .checked_add(stable_delta)
                    .ok_or(DependencyStageError::Projection)?;
                *last_physical = last_physical
                    .checked_add(physical_delta)
                    .ok_or(DependencyStageError::Projection)?;
            } else {
                scratch
                    .key_effects
                    .push((key.clone(), stable_delta, physical_delta));
            }
        }
        remaining = rest;
    }

    for (key, stable_delta, physical_delta) in &scratch.key_effects {
        let shard = cut.projection_shard(dependency_relation_shard(entries, key));
        let origin = key.origin();
        let row = shard
            .dependency_relations
            .get(&origin)
            .and_then(|origin| origin.key(key))
            .ok_or(DependencyStageError::Projection)?;
        let before_physical = row
            .consumers
            .physical_len()
            .ok_or(DependencyStageError::Projection)?;
        let before_stable = row
            .consumers
            .stable_len()
            .ok_or(DependencyStageError::Projection)?;
        let after_physical = checked_add_signed(before_physical, *physical_delta)
            .ok_or(DependencyStageError::Projection)?;
        let after_stable = checked_add_signed(before_stable, *stable_delta)
            .ok_or(DependencyStageError::Projection)?;
        scratch.key_targets.push((
            origin,
            usize::from(before_physical != 0 && before_stable == 0),
            usize::from(after_physical != 0 && after_stable == 0),
        ));
        if after_physical == 0 {
            scratch.touched_consumer_keys.push(key.clone());
        }
    }
    scratch
        .key_targets
        .sort_unstable_by(|left, right| left.0.cmp(&right.0));
    let mut remaining = scratch.key_targets.as_slice();
    while let Some((first, tail)) = remaining.split_first() {
        let origin = &first.0;
        let same_origin_len = tail.partition_point(|target| target.0 == *origin);
        let (same_origin, rest) = tail.split_at(same_origin_len);
        let shard = cut.projection_shard(dependency_origin_shard(entries, origin));
        let row = shard
            .dependency_relations
            .get(origin)
            .ok_or(DependencyStageError::Projection)?;
        let mut transitional = row.transitional_len();
        for (_, before, after) in std::iter::once(first).chain(same_origin) {
            transitional = transitional
                .checked_sub(*before)
                .and_then(|count| count.checked_add(*after))
                .ok_or(DependencyStageError::Projection)?;
        }
        scratch.origin_targets.push(DependencyCleanupOriginTarget {
            origin: origin.clone(),
            transitional,
        });
        remaining = rest;
    }
    scratch.touched_consumer_keys.sort_unstable();
    scratch.touched_consumer_keys.dedup();
    Ok(())
}

fn post_cleanup_physical_consumers(
    entries: &ShardedOwnerMap,
    cut: &ShardedOwnerWriteCut<'_>,
    scratch: &DependencyCleanupScratch,
    key: &DependencyKey,
) -> Result<usize, DependencyStageError> {
    let before = cut
        .projection_shard(dependency_relation_shard(entries, key))
        .dependency_relations
        .get(&key.origin())
        .and_then(|origin| origin.key(key))
        .map_or(Ok(0), |row| {
            row.consumers
                .physical_len()
                .ok_or(DependencyStageError::Projection)
        })?;
    let delta = scratch
        .key_effects
        .binary_search_by(|(candidate, _, _)| candidate.cmp(key))
        .ok()
        .and_then(|position| scratch.key_effects.get(position))
        .map_or(0, |(_, _, physical)| *physical);
    checked_add_signed(before, delta).ok_or(DependencyStageError::Projection)
}

fn set_staged_dependency_cleanup_counts(
    entries: &ShardedOwnerMap,
    cut: &mut ShardedOwnerWriteCut<'_>,
    targets: &[DependencyCleanupOriginTarget],
) -> bool {
    if targets.iter().any(|target| {
        !cut.projection_shard(dependency_origin_shard(entries, &target.origin))
            .dependency_relations
            .contains_key(&target.origin)
    }) {
        return false;
    }
    for target in targets {
        let shard = cut.projection_shard_mut(dependency_origin_shard(entries, &target.origin));
        if let Some(row) = shard.dependency_relations.get_mut(&target.origin) {
            row.transitional = target.transitional;
        }
    }
    true
}

fn preflight_staged_dependency_cleanup(
    cleanup: &mut DependencyCleanupScratch,
    staged_levels: &[StagedDependencyControl<DependencyLevel>],
    staged_dirty: &[StagedDependencyControl<DirtyDependency>],
    entries: &ShardedOwnerMap,
    cut: &ShardedOwnerWriteCut<'_>,
) -> Result<(), DependencyStageError> {
    cleanup.eligible_orphan_keys.clear();
    if staged_levels.iter().any(|staged| {
        let shard = entries
            .layout
            .router
            .shard(b"dependency/level", &staged.key);
        cut.projection_shard(shard)
            .dependency_levels
            .get(&staged.key)
            .is_none_or(|current| !current.is_exact_stage(&staged.staged_cell))
    }) || staged_dirty.iter().any(|staged| {
        let shard = entries
            .layout
            .router
            .shard(b"dependency/level", &staged.key);
        cut.projection_shard(shard)
            .dependency_dirty
            .get(&staged.key)
            .is_none_or(|current| !current.is_exact_stage(&staged.staged_cell))
    }) {
        return Err(DependencyStageError::Projection);
    }

    let mut owned_dirty = [0usize; super::shard::AUTHORITY_SHARD_COUNT];
    for staged in staged_dirty {
        let shard = entries
            .layout
            .router
            .shard(b"dependency/level", &staged.key);
        let count = owned_dirty
            .get_mut(shard)
            .ok_or(DependencyStageError::Projection)?;
        *count = count
            .checked_add(1)
            .ok_or(DependencyStageError::Projection)?;
    }
    if owned_dirty
        .iter()
        .copied()
        .enumerate()
        .any(|(shard, count)| {
            count != 0 && cut.projection_shard(shard).dependency_dirty_staged < count
        })
        || cleanup.origin_targets.iter().any(|target| {
            !cut.projection_shard(dependency_origin_shard(entries, &target.origin))
                .dependency_relations
                .contains_key(&target.origin)
        })
    {
        return Err(DependencyStageError::Projection);
    }

    for key in &cleanup.orphan_keys {
        if post_cleanup_physical_consumers(entries, cut, cleanup, key)? != 0 {
            continue;
        }
        let shard = entries.layout.router.shard(b"dependency/level", key);
        let row = cut.projection_shard(shard);
        let level = dependency_control_cleanup_status(&row.dependency_levels, staged_levels, key);
        let dirty = dependency_control_cleanup_status(&row.dependency_dirty, staged_dirty, key);
        if matches!(level, DependencyControlCleanupStatus::Foreign)
            || matches!(dirty, DependencyControlCleanupStatus::Foreign)
        {
            continue;
        }
        cleanup.eligible_orphan_keys.push(key.clone());
    }
    Ok(())
}

impl Drop for StagedDependencyBatch {
    fn drop(&mut self) {
        if matches!(&self.state, StagedDependencyState::Terminal) {
            return;
        }
        let _ignored_poisoning_terminal = self.finalize_inner(true);
    }
}

/// Remove one prevalidated owner-free key after exact staged normalization.
/// Foreign staged control ownership excludes the whole key from the prepared
/// cleanup; stable level evidence is retired exactly once.
fn prune_stable_orphan_dependency_key_in_cut(
    entries: &ShardedOwnerMap,
    cut: &mut ShardedOwnerWriteCut<'_>,
    key: &DependencyKey,
) -> bool {
    let relation_shard = dependency_relation_shard(entries, key);
    let origin = key.origin();
    let has_consumers = cut
        .projection_shard(relation_shard)
        .dependency_relations
        .get(&origin)
        .and_then(|row| row.key(key))
        .is_some_and(|row| !row.consumers.is_empty());
    if has_consumers {
        return false;
    }
    let level_shard = entries.layout.router.shard(b"dependency/level", key);
    let level_row = cut.projection_shard_mut(level_shard);
    if matches!(
        level_row.dependency_dirty.get(key),
        Some(DependencyControlCell::Staged(_))
    ) || matches!(
        level_row.dependency_levels.get(key),
        Some(DependencyControlCell::Staged(_))
    ) {
        return false;
    }
    let _dirty = level_row
        .dependency_dirty
        .remove(key)
        .and_then(DependencyControlCell::into_stable);
    let level = level_row
        .dependency_levels
        .remove(key)
        .and_then(DependencyControlCell::into_stable);
    let rows = &mut cut
        .projection_shard_mut(relation_shard)
        .dependency_relations;
    if let Some(row) = rows.get_mut(&origin) {
        if row.key(key).is_some_and(DependencyKeyRelationRow::is_empty) {
            row.keys.remove(key);
        }
        if row.is_empty() {
            rows.remove(&origin);
        }
    }
    if let Some(level) = level {
        let shard = entries.layout.router.shard(b"dependency/unindexed", key);
        let unindexed = &mut cut.projection_shard_mut(shard).dependency_unindexed;
        unindexed.last_change = Some(
            unindexed
                .last_change
                .map_or(level.last_change, |current| current.max(level.last_change)),
        );
        if let Some(loss) = level.last_definitive_loss {
            unindexed.last_definitive_loss = Some(
                unindexed
                    .last_definitive_loss
                    .map_or(loss, |current| current.max(loss)),
            );
        }
    }
    true
}

fn dependency_has_visible_accepted_consumers_in_cut(
    entries: &ShardedOwnerMap,
    cut: &ShardedOwnerWriteCut<'_>,
    key: &DependencyKey,
) -> bool {
    let shard = dependency_relation_shard(entries, key);
    let row = cut.projection_shard(shard);
    row.dependency_relations
        .get(&key.origin())
        .and_then(|origin| origin.key(key))
        .is_some_and(|row| row.consumers.accepted.has_visible_bounded().unwrap_or(true))
}

fn dependency_has_consumers_for_stage_in_cut(
    entries: &ShardedOwnerMap,
    cut: &ShardedOwnerWriteCut<'_>,
    key: &DependencyKey,
    visibility: &StagedIngressVisibility,
) -> Result<bool, DependencyStageError> {
    let shard = cut.projection_shard(dependency_relation_shard(entries, key));
    shard
        .dependency_relations
        .get(&key.origin())
        .and_then(|origin| origin.key(key))
        .map_or(Ok(false), |row| {
            row.consumers
                .has_potential_visible_for_stage_bounded(visibility)
        })
        .map_err(|_| DependencyStageError::Projection)
}

fn dependency_has_waiters_for_stage_in_cut(
    entries: &ShardedOwnerMap,
    cut: &ShardedOwnerWriteCut<'_>,
    key: &DependencyKey,
    visibility: &StagedIngressVisibility,
) -> Result<bool, DependencyStageError> {
    let shard = cut.projection_shard(dependency_relation_shard(entries, key));
    shard
        .dependency_relations
        .get(&key.origin())
        .and_then(|origin| origin.key(key))
        .map_or(Ok(false), |row| {
            row.waiters
                .has_potential_visible_for_stage_bounded(visibility)
        })
        .map_err(|_| DependencyStageError::Projection)
}

fn compile_staged_dependency_control_plan(
    frontier: &DependencyFrontier,
    delta: &DependencyBatchDelta,
    relations: &[StagedDependencyRelation],
    cut: &ShardedOwnerWriteCut<'_>,
    visibility: &StagedIngressVisibility,
) -> Result<StagedDependencyControlPlan, DependencyStageError> {
    let control_keys = match &delta.control {
        DependencyControlDelta::Event(event) => event.changes.len(),
        DependencyControlDelta::Maintenance(_) => 1,
        DependencyControlDelta::None => 0,
    };
    let mut keys = Vec::new();
    keys.try_reserve_exact(
        relations
            .len()
            .checked_add(control_keys)
            .ok_or(DependencyStageError::Projection)?,
    )
    .map_err(|_| DependencyStageError::Allocation)?;
    keys.extend(
        relations
            .iter()
            .filter(|change| change.point.target.consumer_phase().is_some())
            .map(|change| change.point.key.clone()),
    );
    match &delta.control {
        DependencyControlDelta::Event(event) => {
            keys.extend(event.changes.iter().map(|change| change.key.clone()));
        }
        DependencyControlDelta::Maintenance(maintenance) => {
            keys.push(maintenance.key().clone());
        }
        DependencyControlDelta::None => {}
    }
    keys.sort_unstable();
    keys.dedup();

    let entries = &frontier.entries;
    let mut plan = StagedDependencyControlPlan::try_for_key_count(keys.len())?;
    let event_fanout = matches!(&delta.control, DependencyControlDelta::Event(_));
    for key in keys {
        let expected = delta
            .prestate
            .keys
            .binary_search_by(|candidate| candidate.key.cmp(&key))
            .ok()
            .and_then(|position| delta.prestate.keys.get(position))
            .ok_or(DependencyStageError::Projection)?;
        let before_level = expected.level;
        let before_dirty = expected.dirty.clone();
        let mut after_level = before_level;
        let mut after_dirty = before_dirty.clone();
        let has_consumers =
            dependency_has_consumers_for_stage_in_cut(entries, cut, &key, visibility)?;
        let mut no_consumers = false;
        let mut no_waiters = false;
        let first_relation = relations.partition_point(|change| change.point.key < key);
        let remaining_relations = relations
            .get(first_relation..)
            .ok_or(DependencyStageError::Projection)?;
        let relation_changed = remaining_relations
            .iter()
            .take_while(|change| change.point.key == key)
            .any(|change| change.point.target.consumer_phase().is_some());
        if relation_changed && !has_consumers {
            no_consumers = event_fanout;
            if let Some(level) = after_level.take() {
                plan.push_unindexed(entries, &key, level);
            }
            after_dirty = None;
        }

        match &delta.control {
            DependencyControlDelta::Event(event) => {
                if let Some(change) = event
                    .changes
                    .binary_search_by(|change| change.key.cmp(&key))
                    .ok()
                    .and_then(|position| event.changes.get(position))
                {
                    if !has_consumers {
                        no_consumers = true;
                        plan.push_unindexed(entries, &key, change.level);
                        if let Some(level) = after_level.take() {
                            plan.push_unindexed(entries, &key, level);
                        }
                        after_dirty = None;
                    } else {
                        after_level = Some(change.level);
                        if let Some(dirty) = after_dirty.as_mut() {
                            dirty.pending = Some(match dirty.pending {
                                Some(pending) => PendingDependency {
                                    target: std::cmp::max(pending.target, change.level.last_change),
                                    scope: pending.scope.merge(change.scope),
                                },
                                None => PendingDependency {
                                    target: change.level.last_change,
                                    scope: change.scope,
                                },
                            });
                        } else {
                            let has_target = match change.scope {
                                DirtyScope::ExistingWaiters => {
                                    dependency_has_waiters_for_stage_in_cut(
                                        entries, cut, &key, visibility,
                                    )?
                                }
                                DirtyScope::AllConsumers => true,
                            };
                            if has_target {
                                after_dirty = Some(DirtyDependency {
                                    target: change.level.last_change,
                                    scope: change.scope,
                                    cursor: None,
                                    pending: None,
                                });
                            } else if change.scope == DirtyScope::ExistingWaiters {
                                no_waiters = true;
                            }
                        }
                    }
                }
            }
            DependencyControlDelta::Maintenance(maintenance) if maintenance.key() == &key => {
                match &maintenance.step {
                    DependencyMaintenanceStep::Advance {
                        expected, cursor, ..
                    } => {
                        if has_consumers {
                            let mut next = expected.clone();
                            next.cursor = Some(cursor.clone());
                            after_dirty = Some(next);
                        } else {
                            if let Some(level) = after_level.take() {
                                plan.push_unindexed(entries, &key, level);
                            }
                            after_dirty = None;
                        }
                    }
                    DependencyMaintenanceStep::Complete { expected, .. } => {
                        if let Some(PendingDependency { target, scope }) = expected.pending {
                            after_dirty = Some(DirtyDependency {
                                target,
                                scope,
                                cursor: None,
                                pending: None,
                            });
                        } else {
                            after_dirty = None;
                            if !has_consumers && let Some(level) = after_level.take() {
                                plan.push_unindexed(entries, &key, level);
                            }
                        }
                    }
                }
                plan.cursor = MaintenanceCursorTail::SetAfterCount(key.clone());
            }
            DependencyControlDelta::None | DependencyControlDelta::Maintenance(_) => {}
        }

        if event_fanout && (no_consumers || no_waiters) {
            plan.fanout_absence.push(StagedDependencyFanoutAbsence {
                key: key.clone(),
                no_consumers,
                no_waiters,
            });
        }

        if before_level != after_level {
            plan.levels.push(StagedDependencyControl::new(
                key.clone(),
                before_level,
                after_level,
                visibility,
            ));
        }
        if before_dirty != after_dirty {
            plan.dirty.push(StagedDependencyControl::new(
                key,
                before_dirty,
                after_dirty,
                visibility,
            ));
        }
    }
    plan.canonicalize_unindexed();
    Ok(plan)
}

fn stage_dependency_control_plan(
    frontier: &DependencyFrontier,
    cut: &mut ShardedOwnerWriteCut<'_>,
    bank_permit: &mut DependencyStageBankPermit,
    plan: &StagedDependencyControlPlan,
) -> Result<(), DependencyStageError> {
    let additional = plan
        .levels
        .len()
        .checked_add(plan.dirty.len())
        .ok_or(DependencyStageError::Capacity)?;
    bank_permit.try_grow(additional)?;
    let entries = &frontier.entries;
    if plan.levels.iter().any(|staged| {
        let shard = entries
            .layout
            .router
            .shard(b"dependency/level", &staged.key);
        !control_cell_matches_before(&cut.projection_shard(shard).dependency_levels, staged)
    }) || plan.dirty.iter().any(|staged| {
        let shard = entries
            .layout
            .router
            .shard(b"dependency/level", &staged.key);
        !control_cell_matches_before(&cut.projection_shard(shard).dependency_dirty, staged)
    }) {
        return Err(DependencyStageError::Stale);
    }

    let mut dirty_additions = [0usize; super::shard::AUTHORITY_SHARD_COUNT];
    for staged in &plan.dirty {
        let shard = entries
            .layout
            .router
            .shard(b"dependency/level", &staged.key);
        let Some(additions) = dirty_additions.get_mut(shard) else {
            return Err(DependencyStageError::Projection);
        };
        *additions = additions
            .checked_add(1)
            .ok_or(DependencyStageError::Projection)?;
    }
    for (shard, additions) in dirty_additions.iter().copied().enumerate() {
        if additions != 0
            && cut
                .projection_shard(shard)
                .dependency_dirty_staged
                .checked_add(additions)
                .is_none()
        {
            return Err(DependencyStageError::Projection);
        }
    }

    for staged in &plan.levels {
        let shard = entries
            .layout
            .router
            .shard(b"dependency/level", &staged.key);
        install_control_cell_prechecked(
            &mut cut.projection_shard_mut(shard).dependency_levels,
            staged,
        );
    }
    for staged in &plan.dirty {
        let shard = entries
            .layout
            .router
            .shard(b"dependency/level", &staged.key);
        let row = cut.projection_shard_mut(shard);
        install_control_cell_prechecked(&mut row.dependency_dirty, staged);
        row.dependency_dirty_staged = row
            .dependency_dirty_staged
            .checked_add(1)
            .ok_or(DependencyStageError::Projection)?;
    }
    Ok(())
}

impl DependencyMaintenanceTicket {
    pub(super) fn key(&self) -> &DependencyKey {
        &self.key
    }

    pub(super) fn hash(&self) -> Option<&RawTxHash> {
        self.hash.as_ref()
    }

    pub(super) fn action(
        &self,
        owner: Option<&OwnedTx>,
        evidence: Option<&SettlementDependencyEvidence>,
    ) -> Result<DependencyMaintenanceAction, DependencyError> {
        let Some(hash) = &self.hash else {
            return Ok(DependencyMaintenanceAction::Advance);
        };
        let owner = owner.ok_or(DependencyError::Projection)?;
        if &owner.record().identity.raw != hash {
            return Err(DependencyError::Projection);
        }
        let entry = match owner {
            OwnedTx::PreAccepted(entry) => entry,
            OwnedTx::Accepted(entry) => {
                match self.scope {
                    DirtyScope::ExistingWaiters => {}
                    DirtyScope::AllConsumers => {
                        if self
                            .last_definitive_loss
                            .is_some_and(|loss| entry.proof.dependency_cut() < loss)
                        {
                            return Err(DependencyError::SurvivingAcceptedConsumer);
                        }
                    }
                }
                return Ok(DependencyMaintenanceAction::Advance);
            }
            OwnedTx::ReplacementHistory(history) => {
                if !history.dependencies().contains(&self.key) {
                    return Err(DependencyError::Projection);
                }
                // A replacement victim may have several blockers. A level
                // change on one blocker is only a prompt to re-evaluate the
                // complete observed set; consuming history at the first free
                // input would lose it if a newer winner still spent another.
                // Every observed key was proven unavailable at the cohort cut,
                // so only a newer final Availability level satisfies it.
                return Ok(
                    if history.observation().contains(&self.key)
                        && evidence.is_some_and(|evidence| {
                            evidence.owner == *hash
                                && evidence
                                    .all_observed_dependencies_available(history.observation())
                        })
                    {
                        DependencyMaintenanceAction::Requeue
                    } else {
                        DependencyMaintenanceAction::Advance
                    },
                );
            }
        };
        if !entry.dependencies().contains(&self.key) {
            return Err(DependencyError::Projection);
        }
        match self.scope {
            DirtyScope::ExistingWaiters => match &entry.phase {
                PreAcceptedPhase::Waiting(observed) => Ok(
                    if observed.contains(&self.key) && observed.dependency_cut() < self.target {
                        DependencyMaintenanceAction::Requeue
                    } else {
                        DependencyMaintenanceAction::Advance
                    },
                ),
                PreAcceptedPhase::Queued(_)
                | PreAcceptedPhase::Computing(_)
                | PreAcceptedPhase::Ready(_) => Ok(DependencyMaintenanceAction::Advance),
            },
            DirtyScope::AllConsumers => {
                let loss = self
                    .last_definitive_loss
                    .ok_or(DependencyError::Projection)?;
                let stale = match &entry.phase {
                    PreAcceptedPhase::Queued(QueuedWork::Resolve)
                    | PreAcceptedPhase::Computing(_) => false,
                    PreAcceptedPhase::Queued(QueuedWork::Verify(resolved)) => {
                        resolved.dependency_cut() < loss
                    }
                    PreAcceptedPhase::Waiting(observed) => {
                        // `AllConsumers` may represent a coalesced loss followed
                        // by a newer availability change. Non-waiting proof is
                        // invalidated only by `loss`, while a waiter must observe
                        // every later level change represented by `target`.
                        observed.dependency_cut() < self.target
                    }
                    PreAcceptedPhase::Ready(verified) => verified.dependency_cut() < loss,
                };
                Ok(if stale {
                    DependencyMaintenanceAction::Requeue
                } else {
                    DependencyMaintenanceAction::Advance
                })
            }
        }
    }
}

impl DependencyMaintenancePlan {
    fn key(&self) -> &DependencyKey {
        match &self.step {
            DependencyMaintenanceStep::Advance { key, .. }
            | DependencyMaintenanceStep::Complete { key, .. } => key,
        }
    }

    fn expected(&self) -> &DirtyDependency {
        match &self.step {
            DependencyMaintenanceStep::Advance { expected, .. }
            | DependencyMaintenanceStep::Complete { expected, .. } => expected,
        }
    }
}

impl DependencySlot {
    fn from_owner(owner: &OwnedTx) -> Result<Self, DependencyError> {
        let (dependencies, waiting, phase) = match owner {
            OwnedTx::PreAccepted(entry) => {
                let waiting = match &entry.phase {
                    PreAcceptedPhase::Waiting(observed) => Some(observed.clone()),
                    PreAcceptedPhase::Queued(_)
                    | PreAcceptedPhase::Computing(_)
                    | PreAcceptedPhase::Ready(_) => None,
                };
                (
                    entry.dependencies().clone(),
                    waiting,
                    DependencyConsumerPhase::Other,
                )
            }
            OwnedTx::Accepted(entry) => (
                entry.proof.payload().dependencies().clone(),
                None,
                DependencyConsumerPhase::Accepted,
            ),
            OwnedTx::ReplacementHistory(entry) => (
                entry.dependencies().clone(),
                Some(entry.observation().clone()),
                DependencyConsumerPhase::Other,
            ),
        };
        if waiting.as_ref().is_some_and(|observed| {
            observed
                .keys()
                .any(|key| dependencies.keys().binary_search(key).is_err())
        }) {
            return Err(DependencyError::Projection);
        }
        Ok(Self {
            hash: owner.record().identity.raw.clone(),
            phase,
            dependencies,
            waiting,
        })
    }
}

impl DependencyFrontier {
    pub(super) fn observe_missing(
        &self,
        missing: &MissingDependencies,
        retained: KnownDependencies,
        dependency_cut: DependencyCut,
    ) -> ObservedDependencies {
        ObservedDependencies::from_missing(missing, retained, dependency_cut)
    }

    pub(super) fn keys_for_origin(
        &self,
        origin: &DependencyOrigin,
    ) -> Result<Option<BTreeSet<DependencyKey>>, DependencyError> {
        self.origin_keys(origin)
    }

    pub(super) fn classify_empty_origin(
        &self,
        origin: &DependencyOrigin,
    ) -> Result<(bool, Option<BTreeSet<DependencyKey>>), DependencyError> {
        Ok(match self.origin_keys(origin)? {
            None => (false, None),
            Some(keys) if keys.is_empty() => (false, Some(BTreeSet::new())),
            Some(_) => (true, None),
        })
    }

    pub(super) fn consumers_for(
        &self,
        key: &DependencyKey,
    ) -> Result<Option<BTreeSet<RawTxHash>>, DependencyError> {
        self.consumers(key)
    }

    pub(super) fn has_waiter_outside(
        &self,
        key: &DependencyKey,
        removed: &[RawTxHash],
    ) -> Result<bool, DependencyError> {
        Ok(self
            .waiters(key)?
            .is_some_and(|waiters| waiters.iter().any(|owner| !removed.contains(owner))))
    }

    pub(super) fn capture_settlement_evidence(
        &self,
        owner: &RawTxHash,
        baseline: &KnownDependencies,
        candidate: Option<&KnownDependencies>,
        missing: Option<&MissingDependencies>,
    ) -> Result<SettlementDependencyEvidence, DependencyError> {
        let capacity = baseline
            .len()
            .checked_add(candidate.map_or(0, KnownDependencies::len))
            .and_then(|count| count.checked_add(missing.map_or(0, MissingDependencies::len)))
            .ok_or(DependencyError::Projection)?;
        let mut keys = Vec::new();
        keys.try_reserve(capacity)
            .map_err(|_| DependencyError::Allocation)?;
        keys.extend(baseline.keys().iter().cloned());
        if let Some(candidate) = candidate {
            keys.extend(candidate.keys().iter().cloned());
        }
        if let Some(missing) = missing {
            keys.extend(missing.keys().iter().cloned());
        }
        keys.sort_unstable();
        keys.dedup();

        let mut support = ShardReadSupport::default();
        for key in &keys {
            support.insert(dependency_relation_shard(&self.entries, key));
            support.insert(self.shard(b"dependency/level", key));
            support.insert(self.shard(b"dependency/unindexed", key));
        }
        let mut evidence = Vec::new();
        evidence
            .try_reserve_exact(keys.len())
            .map_err(|_| DependencyError::Allocation)?;
        let cut = self
            .entries
            .mixed_cut(support, ShardWriteSupport::default());
        let mut visibility = DependencyVisibilityReceipt::default();
        for key in keys {
            let consumer_shard = dependency_relation_shard(&self.entries, &key);
            let consumer_row = cut.projection_shard(consumer_shard);
            let origin = key.origin();
            let origin_row = consumer_row.dependency_relations.get(&origin);
            let key_row = origin_row.and_then(|row| row.key(&key));
            let owner_phase = key_row.map_or(Ok(None), |row| {
                row.consumers.observe_visible_phase(owner, &mut visibility)
            })?;
            if owner_phase.is_some() != baseline.contains(&key) {
                return Err(DependencyError::Projection);
            }
            if baseline.contains(&key) {
                let visible = match (origin_row, key_row) {
                    (Some(origin_row), Some(key_row)) => dependency_origin_key_is_visible_observed(
                        origin_row,
                        &key,
                        key_row,
                        &mut visibility,
                    )?,
                    (None, None) | (Some(_), None) | (None, Some(_)) => false,
                };
                if !visible {
                    return Err(DependencyError::Projection);
                }
            }
            let level_shard = self.shard(b"dependency/level", &key);
            let level_row = cut.projection_shard(level_shard);
            let level = match level_row.dependency_levels.get(&key) {
                Some(cell) => visibility.observe_control(cell)?.copied(),
                None => None,
            };
            let dirty = match level_row.dependency_dirty.get(&key) {
                Some(cell) => visibility.observe_control(cell)?.cloned(),
                None => None,
            };
            let unindexed_shard = self.shard(b"dependency/unindexed", &key);
            let unindexed = cut.projection_shard(unindexed_shard).dependency_unindexed;
            evidence.push(SettlementDependencyKeyEvidence {
                key,
                level,
                dirty,
                unindexed,
                owner_phase,
            });
        }
        if !visibility.is_current() {
            return Err(DependencyError::Stale);
        }
        Ok(SettlementDependencyEvidence {
            owner: owner.clone(),
            keys: evidence,
        })
    }

    pub(super) fn proof_is_current(
        &self,
        dependencies: &KnownDependencies,
        cut: DependencyCut,
    ) -> bool {
        dependencies.keys().iter().all(|key| {
            self.level(key)
                .and_then(|level| level.last_definitive_loss)
                .is_none_or(|loss| loss <= cut)
        })
    }

    /// Validate evidence produced before the transaction owned a dependency
    /// slot. Fixed per-shard unindexed fences retain losses without forcing
    /// unrelated dependency keys through one global scalar.
    pub(super) fn owner_free_proof_is_current(
        &self,
        dependencies: &KnownDependencies,
        cut: DependencyCut,
    ) -> bool {
        self.proof_is_current(dependencies, cut)
            && dependencies.keys().iter().all(|key| {
                self.unindexed_level(key)
                    .last_definitive_loss
                    .is_none_or(|loss| loss <= cut)
            })
    }

    #[cfg(test)]
    pub(super) fn resolution_is_current(
        &self,
        baseline: &KnownDependencies,
        resolved: &KnownDependencies,
        cut: DependencyCut,
    ) -> bool {
        self.proof_is_current(resolved, cut)
            && resolved.keys().iter().all(|key| {
                baseline.keys().binary_search(key).is_ok()
                    || self
                        .unindexed_level(key)
                        .last_definitive_loss
                        .is_none_or(|loss| loss <= cut)
            })
    }

    #[cfg(test)]
    pub(super) fn missing_result_is_current(
        &self,
        baseline: &KnownDependencies,
        dependencies: &KnownDependencies,
        missing: &MissingDependencies,
        cut: DependencyCut,
    ) -> bool {
        self.resolution_is_current(baseline, dependencies, cut)
            && self.missing_observation_is_current(baseline, missing, cut)
    }

    #[cfg(test)]
    pub(super) fn missing_observation_is_current(
        &self,
        baseline: &KnownDependencies,
        missing: &MissingDependencies,
        cut: DependencyCut,
    ) -> bool {
        self.proof_is_current(baseline, cut)
            && missing.keys().iter().all(|key| {
                self.level(key).is_none_or(|level| {
                    level.last_change <= cut
                        && level.last_definitive_loss.is_none_or(|loss| loss <= cut)
                })
            })
            && missing.keys().iter().all(|key| {
                baseline.keys().binary_search(key).is_ok()
                    || self
                        .unindexed_level(key)
                        .last_change
                        .is_none_or(|change| change <= cut)
            })
    }

    /// Compile availability and definitive loss from one projected final
    /// state. A key cannot be both; callers must resolve that contradiction
    /// before publishing the level transition.
    pub(super) fn plan_events(
        &self,
        mut available: Vec<DependencyKey>,
        mut lost: Vec<DependencyKey>,
        cut: DependencyCut,
    ) -> Result<Option<DependencyEntryControlDelta>, DependencyError> {
        available.sort_unstable();
        available.dedup();
        lost.sort_unstable();
        lost.dedup();
        if available.iter().any(|key| lost.binary_search(key).is_ok()) {
            return Err(DependencyError::Projection);
        }
        let change_count = available
            .len()
            .checked_add(lost.len())
            .ok_or(DependencyError::Projection)?;
        if change_count == 0 {
            return Ok(None);
        }
        let mut changes = Vec::new();
        changes
            .try_reserve(change_count)
            .map_err(|_| DependencyError::Allocation)?;
        for (key, definitive_loss) in available
            .into_iter()
            .map(|key| (key, false))
            .chain(lost.into_iter().map(|key| (key, true)))
        {
            let previous = self.level(&key);
            if previous.is_some_and(|level| level.last_change >= cut) {
                return Err(DependencyError::Projection);
            }
            let (last_definitive_loss, scope) = if definitive_loss {
                (Some(cut), DirtyScope::AllConsumers)
            } else {
                (
                    previous.and_then(|level| level.last_definitive_loss),
                    DirtyScope::ExistingWaiters,
                )
            };
            changes.push(DependencyEventChange {
                key,
                expected_level: previous,
                level: DependencyLevel {
                    last_change: cut,
                    last_definitive_loss,
                },
                scope,
            });
        }
        changes.sort_unstable_by(|left, right| left.key.cmp(&right.key));
        Ok(Some(DependencyEntryControlDelta::Event(
            DependencyEventPlan {
                changes,
                origins: Vec::new(),
            },
        )))
    }

    pub(super) fn plan_events_with_origin_expectation(
        &self,
        available: Vec<DependencyKey>,
        lost: Vec<DependencyKey>,
        cut: DependencyCut,
        origin: DependencyOrigin,
        keys: Option<BTreeSet<DependencyKey>>,
    ) -> Result<Option<DependencyEntryControlDelta>, DependencyError> {
        let mut control = self.plan_events(available, lost, cut)?;
        if let Some(DependencyEntryControlDelta::Event(event)) = &mut control {
            event
                .origins
                .push(DependencyOriginExpectation { origin, keys });
        }
        Ok(control)
    }

    #[cfg(test)]
    pub(super) fn maintenance_pending(&self) -> bool {
        !self.dirty_is_empty()
    }

    pub(super) fn next_maintenance(
        &self,
    ) -> Result<Option<DependencyMaintenanceTicket>, DependencyError> {
        let Some(key) = self.next_dirty_key()? else {
            return Ok(None);
        };
        let dirty = self.dirty(&key).ok_or(DependencyError::Stale)?;
        let next = self.next_visible_owner(&key, dirty.scope, dirty.cursor.as_ref())?;
        Ok(Some(DependencyMaintenanceTicket {
            key: key.clone(),
            hash: next,
            target: dirty.target,
            scope: dirty.scope,
            last_definitive_loss: self
                .level(&key)
                .and_then(|level| level.last_definitive_loss),
            expected: dirty,
        }))
    }

    pub(super) fn maintenance_ticket_is_current(
        &self,
        ticket: &DependencyMaintenanceTicket,
    ) -> bool {
        if self.dirty(&ticket.key).as_ref() != Some(&ticket.expected)
            || self
                .level(&ticket.key)
                .and_then(|level| level.last_definitive_loss)
                != ticket.last_definitive_loss
        {
            return false;
        }
        self.next_visible_owner(&ticket.key, ticket.scope, ticket.expected.cursor.as_ref())
            .is_ok_and(|next| next == ticket.hash)
    }

    pub(super) fn plan_maintenance(
        &self,
        ticket: DependencyMaintenanceTicket,
    ) -> Result<DependencyMaintenancePlan, DependencyError> {
        if self.dirty(&ticket.key).as_ref() != Some(&ticket.expected) {
            return Err(DependencyError::Stale);
        }
        let step = match ticket.hash {
            Some(hash) => DependencyMaintenanceStep::Advance {
                key: ticket.key,
                expected: ticket.expected,
                cursor: hash,
            },
            None => DependencyMaintenanceStep::Complete {
                key: ticket.key,
                expected: ticket.expected,
            },
        };
        Ok(DependencyMaintenancePlan { step })
    }

    pub(super) fn seal_shared_maintenance(
        &self,
        maintenance: DependencyMaintenancePlan,
    ) -> Result<DependencyBatchDelta, DependencyError> {
        DependencyDelta {
            before: None,
            after: None,
            observed: None,
            control: DependencyEntryControlDelta::None,
        }
        .into_shared_maintenance_batch(self, maintenance, None)
    }

    #[cfg(test)]
    pub(super) fn seal_shared_control_for_foundation(
        &self,
        control: DependencyEntryControlDelta,
    ) -> Result<DependencyBatchDelta, DependencyError> {
        DependencyDelta {
            before: None,
            after: None,
            observed: None,
            control,
        }
        .into_shared_batch(self, None)
    }

    pub(super) fn plan_replace(
        &self,
        before: Option<&OwnedTx>,
        after: Option<&OwnedTx>,
    ) -> Result<DependencyDelta, DependencyError> {
        let before = before.map(DependencySlot::from_owner).transpose()?;
        let after = after.map(DependencySlot::from_owner).transpose()?;
        if before.as_ref().is_some_and(|slot| !self.contains(slot)) {
            return Err(DependencyError::Projection);
        }
        if before == after {
            // Phase/version changes commonly retain the exact dependency
            // footprint. Carry the unchanged slot as final-cut observation so
            // settlement evidence remains bound without encoding a physical
            // detach+attach or adding B-tree work.
            return Ok(DependencyDelta {
                before: None,
                after: None,
                observed: before,
                control: DependencyEntryControlDelta::default(),
            });
        }
        Ok(DependencyDelta {
            before,
            after,
            observed: None,
            control: DependencyEntryControlDelta::default(),
        })
    }

    pub(super) fn plan_replacements<'entry>(
        &self,
        changes: impl IntoIterator<Item = (Option<&'entry OwnedTx>, Option<&'entry OwnedTx>)>,
    ) -> Result<DependencyBatchDelta, DependencyError> {
        self.plan_replacements_with_additions(changes, VacancyPolicy::ExistingOwnersOnly)
    }

    pub(super) fn plan_settlement_replacements<'entry>(
        &self,
        changes: impl IntoIterator<Item = (Option<&'entry OwnedTx>, Option<&'entry OwnedTx>)>,
        evidence: Vec<SettlementDependencyEvidence>,
    ) -> Result<DependencyBatchDelta, DependencyError> {
        self.plan_replacements(changes)?
            .seal_settlement_evidence(evidence, self)
    }

    /// Compile a batch that may introduce a new primary owner. The authority
    /// caller must prove every addition vacant in its sole owner map before
    /// invoking this projection compiler. Chain recovery and synchronous
    /// direct admission are the only current callers.
    pub(super) fn plan_primary_replacements<'entry>(
        &self,
        changes: impl IntoIterator<Item = (Option<&'entry OwnedTx>, Option<&'entry OwnedTx>)>,
    ) -> Result<DependencyBatchDelta, DependencyError> {
        self.plan_replacements_with_additions(changes, VacancyPolicy::PrimaryVacancyProven)
    }

    fn plan_replacements_with_additions<'entry>(
        &self,
        changes: impl IntoIterator<Item = (Option<&'entry OwnedTx>, Option<&'entry OwnedTx>)>,
        vacancy: VacancyPolicy,
    ) -> Result<DependencyBatchDelta, DependencyError> {
        let mut input = changes.into_iter();
        let mut changes = Vec::new();
        if let Some(capacity) = input.size_hint().1 {
            changes
                .try_reserve_exact(capacity)
                .map_err(|_| DependencyError::Allocation)?;
        }
        let mut observed = Vec::new();
        if let Some(capacity) = input.size_hint().1 {
            observed
                .try_reserve_exact(capacity)
                .map_err(|_| DependencyError::Allocation)?;
        }
        for (before, after) in input.by_ref() {
            let before = before.map(DependencySlot::from_owner).transpose()?;
            let after = after.map(DependencySlot::from_owner).transpose()?;
            if before == after {
                if let Some(slot) = before {
                    if !self.contains(&slot) {
                        return Err(DependencyError::Projection);
                    }
                    if observed.len() == observed.capacity() {
                        observed
                            .try_reserve(1)
                            .map_err(|_| DependencyError::Allocation)?;
                    }
                    observed.push(slot);
                }
                continue;
            }
            if changes.len() == changes.capacity() {
                changes
                    .try_reserve(1)
                    .map_err(|_| DependencyError::Allocation)?;
            }
            changes.push((before, after));
        }
        let mut removed = Vec::new();
        let mut added = Vec::new();
        removed
            .try_reserve(changes.len())
            .map_err(|_| DependencyError::Allocation)?;
        added
            .try_reserve(changes.len())
            .map_err(|_| DependencyError::Allocation)?;
        for (before, after) in changes {
            if let Some(before) = before {
                if !self.contains(&before) {
                    return Err(DependencyError::Projection);
                }
                removed.push(before);
            }
            if let Some(after) = after {
                added.push(after);
            }
        }
        removed.sort_unstable_by(|left, right| left.hash.cmp(&right.hash));
        added.sort_unstable_by(|left, right| left.hash.cmp(&right.hash));
        observed.sort_unstable_by(|left, right| left.hash.cmp(&right.hash));
        if removed
            .array_windows::<2>()
            .any(|[left, right]| left.hash == right.hash)
            || added
                .array_windows::<2>()
                .any(|[left, right]| left.hash == right.hash)
            || observed
                .array_windows::<2>()
                .any(|[left, right]| left.hash == right.hash)
        {
            return Err(DependencyError::Projection);
        }
        // This compiler accepts only replacements/removals of primary owners.
        // Requiring every added identity to have an exact `before` proof makes
        // accidental duplicate attachment unrepresentable without scanning the
        // complete reverse index under the authority guard. A future bulk
        // admission/chain-generation API must carry its own typed vacancy proof.
        match vacancy {
            VacancyPolicy::ExistingOwnersOnly => {
                if added.iter().any(|slot| {
                    removed
                        .binary_search_by(|removed| removed.hash.cmp(&slot.hash))
                        .is_err()
                }) {
                    return Err(DependencyError::Projection);
                }
            }
            VacancyPolicy::PrimaryVacancyProven => {}
        }
        DependencyBatchDelta {
            removed,
            added,
            observed,
            unchanged: Vec::new(),
            relation_changes: Vec::new(),
            settlement_evidence: Vec::new(),
            control: DependencyControlDelta::default(),
            prestate: DependencyBatchPrestate::default(),
        }
        .seal_prestate(self)
    }

    fn contains(&self, slot: &DependencySlot) -> bool {
        slot.dependencies.keys().iter().all(|key| {
            self.consumer_contains(key, &slot.hash) && self.origin_contains(&key.origin(), key)
        }) && slot.waiting.as_ref().is_none_or(|observed| {
            observed
                .keys()
                .all(|key| self.waiter_contains(key, &slot.hash))
        })
    }

    #[cfg(test)]
    fn attach(&self, slot: &DependencySlot) {
        for key in slot.dependencies.keys() {
            let origin = key.origin();
            self.routed_shard(dependency_origin_shard(&self.entries, &origin))
                .write()
                .dependency_relations
                .entry(origin)
                .or_default()
                .stable_insert(
                    key.clone(),
                    DependencyRelationTarget::consumer(slot.phase),
                    slot.hash.clone(),
                );
        }
        if let Some(observed) = &slot.waiting {
            for key in observed.keys() {
                let origin = key.origin();
                self.routed_shard(dependency_origin_shard(&self.entries, &origin))
                    .write()
                    .dependency_relations
                    .entry(origin)
                    .or_default()
                    .stable_insert(
                        key.clone(),
                        DependencyRelationTarget::Waiter,
                        slot.hash.clone(),
                    );
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/support/dependency.rs"]
pub(in crate::authority) mod test_support;
