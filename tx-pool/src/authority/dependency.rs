use super::shard::{ShardedOwnerMap, ShardedOwnerWriteCut};
use super::state::{
    DependencyCut, DependencyKey, DependencyOrigin, KnownDependencies, MissingDependencies,
    ObservedDependencies, OwnedTx, PreAcceptedPhase, QueuedWork, RawTxHash,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Bound::{Excluded, Unbounded},
};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DependencyError {
    Projection,
    Allocation,
    SurvivingAcceptedConsumer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StableDependencyError {
    Projection,
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

#[must_use = "a dependency maintenance successor must be carried by one authority Apply"]
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
impl DependencyControlDelta {
    fn extend_shard_support(
        &self,
        support: &mut super::shard_support::AuthorityShardSupport,
        exclusive: &mut super::shard_support::ExclusiveSupport,
    ) {
        match self {
            Self::None => {}
            Self::Event(event) => {
                for change in &event.changes {
                    support.insert(b"dependency/consumer", &change.key);
                    support.insert(b"dependency/waiter", &change.key);
                    support.insert(b"dependency/level", &change.key);
                }
                exclusive.dependency_control = true;
            }
            Self::Maintenance(DependencyMaintenancePlan(step)) => {
                let key = match step {
                    DependencyMaintenanceStep::Advance { key, .. }
                    | DependencyMaintenanceStep::Complete { key, .. } => key,
                };
                support.insert(b"dependency/consumer", key);
                support.insert(b"dependency/level", key);
                exclusive.dependency_control = true;
            }
        }
    }
}

#[cfg(test)]
impl DependencyDelta {
    pub(in crate::authority) fn extend_shard_support(
        &self,
        support: &mut super::shard_support::AuthorityShardSupport,
        exclusive: &mut super::shard_support::ExclusiveSupport,
    ) {
        if let Some(before) = &self.before {
            before.extend_shard_support(support);
        }
        if let Some(after) = &self.after {
            after.extend_shard_support(support);
        }
        self.control.extend_shard_support(support, exclusive);
    }
}

#[cfg(test)]
impl DependencyBatchDelta {
    pub(in crate::authority) fn extend_shard_support(
        &self,
        support: &mut super::shard_support::AuthorityShardSupport,
        exclusive: &mut super::shard_support::ExclusiveSupport,
    ) {
        for slot in self.removed.iter().chain(&self.added) {
            slot.extend_shard_support(support);
        }
        self.control.extend_shard_support(support, exclusive);
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
    dirty: BTreeMap<DependencyKey, DirtyDependency>,
    dirty_cursor: Option<DependencyKey>,
}

#[expect(
    clippy::indexing_slicing,
    reason = "the sole shard router masks every domain/key result to the fixed 64-shard layout"
)]
impl DependencyFrontier {
    pub(super) fn for_entries(entries: &ShardedOwnerMap) -> Self {
        Self {
            entries: entries.clone(),
            dirty: BTreeMap::new(),
            dirty_cursor: None,
        }
    }

    fn shard<K: std::hash::Hash>(&self, domain: &'static [u8], key: &K) -> usize {
        self.entries.layout.router.shard(domain, key)
    }

    fn consumers(&self, key: &DependencyKey) -> Option<BTreeSet<RawTxHash>> {
        self.entries.layout.shards[self.shard(b"dependency/consumer", key)]
            .read()
            .dependency_consumers
            .get(key)
            .cloned()
    }

    fn waiters(&self, key: &DependencyKey) -> Option<BTreeSet<RawTxHash>> {
        self.entries.layout.shards[self.shard(b"dependency/waiter", key)]
            .read()
            .dependency_waiters
            .get(key)
            .cloned()
    }

    fn level(&self, key: &DependencyKey) -> Option<DependencyLevel> {
        self.entries.layout.shards[self.shard(b"dependency/level", key)]
            .read()
            .dependency_levels
            .get(key)
            .copied()
    }

    fn unindexed_level(&self, key: &DependencyKey) -> UnindexedDependencyLevel {
        self.entries.layout.shards[self.shard(b"dependency/unindexed", key)]
            .read()
            .dependency_unindexed
    }

    fn origin_keys(&self, origin: &DependencyOrigin) -> Option<BTreeSet<DependencyKey>> {
        self.entries.layout.shards[self.shard(b"dependency/origin", origin)]
            .read()
            .dependency_keys_by_origin
            .get(origin)
            .cloned()
    }

    fn has_consumers(&self, key: &DependencyKey) -> bool {
        self.consumers(key).is_some_and(|owners| !owners.is_empty())
    }

    fn has_waiters(&self, key: &DependencyKey) -> bool {
        self.waiters(key).is_some_and(|owners| !owners.is_empty())
    }

    fn replace_level(&self, key: DependencyKey, level: DependencyLevel) -> Option<DependencyLevel> {
        self.entries.layout.shards[self.shard(b"dependency/level", &key)]
            .write()
            .dependency_levels
            .insert(key, level)
    }

    fn remove_level(&self, key: &DependencyKey) -> Option<DependencyLevel> {
        self.entries.layout.shards[self.shard(b"dependency/level", key)]
            .write()
            .dependency_levels
            .remove(key)
    }
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

    pub(super) fn closed_removal_compatible(&self, frontier: &DependencyFrontier) -> bool {
        let control_compatible = match &self.control {
            DependencyControlDelta::None => true,
            DependencyControlDelta::Event(event) => event.changes.iter().all(|change| {
                change.scope == DirtyScope::AllConsumers
                    && !frontier.dirty.contains_key(&change.key)
                    && frontier.level(&change.key).is_none()
                    && frontier
                        .consumers(&change.key)
                        .into_iter()
                        .flatten()
                        .all(|owner| self.removed.iter().any(|slot| slot.hash == owner))
                    && frontier
                        .waiters(&change.key)
                        .into_iter()
                        .flatten()
                        .all(|owner| self.removed.iter().any(|slot| slot.hash == owner))
            }),
            DependencyControlDelta::Maintenance(_) => false,
        };
        control_compatible
            && self.added.is_empty()
            && self.removed.iter().all(|slot| {
                slot.waiting.is_none()
                    && slot.dependencies.keys().iter().all(|key| {
                        frontier.level(key).is_none() && !frontier.dirty.contains_key(key)
                    })
            })
    }

    pub(in crate::authority) fn sharded_write_support(
        &self,
        entries: &ShardedOwnerMap,
    ) -> super::shard::ShardWriteSupport {
        let mut support = super::shard::ShardWriteSupport::default();
        for slot in self.removed.iter().chain(&self.added) {
            for key in slot.dependencies.keys() {
                support.insert(entries.layout.router.shard(b"dependency/consumer", key));
                support.insert(entries.layout.router.shard(b"dependency/waiter", key));
                support.insert(
                    entries
                        .layout
                        .router
                        .shard(b"dependency/origin", &key.origin()),
                );
                support.insert(entries.layout.router.shard(b"dependency/level", key));
            }
            support.insert(entries.layout.router.shard(
                b"dependency/origin",
                &DependencyOrigin::Transaction(slot.hash.clone()),
            ));
            if let Some(waiting) = &slot.waiting {
                for key in waiting.keys() {
                    support.insert(entries.layout.router.shard(b"dependency/waiter", key));
                }
            }
        }
        if let DependencyControlDelta::Event(event) = &self.control {
            for change in &event.changes {
                support.insert(
                    entries
                        .layout
                        .router
                        .shard(b"dependency/consumer", &change.key),
                );
                support.insert(
                    entries
                        .layout
                        .router
                        .shard(b"dependency/waiter", &change.key),
                );
                support.insert(
                    entries
                        .layout
                        .router
                        .shard(b"dependency/level", &change.key),
                );
                support.insert(
                    entries
                        .layout
                        .router
                        .shard(b"dependency/unindexed", &change.key),
                );
            }
        }
        support
    }

    pub(in crate::authority) fn apply_closed_removal_sharded(
        self,
        entries: &ShardedOwnerMap,
        cut: &mut ShardedOwnerWriteCut<'_>,
    ) {
        for slot in &self.removed {
            for key in slot.dependencies.keys() {
                let consumer_shard = entries.layout.router.shard(b"dependency/consumer", key);
                let consumers = &mut cut
                    .projection_shard_mut(consumer_shard)
                    .dependency_consumers;
                let empty = consumers.get_mut(key).is_none_or(|owners| {
                    owners.remove(&slot.hash);
                    owners.is_empty()
                });
                if empty {
                    consumers.remove(key);
                }
            }
        }
        if let DependencyControlDelta::Event(event) = self.control {
            for change in event.changes {
                let shard = entries
                    .layout
                    .router
                    .shard(b"dependency/unindexed", &change.key);
                let unindexed = &mut cut.projection_shard_mut(shard).dependency_unindexed;
                unindexed.last_change = Some(
                    unindexed
                        .last_change
                        .map_or(change.level.last_change, |current| {
                            current.max(change.level.last_change)
                        }),
                );
                if let Some(loss) = change.level.last_definitive_loss {
                    unindexed.last_definitive_loss = Some(
                        unindexed
                            .last_definitive_loss
                            .map_or(loss, |current| current.max(loss)),
                    );
                }
            }
        }
        for slot in &self.removed {
            for key in slot.dependencies.keys() {
                let consumer_shard = entries.layout.router.shard(b"dependency/consumer", key);
                if cut
                    .projection_shard_mut(consumer_shard)
                    .dependency_consumers
                    .contains_key(key)
                {
                    continue;
                }
                let origin = key.origin();
                let origin_shard = entries.layout.router.shard(b"dependency/origin", &origin);
                let rows = &mut cut
                    .projection_shard_mut(origin_shard)
                    .dependency_keys_by_origin;
                let empty = rows.get_mut(&origin).is_none_or(|keys| {
                    keys.remove(key);
                    keys.is_empty()
                });
                if empty {
                    rows.remove(&origin);
                }
            }
        }
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

impl DependencyMaintenancePlan {
    pub(super) fn into_control(self) -> DependencyControlDelta {
        DependencyControlDelta::Maintenance(self)
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

#[expect(
    clippy::indexing_slicing,
    reason = "the sole shard router masks every domain/key result to the fixed 64-shard layout"
)]
impl DependencyFrontier {
    fn all_observed_dependencies_available(&self, observed: &ObservedDependencies) -> bool {
        observed.keys().all(|key| {
            self.level(key).is_some_and(|level| {
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
    ) -> Option<BTreeSet<DependencyKey>> {
        self.origin_keys(origin)
    }

    pub(super) fn consumers_for(&self, key: &DependencyKey) -> Option<BTreeSet<RawTxHash>> {
        self.consumers(key)
    }

    pub(super) fn has_waiter_outside(&self, key: &DependencyKey, removed: &[RawTxHash]) -> bool {
        self.waiters(key)
            .into_iter()
            .flatten()
            .any(|owner| !removed.contains(&owner))
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

    pub(super) fn maintenance_pending(&self) -> bool {
        !self.dirty.is_empty()
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
            DirtyScope::ExistingWaiters => self.waiters(key),
            DirtyScope::AllConsumers => self.consumers(key),
        };
        let next = edges.as_ref().and_then(|edges| {
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
            last_definitive_loss: self.level(key).and_then(|level| level.last_definitive_loss),
            expected: dirty,
        }))
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
    ) -> Result<DependencyMaintenancePlan, DependencyError> {
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
        Ok(DependencyMaintenancePlan(step))
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
            if !self.has_consumers(&change.key) {
                self.retire_level(&change.key, change.level);
                if let Some(previous) = self.remove_level(&change.key) {
                    self.retire_level(&change.key, previous);
                }
                self.dirty.remove(&change.key);
                continue;
            }
            self.replace_level(change.key.clone(), change.level);
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
                DirtyScope::ExistingWaiters => self.has_waiters(&change.key),
                DirtyScope::AllConsumers => self.has_consumers(&change.key),
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
                if self.has_consumers(&key) {
                    let mut next = expected;
                    next.cursor = Some(cursor);
                    self.dirty.insert(key.clone(), next);
                    self.dirty_cursor = Some(key);
                } else {
                    self.dirty.remove(&key);
                    if let Some(level) = self.remove_level(&key) {
                        self.retire_level(&key, level);
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
                    if !self.has_consumers(&key)
                        && let Some(level) = self.remove_level(&key)
                    {
                        self.retire_level(&key, level);
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
        if before == after {
            // Phase/version changes commonly retain the exact dependency
            // footprint. Encoding that as detach+attach adds B-tree work and
            // allocation risk without changing the projection.
            return Ok(DependencyDelta {
                before: None,
                after: None,
                control: DependencyControlDelta::default(),
            });
        }
        Ok(DependencyDelta {
            before,
            after,
            control: DependencyControlDelta::default(),
        })
    }

    /// Validate a phase-only owner replacement with an unchanged dependency
    /// slot. No detach/attach storage is produced, so cancellation cannot
    /// acquire allocator backpressure from this projection.
    pub(super) fn plan_stable_replace(
        &self,
        before: &OwnedTx,
        after: &OwnedTx,
    ) -> Result<DependencyDelta, StableDependencyError> {
        let before =
            DependencySlot::from_owner(before).map_err(|_| StableDependencyError::Projection)?;
        let after =
            DependencySlot::from_owner(after).map_err(|_| StableDependencyError::Projection)?;
        if before != after || !self.contains(&before) {
            return Err(StableDependencyError::Projection);
        }
        Ok(DependencyDelta {
            before: None,
            after: None,
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
        let mut input = changes.into_iter();
        let mut changes = Vec::new();
        if let Some(capacity) = input.size_hint().1 {
            changes
                .try_reserve_exact(capacity)
                .map_err(|_| DependencyError::Allocation)?;
        }
        for (before, after) in input.by_ref() {
            if changes.len() == changes.capacity() {
                changes
                    .try_reserve(1)
                    .map_err(|_| DependencyError::Allocation)?;
            }
            changes.push((
                before.map(DependencySlot::from_owner).transpose()?,
                after.map(DependencySlot::from_owner).transpose()?,
            ));
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
        if removed
            .array_windows::<2>()
            .any(|[left, right]| left.hash == right.hash)
            || added
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

    fn contains(&self, slot: &DependencySlot) -> bool {
        slot.dependencies.keys().iter().all(|key| {
            self.consumers(key)
                .is_some_and(|consumers| consumers.contains(&slot.hash))
                && self
                    .origin_keys(&key.origin())
                    .is_some_and(|keys| keys.contains(key))
        }) && slot.waiting.as_ref().is_none_or(|observed| {
            observed.keys().all(|key| {
                self.waiters(key)
                    .is_some_and(|waiters| waiters.contains(&slot.hash))
            })
        })
    }

    fn attach(&mut self, slot: &DependencySlot) {
        for key in slot.dependencies.keys() {
            self.entries.layout.shards[self.shard(b"dependency/consumer", key)]
                .write()
                .dependency_consumers
                .entry(key.clone())
                .or_default()
                .insert(slot.hash.clone());
            let origin = key.origin();
            self.entries.layout.shards[self.shard(b"dependency/origin", &origin)]
                .write()
                .dependency_keys_by_origin
                .entry(key.origin())
                .or_default()
                .insert(key.clone());
        }
        if let Some(observed) = &slot.waiting {
            for key in observed.keys() {
                self.entries.layout.shards[self.shard(b"dependency/waiter", key)]
                    .write()
                    .dependency_waiters
                    .entry(key.clone())
                    .or_default()
                    .insert(slot.hash.clone());
            }
        }
    }

    fn detach(&mut self, slot: &DependencySlot) {
        if let Some(observed) = &slot.waiting {
            for key in observed.keys() {
                let mut shard =
                    self.entries.layout.shards[self.shard(b"dependency/waiter", key)].write();
                let empty = shard.dependency_waiters.get_mut(key).is_none_or(|waiters| {
                    waiters.remove(&slot.hash);
                    waiters.is_empty()
                });
                if empty {
                    shard.dependency_waiters.remove(key);
                }
            }
        }
        for key in slot.dependencies.keys() {
            let mut shard =
                self.entries.layout.shards[self.shard(b"dependency/consumer", key)].write();
            let empty = shard
                .dependency_consumers
                .get_mut(key)
                .is_none_or(|consumers| {
                    consumers.remove(&slot.hash);
                    consumers.is_empty()
                });
            if empty {
                shard.dependency_consumers.remove(key);
            }
        }
    }

    fn prune_orphaned(&mut self, slot: &DependencySlot) {
        for key in slot.dependencies.keys() {
            if self.consumers(key).is_some() {
                continue;
            }
            let origin = key.origin();
            let mut shard =
                self.entries.layout.shards[self.shard(b"dependency/origin", &origin)].write();
            let origin_empty =
                shard
                    .dependency_keys_by_origin
                    .get_mut(&origin)
                    .is_none_or(|keys| {
                        keys.remove(key);
                        keys.is_empty()
                    });
            if origin_empty {
                shard.dependency_keys_by_origin.remove(&origin);
            }
            drop(shard);
            self.dirty.remove(key);
            if let Some(level) = self.remove_level(key) {
                self.retire_level(key, level);
            }
        }
        if self.dirty.is_empty() {
            self.dirty_cursor = None;
        }
    }

    fn retire_level(&self, key: &DependencyKey, level: DependencyLevel) {
        let mut shard =
            self.entries.layout.shards[self.shard(b"dependency/unindexed", key)].write();
        let unindexed = &mut shard.dependency_unindexed;
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
}

#[cfg(test)]
#[path = "tests/support/dependency.rs"]
pub(in crate::authority) mod test_support;
