use super::state::{
    DependencyCut, DependencyKey, DependencyOrigin, KnownDependencies, MissingDependencies,
    ObservedDependencies, OwnedTx, PreAcceptedPhase, QueuedWork, RawTxHash,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Bound::{Excluded, Unbounded},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DependencyLevel {
    last_change: DependencyCut,
    last_definitive_loss: Option<DependencyCut>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct UnindexedDependencyLevel {
    last_change: Option<DependencyCut>,
    last_definitive_loss: Option<DependencyCut>,
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

    fn requires_definitive_loss(self) -> bool {
        match self {
            Self::ExistingWaiters => false,
            Self::AllConsumers => true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DirtyDependency {
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
    dependencies: KnownDependencies,
    waiting: Option<ObservedDependencies>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DependencySnapshot {
    consumers: BTreeMap<DependencyKey, BTreeSet<RawTxHash>>,
    waiters: BTreeMap<DependencyKey, BTreeSet<RawTxHash>>,
    keys_by_origin: BTreeMap<DependencyOrigin, BTreeSet<DependencyKey>>,
    levels: BTreeMap<DependencyKey, DependencyLevel>,
    dirty: BTreeMap<DependencyKey, DirtyDependency>,
    dirty_cursor: Option<DependencyKey>,
    unindexed: UnindexedDependencyLevel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DependencyError {
    Projection,
    Allocation,
    SurvivingAcceptedConsumer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DependencyEvent {
    Availability(DependencyCut),
    DefinitiveLoss(DependencyCut),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DependencyMaintenanceAction {
    Advance,
    Requeue,
}

struct DependencyEventChange {
    key: DependencyKey,
    level: DependencyLevel,
    scope: DirtyScope,
}

pub(super) struct DependencyEventPlan {
    changes: Vec<DependencyEventChange>,
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

pub(super) struct DependencyMaintenancePlan(DependencyMaintenanceStep);

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

pub(super) struct DependencyDelta {
    before: Option<DependencySlot>,
    after: Option<DependencySlot>,
    control: DependencyControlDelta,
}

pub(super) struct DependencyBatchDelta {
    removed: Vec<DependencySlot>,
    added: Vec<DependencySlot>,
    control: DependencyControlDelta,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VacancyPolicy {
    ExistingOwnersOnly,
    PrimaryVacancyProven,
}

#[derive(Debug, Default)]
pub(super) struct DependencyFrontier {
    consumers: BTreeMap<DependencyKey, BTreeSet<RawTxHash>>,
    waiters: BTreeMap<DependencyKey, BTreeSet<RawTxHash>>,
    keys_by_origin: BTreeMap<DependencyOrigin, BTreeSet<DependencyKey>>,
    levels: BTreeMap<DependencyKey, DependencyLevel>,
    dirty: BTreeMap<DependencyKey, DirtyDependency>,
    dirty_cursor: Option<DependencyKey>,
    unindexed: UnindexedDependencyLevel,
}

impl DependencyDelta {
    pub(super) fn with_control(mut self, control: DependencyControlDelta) -> Self {
        self.control = control;
        self
    }
}

impl DependencyBatchDelta {
    pub(super) fn with_control(mut self, control: DependencyControlDelta) -> Self {
        self.control = control;
        self
    }
}

impl DependencyMaintenanceTicket {
    pub(super) fn hash(&self) -> Option<&RawTxHash> {
        self.hash.as_ref()
    }

    pub(super) fn action(
        &self,
        frontier: &DependencyFrontier,
        owner: Option<&OwnedTx>,
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
                        && frontier.all_observed_dependencies_available(history.observation())
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

impl DependencySlot {
    fn from_owner(owner: &OwnedTx) -> Result<Self, DependencyError> {
        let (dependencies, waiting) = match owner {
            OwnedTx::PreAccepted(entry) => {
                let waiting = match &entry.phase {
                    PreAcceptedPhase::Waiting(observed) => Some(observed.clone()),
                    PreAcceptedPhase::Queued(_)
                    | PreAcceptedPhase::Computing(_)
                    | PreAcceptedPhase::Ready(_) => None,
                };
                (entry.dependencies().clone(), waiting)
            }
            OwnedTx::Accepted(entry) => (entry.proof.payload().dependencies().clone(), None),
            OwnedTx::ReplacementHistory(entry) => (
                entry.dependencies().clone(),
                Some(entry.observation().clone()),
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
            dependencies,
            waiting,
        })
    }
}

impl DependencyFrontier {
    fn all_observed_dependencies_available(&self, observed: &ObservedDependencies) -> bool {
        observed.keys().all(|key| {
            self.levels.get(key).is_some_and(|level| {
                observed.dependency_cut() < level.last_change
                    && level
                        .last_definitive_loss
                        .is_none_or(|loss| loss < level.last_change)
            })
        })
    }

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
    ) -> Option<&BTreeSet<DependencyKey>> {
        self.keys_by_origin.get(origin)
    }

    pub(super) fn consumers_for(&self, key: &DependencyKey) -> Option<&BTreeSet<RawTxHash>> {
        self.consumers.get(key)
    }

    pub(super) fn proof_is_current(
        &self,
        dependencies: &KnownDependencies,
        cut: DependencyCut,
    ) -> bool {
        dependencies.keys().iter().all(|key| {
            self.levels
                .get(key)
                .and_then(|level| level.last_definitive_loss)
                .is_none_or(|loss| loss <= cut)
        })
    }

    pub(super) fn resolution_is_current(
        &self,
        baseline: &KnownDependencies,
        resolved: &KnownDependencies,
        cut: DependencyCut,
    ) -> bool {
        self.proof_is_current(resolved, cut)
            && (!Self::has_new_dependencies(baseline, resolved)
                || self
                    .unindexed
                    .last_definitive_loss
                    .is_none_or(|loss| loss <= cut))
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

    pub(super) fn missing_observation_is_current(
        &self,
        baseline: &KnownDependencies,
        missing: &MissingDependencies,
        cut: DependencyCut,
    ) -> bool {
        self.proof_is_current(baseline, cut)
            && missing.keys().iter().all(|key| {
                self.levels.get(key).is_none_or(|level| {
                    level.last_change <= cut
                        && level.last_definitive_loss.is_none_or(|loss| loss <= cut)
                })
            })
            && (!missing
                .keys()
                .iter()
                .any(|key| baseline.keys().binary_search(key).is_err())
                || self
                    .unindexed
                    .last_change
                    .is_none_or(|change| change <= cut))
    }

    fn has_new_dependencies(
        baseline: &KnownDependencies,
        dependencies: &KnownDependencies,
    ) -> bool {
        dependencies
            .keys()
            .iter()
            .any(|key| baseline.keys().binary_search(key).is_err())
    }

    pub(super) fn plan_event(
        &self,
        keys: Vec<DependencyKey>,
        event: DependencyEvent,
    ) -> Result<Option<DependencyControlDelta>, DependencyError> {
        match event {
            DependencyEvent::Availability(cut) => self.plan_events(keys, Vec::new(), cut),
            DependencyEvent::DefinitiveLoss(cut) => self.plan_events(Vec::new(), keys, cut),
        }
    }

    /// Compile availability and definitive loss from one projected final
    /// state. A key cannot be both; callers must resolve that contradiction
    /// before publishing the level transition.
    pub(super) fn plan_events(
        &self,
        mut available: Vec<DependencyKey>,
        mut lost: Vec<DependencyKey>,
        cut: DependencyCut,
    ) -> Result<Option<DependencyControlDelta>, DependencyError> {
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
            let previous = self.levels.get(&key).copied();
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
                level: DependencyLevel {
                    last_change: cut,
                    last_definitive_loss,
                },
                scope,
            });
        }
        Ok(Some(DependencyControlDelta::Event(DependencyEventPlan {
            changes,
        })))
    }

    pub(super) fn next_maintenance(
        &self,
    ) -> Result<Option<DependencyMaintenanceTicket>, DependencyError> {
        let Some(key) = self.next_dirty_key() else {
            return Ok(None);
        };
        let dirty = self
            .dirty
            .get(key)
            .cloned()
            .ok_or(DependencyError::Projection)?;
        let edges = match dirty.scope {
            DirtyScope::ExistingWaiters => self.waiters.get(key),
            DirtyScope::AllConsumers => self.consumers.get(key),
        };
        let next = edges.and_then(|edges| {
            dirty.cursor.as_ref().map_or_else(
                || edges.iter().next().cloned(),
                |cursor| edges.range((Excluded(cursor), Unbounded)).next().cloned(),
            )
        });
        Ok(Some(DependencyMaintenanceTicket {
            key: key.clone(),
            hash: next,
            target: dirty.target,
            scope: dirty.scope,
            last_definitive_loss: self
                .levels
                .get(key)
                .and_then(|level| level.last_definitive_loss),
            expected: dirty,
        }))
    }

    #[cfg(test)]
    pub(super) fn next_maintenance_observation(
        &self,
    ) -> Result<Option<(DependencyKey, Option<RawTxHash>)>, DependencyError> {
        Ok(self
            .next_maintenance()?
            .map(|ticket| (ticket.key, ticket.hash)))
    }

    fn next_dirty_key(&self) -> Option<&DependencyKey> {
        self.dirty_cursor
            .as_ref()
            .and_then(|cursor| {
                self.dirty
                    .range((Excluded(cursor), Unbounded))
                    .next()
                    .map(|(key, _)| key)
            })
            .or_else(|| self.dirty.first_key_value().map(|(key, _)| key))
    }

    pub(super) fn plan_maintenance(
        &self,
        ticket: DependencyMaintenanceTicket,
    ) -> Result<DependencyControlDelta, DependencyError> {
        if self.dirty.get(&ticket.key) != Some(&ticket.expected)
            || self.next_dirty_key() != Some(&ticket.key)
        {
            return Err(DependencyError::Projection);
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
        Ok(DependencyControlDelta::Maintenance(
            DependencyMaintenancePlan(step),
        ))
    }

    pub(super) fn apply_control(&mut self, control: DependencyControlDelta) {
        match control {
            DependencyControlDelta::None => {}
            DependencyControlDelta::Event(event) => self.apply_event(event),
            DependencyControlDelta::Maintenance(maintenance) => {
                self.apply_maintenance(maintenance);
            }
        }
    }

    fn apply_event(&mut self, DependencyEventPlan { changes }: DependencyEventPlan) {
        for change in changes {
            if !self.consumers.contains_key(&change.key) {
                self.retire_level(change.level);
                if let Some(previous) = self.levels.remove(&change.key) {
                    self.retire_level(previous);
                }
                self.dirty.remove(&change.key);
                continue;
            }
            self.levels.insert(change.key.clone(), change.level);
            if let Some(dirty) = self.dirty.get_mut(&change.key) {
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
                continue;
            }
            let has_target = match change.scope {
                DirtyScope::ExistingWaiters => self
                    .waiters
                    .get(&change.key)
                    .is_some_and(|waiters| !waiters.is_empty()),
                DirtyScope::AllConsumers => self
                    .consumers
                    .get(&change.key)
                    .is_some_and(|consumers| !consumers.is_empty()),
            };
            if has_target {
                self.dirty.insert(
                    change.key.clone(),
                    DirtyDependency {
                        target: change.level.last_change,
                        scope: change.scope,
                        cursor: None,
                        pending: None,
                    },
                );
            }
        }
        if self.dirty.is_empty() {
            self.dirty_cursor = None;
        }
    }

    fn apply_maintenance(&mut self, DependencyMaintenancePlan(step): DependencyMaintenancePlan) {
        match step {
            DependencyMaintenanceStep::Advance {
                key,
                expected,
                cursor,
            } => {
                if self.consumers.contains_key(&key) {
                    let mut next = expected;
                    next.cursor = Some(cursor);
                    self.dirty.insert(key.clone(), next);
                    self.dirty_cursor = Some(key);
                } else {
                    self.dirty.remove(&key);
                    if let Some(level) = self.levels.remove(&key) {
                        self.retire_level(level);
                    }
                    if self.dirty.is_empty() {
                        self.dirty_cursor = None;
                    }
                }
            }
            DependencyMaintenanceStep::Complete { key, expected } => {
                if let Some(PendingDependency { target, scope }) = expected.pending {
                    self.dirty.insert(
                        key.clone(),
                        DirtyDependency {
                            target,
                            scope,
                            cursor: None,
                            pending: None,
                        },
                    );
                } else {
                    self.dirty.remove(&key);
                    if !self.consumers.contains_key(&key)
                        && let Some(level) = self.levels.remove(&key)
                    {
                        self.retire_level(level);
                    }
                }
                self.dirty_cursor = (!self.dirty.is_empty()).then_some(key);
            }
        }
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
        Ok(DependencyDelta {
            before,
            after,
            control: DependencyControlDelta::default(),
        })
    }

    pub(super) fn plan_replacements<'entry>(
        &self,
        changes: impl IntoIterator<Item = (Option<&'entry OwnedTx>, Option<&'entry OwnedTx>)>,
    ) -> Result<DependencyBatchDelta, DependencyError> {
        self.plan_replacements_with_additions(changes, VacancyPolicy::ExistingOwnersOnly)
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
        let changes = changes
            .into_iter()
            .map(|(before, after)| {
                Ok((
                    before.map(DependencySlot::from_owner).transpose()?,
                    after.map(DependencySlot::from_owner).transpose()?,
                ))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut removed_hashes = BTreeSet::new();
        let mut added_hashes = BTreeSet::new();
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
                if !removed_hashes.insert(before.hash.clone()) || !self.contains(&before) {
                    return Err(DependencyError::Projection);
                }
                removed.push(before);
            }
            if let Some(after) = after {
                if !added_hashes.insert(after.hash.clone()) {
                    return Err(DependencyError::Projection);
                }
                added.push(after);
            }
        }
        // This compiler accepts only replacements/removals of primary owners.
        // Requiring every added identity to have an exact `before` proof makes
        // accidental duplicate attachment unrepresentable without scanning the
        // complete reverse index under the authority guard. A future bulk
        // admission/chain-generation API must carry its own typed vacancy proof.
        match vacancy {
            VacancyPolicy::ExistingOwnersOnly => {
                if !added_hashes.is_subset(&removed_hashes) {
                    return Err(DependencyError::Projection);
                }
            }
            VacancyPolicy::PrimaryVacancyProven => {}
        }
        Ok(DependencyBatchDelta {
            removed,
            added,
            control: DependencyControlDelta::default(),
        })
    }

    pub(super) fn apply(&mut self, delta: DependencyDelta) {
        if let Some(before) = &delta.before {
            self.detach(before);
        }
        if let Some(after) = &delta.after {
            self.attach(after);
        }
        if let Some(before) = &delta.before {
            self.prune_orphaned(before);
        }
        self.apply_control(delta.control);
    }

    pub(super) fn apply_batch(&mut self, delta: DependencyBatchDelta) {
        for slot in &delta.removed {
            self.detach(slot);
        }
        for slot in &delta.added {
            self.attach(slot);
        }
        for slot in &delta.removed {
            self.prune_orphaned(slot);
        }
        self.apply_control(delta.control);
    }

    pub(super) fn snapshot(&self) -> DependencySnapshot {
        DependencySnapshot {
            consumers: self.consumers.clone(),
            waiters: self.waiters.clone(),
            keys_by_origin: self.keys_by_origin.clone(),
            levels: self.levels.clone(),
            dirty: self.dirty.clone(),
            dirty_cursor: self.dirty_cursor.clone(),
            unindexed: self.unindexed,
        }
    }

    fn contains(&self, slot: &DependencySlot) -> bool {
        slot.dependencies.keys().iter().all(|key| {
            self.consumers
                .get(key)
                .is_some_and(|consumers| consumers.contains(&slot.hash))
                && self
                    .keys_by_origin
                    .get(&key.origin())
                    .is_some_and(|keys| keys.contains(key))
        }) && slot.waiting.as_ref().is_none_or(|observed| {
            observed.keys().all(|key| {
                self.waiters
                    .get(key)
                    .is_some_and(|waiters| waiters.contains(&slot.hash))
            })
        })
    }

    fn attach(&mut self, slot: &DependencySlot) {
        for key in slot.dependencies.keys() {
            self.consumers
                .entry(key.clone())
                .or_default()
                .insert(slot.hash.clone());
            self.keys_by_origin
                .entry(key.origin())
                .or_default()
                .insert(key.clone());
        }
        if let Some(observed) = &slot.waiting {
            for key in observed.keys() {
                self.waiters
                    .entry(key.clone())
                    .or_default()
                    .insert(slot.hash.clone());
            }
        }
    }

    fn detach(&mut self, slot: &DependencySlot) {
        if let Some(observed) = &slot.waiting {
            for key in observed.keys() {
                let empty = self.waiters.get_mut(key).is_none_or(|waiters| {
                    waiters.remove(&slot.hash);
                    waiters.is_empty()
                });
                if empty {
                    self.waiters.remove(key);
                }
            }
        }
        for key in slot.dependencies.keys() {
            let empty = self.consumers.get_mut(key).is_none_or(|consumers| {
                consumers.remove(&slot.hash);
                consumers.is_empty()
            });
            if empty {
                self.consumers.remove(key);
            }
        }
    }

    fn prune_orphaned(&mut self, slot: &DependencySlot) {
        for key in slot.dependencies.keys() {
            if self.consumers.contains_key(key) {
                continue;
            }
            let origin = key.origin();
            let origin_empty = self.keys_by_origin.get_mut(&origin).is_none_or(|keys| {
                keys.remove(key);
                keys.is_empty()
            });
            if origin_empty {
                self.keys_by_origin.remove(&origin);
            }
            self.dirty.remove(key);
            if let Some(level) = self.levels.remove(key) {
                self.retire_level(level);
            }
        }
        if self.dirty.is_empty() {
            self.dirty_cursor = None;
        }
    }

    fn retire_level(&mut self, level: DependencyLevel) {
        self.unindexed.last_change = Some(
            self.unindexed
                .last_change
                .map_or(level.last_change, |current| current.max(level.last_change)),
        );
        if let Some(loss) = level.last_definitive_loss {
            self.unindexed.last_definitive_loss = Some(
                self.unindexed
                    .last_definitive_loss
                    .map_or(loss, |current| current.max(loss)),
            );
        }
    }

    #[cfg(test)]
    pub(super) fn semantically_matches(
        &self,
        entries: &std::collections::HashMap<RawTxHash, OwnedTx>,
    ) -> bool {
        let mut expected = Self::default();
        for owner in entries.values() {
            let Ok(slot) = DependencySlot::from_owner(owner) else {
                return false;
            };
            if let OwnedTx::PreAccepted(entry) = owner
                && entry.dependencies().len() > entry.charge.edges
            {
                return false;
            }
            if let OwnedTx::ReplacementHistory(entry) = owner
                && entry.dependencies().len() > entry.charge().edges
            {
                return false;
            }
            expected.attach(&slot);
        }
        self.consumers == expected.consumers
            && self.waiters == expected.waiters
            && self.keys_by_origin == expected.keys_by_origin
            && self
                .levels
                .keys()
                .all(|key| self.consumers.contains_key(key))
            && self.dirty.iter().all(|(key, dirty)| {
                self.consumers.contains_key(key)
                    && self.levels.get(key).is_some_and(|level| {
                        dirty.target <= level.last_change
                            && (!dirty.scope.requires_definitive_loss()
                                || level.last_definitive_loss.is_some())
                            && dirty.pending.is_none_or(|pending| {
                                pending.target <= level.last_change
                                    && (!pending.scope.requires_definitive_loss()
                                        || level.last_definitive_loss.is_some())
                            })
                    })
            })
            && (!self.dirty.is_empty() || self.dirty_cursor.is_none())
            && self.unindexed.last_definitive_loss.is_none_or(|loss| {
                self.unindexed
                    .last_change
                    .is_some_and(|change| loss <= change)
            })
            && self.waiters.iter().all(|(key, waiters)| {
                self.consumers
                    .get(key)
                    .is_some_and(|consumers| waiters.is_subset(consumers))
            })
    }
}
