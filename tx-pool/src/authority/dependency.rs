use super::shard::{
    AUTHORITY_SHARD_COUNT, DependencyGateCut, DependencyGateSupport, DependencyRelationShard,
    ShardReadSupport, ShardWriteSupport, ShardedDependencyRelationWriteCut, ShardedOwnerMap,
    ShardedOwnerWriteCut,
};
use super::state::{
    DependencyCut, DependencyKey, KnownDependencies, MissingDependencies, ObservedDependencies,
    OwnedTx, PreAcceptedPhase, QueuedWork, RawTxHash,
};
use ckb_util::parking_lot::Mutex;
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Bound::{Excluded, Unbounded},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::authority) enum DependencyConsumerPhase {
    Accepted,
    Other,
}

/// The complete logical relation for one `(dependency key, owner)` pair.
/// Waiting is a refinement of consumer membership, not a second edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DependencyRelationValue {
    phase: DependencyConsumerPhase,
    waiting: bool,
}

impl DependencyRelationValue {
    fn for_slot(slot: &DependencySlot, key: &DependencyKey) -> Self {
        Self {
            phase: slot.phase,
            waiting: slot
                .waiting
                .as_ref()
                .is_some_and(|waiting| waiting.contains(key)),
        }
    }
}

#[derive(Clone, Copy)]
enum DependencyRelationFilter {
    Accepted,
    Consumers,
    Waiters,
}

impl DependencyRelationFilter {
    fn matches(self, value: DependencyRelationValue) -> bool {
        match self {
            Self::Accepted => value.phase == DependencyConsumerPhase::Accepted,
            Self::Consumers => true,
            Self::Waiters => value.waiting,
        }
    }
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
    visible: Option<BTreeSet<RawTxHash>>,
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
    pub(in crate::authority) fn dependency_gate_support(
        &self,
        entries: &ShardedOwnerMap,
    ) -> DependencyGateSupport {
        let mut support = DependencyGateSupport::default();
        support.write(dependency_key_gate(entries, &self.key));
        support
    }

    pub(in crate::authority) fn is_fresh_under_gate(
        &self,
        entries: &ShardedOwnerMap,
        gates: &DependencyGateCut<'_>,
    ) -> bool {
        let filter = match self.kind {
            DependencyConsumerObservationKind::Accepted => DependencyRelationFilter::Accepted,
            DependencyConsumerObservationKind::General => DependencyRelationFilter::Consumers,
        };
        let limit = self
            .accepted_over_limit
            .unwrap_or_else(|| self.visible.as_ref().map_or(0, BTreeSet::len));
        match dependency_visible_owners_under_gate(entries, gates, &self.key, filter, limit) {
            Err(DependencyError::Fanout) => self.accepted_over_limit.is_some(),
            Ok(visible) => self.accepted_over_limit.is_none() && visible == self.visible,
            Err(_) => false,
        }
    }
}

/// One stable value per `(dependency key, owner)`. The relation lives in the
/// independent consumer-owner bank; the accepted set is a derived local
/// accelerator.
#[derive(Clone, Debug, Default)]
pub(in crate::authority) struct DependencyRelationSet {
    entries: BTreeMap<RawTxHash, DependencyRelationValue>,
    accepted_participants: BTreeSet<RawTxHash>,
}

impl DependencyRelationSet {
    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[cfg(test)]
    fn accepted_participants_are_exact(&self) -> bool {
        self.entries
            .iter()
            .filter_map(|(owner, value)| {
                (value.phase == DependencyConsumerPhase::Accepted).then_some(owner)
            })
            .eq(self.accepted_participants.iter())
    }

    fn contains_visible_matching(
        &self,
        owner: &RawTxHash,
        filter: DependencyRelationFilter,
    ) -> bool {
        self.entries
            .get(owner)
            .is_some_and(|value| filter.matches(*value))
    }

    fn has_visible(&self, filter: DependencyRelationFilter) -> bool {
        match filter {
            DependencyRelationFilter::Accepted => !self.accepted_participants.is_empty(),
            DependencyRelationFilter::Consumers => !self.entries.is_empty(),
            DependencyRelationFilter::Waiters => self.entries.values().any(|value| value.waiting),
        }
    }

    fn first_visible_after_bounded(
        &self,
        filter: DependencyRelationFilter,
        cursor: Option<&RawTxHash>,
    ) -> Result<Option<RawTxHash>, DependencyError> {
        Ok(match cursor {
            Some(cursor) => self
                .entries
                .range((Excluded(cursor), Unbounded))
                .find(|(_, value)| filter.matches(**value))
                .map(|(owner, _)| owner.clone()),
            None => self
                .entries
                .iter()
                .find(|(_, value)| filter.matches(**value))
                .map(|(owner, _)| owner.clone()),
        })
    }

    fn apply(&mut self, owner: RawTxHash, after: Option<DependencyRelationValue>) {
        match after {
            Some(value) => {
                self.entries.insert(owner.clone(), value);
                if value.phase == DependencyConsumerPhase::Accepted {
                    self.accepted_participants.insert(owner);
                } else {
                    self.accepted_participants.remove(&owner);
                }
            }
            None => {
                self.entries.remove(&owner);
                self.accepted_participants.remove(&owner);
            }
        }
    }
}

#[cfg(test)]
mod relation_set_tests {
    use super::*;
    use ckb_types::packed::Byte32;

    #[test]
    fn accepted_capture_cost_excludes_other_phase_fanout() {
        let accepted = RawTxHash(Byte32::new([0; 32]));
        let mut relations = DependencyRelationSet::default();
        for index in 0..64u8 {
            let owner = RawTxHash(Byte32::new([index; 32]));
            let phase = if owner == accepted {
                DependencyConsumerPhase::Accepted
            } else {
                DependencyConsumerPhase::Other
            };
            relations.entries.insert(
                owner.clone(),
                DependencyRelationValue {
                    phase,
                    waiting: false,
                },
            );
        }
        relations.accepted_participants.insert(accepted.clone());

        let visible = relations
            .accepted_participants
            .iter()
            .cloned()
            .collect::<Vec<_>>();

        assert_eq!(relations.entries.len(), 64);
        assert_eq!(visible, vec![accepted]);
        assert!(relations.accepted_participants_are_exact());
    }
}

fn dependency_relation_shard(entries: &ShardedOwnerMap, owner: &RawTxHash) -> usize {
    entries.owner_shard(owner)
}

fn dependency_key_gate(entries: &ShardedOwnerMap, key: &DependencyKey) -> usize {
    entries.layout.router.shard(b"dependency/gate/key", key)
}

fn dependency_relation_set<'row>(
    row: &'row DependencyRelationShard,
    key: &DependencyKey,
) -> Option<&'row DependencyRelationSet> {
    row.rows.get(key)
}

/// Fold one dependency key across consumer-owned shards. The gate token is the
/// proof that no mutation of this logical key can pass between shard reads.
fn for_each_dependency_relation_set(
    entries: &ShardedOwnerMap,
    _gates: &DependencyGateCut<'_>,
    key: &DependencyKey,
    mut visit: impl FnMut(&DependencyRelationSet) -> Result<(), DependencyError>,
) -> Result<(), DependencyError> {
    for shard in 0..AUTHORITY_SHARD_COUNT {
        let shard = entries.dependency_relation_shard_read(shard);
        if let Some(relations) = dependency_relation_set(&shard, key) {
            visit(relations)?;
        }
    }
    Ok(())
}

fn dependency_has_matching_under_gate(
    entries: &ShardedOwnerMap,
    gates: &DependencyGateCut<'_>,
    key: &DependencyKey,
    filter: DependencyRelationFilter,
) -> Result<bool, DependencyError> {
    let mut found = false;
    for_each_dependency_relation_set(entries, gates, key, |relations| {
        found |= relations.has_visible(filter);
        Ok(())
    })?;
    Ok(found)
}

fn dependency_next_owner_under_gate(
    entries: &ShardedOwnerMap,
    gates: &DependencyGateCut<'_>,
    key: &DependencyKey,
    scope: DirtyScope,
    cursor: Option<&RawTxHash>,
) -> Result<Option<RawTxHash>, DependencyError> {
    let filter = match scope {
        DirtyScope::AllConsumers => DependencyRelationFilter::Consumers,
        DirtyScope::ExistingWaiters => DependencyRelationFilter::Waiters,
    };
    let mut next: Option<RawTxHash> = None;
    for_each_dependency_relation_set(entries, gates, key, |relations| {
        if let Some(candidate) = relations.first_visible_after_bounded(filter, cursor)? {
            next = Some(
                next.take()
                    .map_or(candidate.clone(), |current| current.min(candidate)),
            );
        }
        Ok(())
    })?;
    Ok(next)
}

fn dependency_visible_owners_under_gate(
    entries: &ShardedOwnerMap,
    gates: &DependencyGateCut<'_>,
    key: &DependencyKey,
    filter: DependencyRelationFilter,
    limit: usize,
) -> Result<Option<BTreeSet<RawTxHash>>, DependencyError> {
    let mut visible = BTreeSet::new();
    for_each_dependency_relation_set(entries, gates, key, |relations| {
        for (owner, value) in &relations.entries {
            if filter.matches(*value) {
                if !visible.insert(owner.clone()) {
                    return Err(DependencyError::Projection);
                }
                if visible.len() > limit {
                    return Err(DependencyError::Fanout);
                }
            }
        }
        Ok(())
    })?;
    Ok((!visible.is_empty()).then_some(visible))
}

fn dependency_consumers_under_gate(
    entries: &ShardedOwnerMap,
    gates: &DependencyGateCut<'_>,
    key: &DependencyKey,
    filter: DependencyRelationFilter,
    limit: usize,
) -> Result<Option<BTreeSet<RawTxHash>>, DependencyError> {
    dependency_visible_owners_under_gate(entries, gates, key, filter, limit)
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

impl UnindexedDependencyLevel {
    fn merge(&mut self, level: DependencyLevel) {
        self.last_change = Some(
            self.last_change
                .map_or(level.last_change, |value| value.max(level.last_change)),
        );
        if let Some(loss) = level.last_definitive_loss {
            self.last_definitive_loss = Some(
                self.last_definitive_loss
                    .map_or(loss, |value| value.max(loss)),
            );
        }
    }

    fn merge_unindexed(&mut self, level: Self) {
        self.last_change = self.last_change.max(level.last_change);
        self.last_definitive_loss = self.last_definitive_loss.max(level.last_definitive_loss);
    }
}

fn replace_control_cell<T>(
    rows: &mut BTreeMap<DependencyKey, T>,
    key: &DependencyKey,
    value: Option<T>,
) {
    if let Some(value) = value {
        rows.insert(key.clone(), value);
    } else {
        rows.remove(key);
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

impl DependencyControlDelta {
    fn for_each_key(&self, mut visit: impl FnMut(&DependencyKey)) {
        match self {
            Self::None => {}
            Self::Event(event) => event.changes.iter().for_each(|change| visit(&change.key)),
            Self::Maintenance(maintenance) => visit(maintenance.key()),
        }
    }

    fn contains_key(&self, key: &DependencyKey) -> bool {
        match self {
            Self::None => false,
            Self::Event(event) => event
                .changes
                .binary_search_by(|change| change.key.cmp(key))
                .is_ok(),
            Self::Maintenance(maintenance) => maintenance.key() == key,
        }
    }
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
    relation_changes: Vec<DependencyRelationChange>,
    settlement_evidence: Vec<SettlementDependencyEvidence>,
    control: DependencyControlDelta,
    prestate: DependencyBatchPrestate,
}

#[derive(Default)]
struct DependencyBatchPrestate {
    relations: Vec<DependencyRelationPointPrestate>,
    keys: Vec<DependencyKeyPrestate>,
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
    owner: RawTxHash,
}

struct DependencyRelationPointPrestate {
    point: DependencyRelationPoint,
    value: Option<DependencyRelationValue>,
}

enum DependencyControlKeyPrestate {
    Event {
        // Accepted consumers participate in the administrative closure and
        // therefore remain a Plan-time semantic precondition. Other
        // consumers and waiters are operational fanout: the gate-held binder
        // projects them from one exact cut and carries the required absence
        // facts to the final Apply cut.
        has_accepted_consumers: bool,
    },
    Maintenance {
        scope: DirtyScope,
        next: Option<RawTxHash>,
    },
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct ProjectedDependencyFanout {
    has_consumers: bool,
    has_waiters: bool,
}

fn projected_dependency_fanout_under_gate(
    entries: &ShardedOwnerMap,
    gates: &DependencyGateCut<'_>,
    key: &DependencyKey,
    relations: &[DependencyRelationChange],
) -> Result<ProjectedDependencyFanout, DependencyError> {
    let mut projected = ProjectedDependencyFanout::default();
    for_each_dependency_relation_set(entries, gates, key, |current| {
        for (owner, value) in &current.entries {
            let after = match relations.binary_search_by(|change| change.point.owner.cmp(owner)) {
                Ok(position) => relations.get(position).and_then(|change| change.after),
                Err(_) => Some(*value),
            };
            if let Some(after) = after {
                projected.has_consumers = true;
                projected.has_waiters |= after.waiting;
            }
        }
        Ok(())
    })?;
    for relation in relations {
        if let Some(after) = relation.after {
            projected.has_consumers = true;
            projected.has_waiters |= after.waiting;
        }
    }
    Ok(projected)
}

struct DependencyControlTransition {
    level: Option<DependencyLevel>,
    dirty: Option<DirtyDependency>,
    unindexed: Option<UnindexedDependencyLevel>,
}

impl DependencyControlTransition {
    fn push_unindexed(&mut self, level: DependencyLevel) {
        self.unindexed.get_or_insert_default().merge(level);
    }
}

struct DependencyKeyPrestate {
    key: DependencyKey,
    level: Option<DependencyLevel>,
    dirty: Option<DirtyDependency>,
    control: Option<DependencyControlKeyPrestate>,
    projected_fanout: Option<ProjectedDependencyFanout>,
}

type DependencyPrestatePoints = (Vec<DependencyRelationPoint>, Vec<DependencyKey>);

impl DependencyBatchPrestate {
    fn capture(
        frontier: &DependencyFrontier,
        delta: &DependencyBatchDelta,
    ) -> Result<Self, DependencyError> {
        let (relations, keys) = Self::capture_points(delta)?;
        let entries = &frontier.entries;
        let gates = entries.dependency_gate_cut(dependency_gate_support_for(
            entries,
            &delta.relation_changes,
            &delta.control,
        ));

        let mut key_controls = Vec::with_capacity(keys.len());
        for key in &keys {
            let control = match &delta.control {
                DependencyControlDelta::Event(event)
                    if event.changes.iter().any(|change| &change.key == key) =>
                {
                    Some(DependencyControlKeyPrestate::Event {
                        has_accepted_consumers: dependency_has_matching_under_gate(
                            entries,
                            &gates,
                            key,
                            DependencyRelationFilter::Accepted,
                        )?,
                    })
                }
                DependencyControlDelta::Maintenance(maintenance) if maintenance.key() == key => {
                    Some(DependencyControlKeyPrestate::Maintenance {
                        scope: maintenance.expected().scope,
                        next: dependency_next_owner_under_gate(
                            entries,
                            &gates,
                            key,
                            maintenance.expected().scope,
                            maintenance.expected().cursor.as_ref(),
                        )?,
                    })
                }
                DependencyControlDelta::None
                | DependencyControlDelta::Event(_)
                | DependencyControlDelta::Maintenance(_) => None,
            };
            key_controls.push(control);
        }

        let mut relation_read_support = ShardReadSupport::default();
        let mut owner_read_support = ShardReadSupport::default();
        for key in &keys {
            for relation in relations.iter().filter(|point| &point.key == key) {
                relation_read_support.insert(dependency_relation_shard(entries, &relation.owner));
            }
            owner_read_support.insert(entries.layout.router.shard(b"dependency/level", key));
            owner_read_support.insert(entries.layout.router.shard(b"dependency/unindexed", key));
        }
        let relation_cut = entries
            .dependency_relation_mixed_cut(relation_read_support, ShardWriteSupport::default());
        let owner_cut = entries.mixed_cut(owner_read_support, ShardWriteSupport::default());
        Self::capture_in_cut(
            frontier,
            delta,
            relations,
            keys,
            key_controls,
            &relation_cut,
            &owner_cut,
        )
    }

    fn capture_points(
        delta: &DependencyBatchDelta,
    ) -> Result<DependencyPrestatePoints, DependencyError> {
        let slot_count = delta
            .removed
            .len()
            .checked_add(delta.added.len())
            .and_then(|count| count.checked_add(delta.observed.len()))
            .ok_or(DependencyError::Projection)?;
        let mut keys = Vec::with_capacity(slot_count);
        let mut relations = Vec::new();
        for slot in delta
            .removed
            .iter()
            .chain(&delta.added)
            .chain(&delta.observed)
        {
            keys.reserve(slot.dependencies.len());
            keys.extend(slot.dependencies.keys().iter().cloned());
            relations.reserve(slot.dependencies.len());
            relations.extend(slot.dependencies.keys().iter().cloned().map(|key| {
                DependencyRelationPoint {
                    key,
                    owner: slot.hash.clone(),
                }
            }));
        }
        relations.sort_unstable();
        relations.dedup();
        delta.control.for_each_key(|key| keys.push(key.clone()));
        keys.sort_unstable();
        keys.dedup();

        Ok((relations, keys))
    }

    fn capture_in_cut(
        frontier: &DependencyFrontier,
        delta: &DependencyBatchDelta,
        relations: Vec<DependencyRelationPoint>,
        keys: Vec<DependencyKey>,
        key_controls: Vec<Option<DependencyControlKeyPrestate>>,
        relation_cut: &ShardedDependencyRelationWriteCut<'_>,
        owner_cut: &ShardedOwnerWriteCut<'_>,
    ) -> Result<Self, DependencyError> {
        let entries = &frontier.entries;

        let mut relation_witnesses = Vec::with_capacity(relations.len());
        for point in relations {
            let value = dependency_relation_point_value_in_cut(entries, relation_cut, &point)?;
            relation_witnesses.push(DependencyRelationPointPrestate { point, value });
        }

        let mut key_witnesses = Vec::with_capacity(keys.len());
        let mut unindexed_shards = Vec::with_capacity(keys.len());

        for (key, control) in keys.into_iter().zip(key_controls) {
            let level_shard = entries.layout.router.shard(b"dependency/level", &key);
            let level_row = owner_cut.projection_shard(level_shard);
            let level = level_row.dependency_levels.get(&key).copied();
            let dirty = level_row.dependency_dirty.get(&key).cloned();
            unindexed_shards.push(entries.layout.router.shard(b"dependency/unindexed", &key));
            key_witnesses.push(DependencyKeyPrestate {
                key,
                level,
                dirty,
                control,
                projected_fanout: None,
            });
        }

        unindexed_shards.sort_unstable();
        unindexed_shards.dedup();
        let mut unindexed = Vec::with_capacity(unindexed_shards.len());
        for shard in unindexed_shards {
            let level = owner_cut.projection_shard(shard).dependency_unindexed;
            unindexed.push((shard, level));
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
            unindexed,
        })
    }

    fn point_rows_are_fresh(
        &self,
        entries: &ShardedOwnerMap,
        relation_cut: &ShardedDependencyRelationWriteCut<'_>,
        owner_cut: &ShardedOwnerWriteCut<'_>,
    ) -> bool {
        self.relations.iter().all(|expected| {
            dependency_relation_point_value_in_cut(entries, relation_cut, &expected.point)
                .is_ok_and(|value| value == expected.value)
        }) && self.keys.iter().all(|expected| {
            let level_shard = entries
                .layout
                .router
                .shard(b"dependency/level", &expected.key);
            let level_row = owner_cut.projection_shard(level_shard);
            level_row.dependency_levels.get(&expected.key).copied() == expected.level
                && level_row.dependency_dirty.get(&expected.key) == expected.dirty.as_ref()
        }) && self.unindexed.iter().all(|(shard, expected)| {
            owner_cut.projection_shard(*shard).dependency_unindexed == *expected
        })
    }

    fn aggregate_rows_are_fresh_under_gate(
        &self,
        entries: &ShardedOwnerMap,
        gates: &DependencyGateCut<'_>,
        control: &DependencyControlDelta,
    ) -> bool {
        let control_fresh = self.keys.iter().all(|expected| match &expected.control {
            None => true,
            Some(DependencyControlKeyPrestate::Event {
                has_accepted_consumers,
            }) => dependency_has_matching_under_gate(
                entries,
                gates,
                &expected.key,
                DependencyRelationFilter::Accepted,
            )
            .is_ok_and(|current| current == *has_accepted_consumers),
            Some(DependencyControlKeyPrestate::Maintenance { scope, next }) => {
                dependency_next_owner_under_gate(
                    entries,
                    gates,
                    &expected.key,
                    *scope,
                    expected
                        .dirty
                        .as_ref()
                        .and_then(|dirty| dirty.cursor.as_ref()),
                )
                .is_ok_and(|current| &current == next)
            }
        });
        let _ = control;
        control_fresh
    }

    fn bind_projected_fanout_under_gate(
        &mut self,
        entries: &ShardedOwnerMap,
        gates: &DependencyGateCut<'_>,
        control: &DependencyControlDelta,
        relations: &[DependencyRelationChange],
    ) -> Result<(), DependencyError> {
        for expected in &mut self.keys {
            let key_relations = dependency_relation_changes_for_key(relations, &expected.key);
            let relation_sensitive = key_relations.iter().any(|relation| {
                relation.after.is_none()
                    || relation.before.is_some_and(|value| value.waiting)
                        != relation.after.is_some_and(|value| value.waiting)
            });
            if control.contains_key(&expected.key) || relation_sensitive {
                expected.projected_fanout = Some(projected_dependency_fanout_under_gate(
                    entries,
                    gates,
                    &expected.key,
                    key_relations,
                )?);
            }
        }
        Ok(())
    }

    fn has_projected_fanout(&self) -> bool {
        self.keys
            .iter()
            .any(|expected| expected.projected_fanout.is_some())
    }

    fn is_fresh_in_apply_cut(
        &self,
        entries: &ShardedOwnerMap,
        relation_cut: &ShardedDependencyRelationWriteCut<'_>,
        owner_cut: &ShardedOwnerWriteCut<'_>,
    ) -> bool {
        self.point_rows_are_fresh(entries, relation_cut, owner_cut)
    }
}

fn dependency_relation_point_value_in_cut(
    entries: &ShardedOwnerMap,
    cut: &ShardedDependencyRelationWriteCut<'_>,
    point: &DependencyRelationPoint,
) -> Result<Option<DependencyRelationValue>, DependencyError> {
    let shard = cut.projection_shard(dependency_relation_shard(entries, &point.owner));
    Ok(shard
        .rows
        .get(&point.key)
        .and_then(|relations| relations.entries.get(&point.owner).copied()))
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

    fn extend_relation_read_support(
        &self,
        entries: &ShardedOwnerMap,
        support: &mut ShardReadSupport,
    ) {
        if !self.keys.is_empty() {
            support.insert(dependency_relation_shard(entries, &self.owner));
        }
    }

    fn extend_owner_read_support(&self, entries: &ShardedOwnerMap, support: &mut ShardReadSupport) {
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

    fn is_fresh(
        &self,
        entries: &ShardedOwnerMap,
        relation_cut: &ShardedDependencyRelationWriteCut<'_>,
        owner_cut: &ShardedOwnerWriteCut<'_>,
    ) -> bool {
        self.keys.iter().all(|expected| {
            let consumer_shard = dependency_relation_shard(entries, &self.owner);
            let shard = relation_cut.projection_shard(consumer_shard);
            let owner_matches = shard
                .rows
                .get(&expected.key)
                .and_then(|relations| relations.entries.get(&self.owner))
                .map(|value| value.phase)
                == expected.owner_phase;
            let level_row = owner_cut.projection_shard(
                entries
                    .layout
                    .router
                    .shard(b"dependency/level", &expected.key),
            );
            let unindexed = owner_cut
                .projection_shard(
                    entries
                        .layout
                        .router
                        .shard(b"dependency/unindexed", &expected.key),
                )
                .dependency_unindexed;
            owner_matches
                && level_row.dependency_levels.get(&expected.key).copied() == expected.level
                && level_row.dependency_dirty.get(&expected.key) == expected.dirty.as_ref()
                && unindexed == expected.unindexed
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DependencyPrepareError {
    Stale,
    Projection,
}

/// Linear terminal of one applied dependency batch. `Activated` is the exact
/// receipt for maintenance becoming newly runnable. Deliberately not `Clone`
/// or `Copy`.
#[derive(Debug, PartialEq, Eq)]
#[must_use = "dependency Apply outcome must reach committed maintenance-wake publication"]
pub(super) enum DependencyApplyOutcome {
    Quiet,
    Activated,
}

/// Move-only capability whose complete aggregate premises were checked while
/// its dependency gates were held. Apply rechecks every point in one owner cut
/// and writes the stable consumer-owned relation exactly once.
#[must_use = "a prepared dependency batch has no effect until applied"]
pub(super) struct PreparedDependencyBatch {
    entries: ShardedOwnerMap,
    maintenance: std::sync::Arc<DependencyMaintenanceState>,
    delta: DependencyBatchDelta,
}

#[derive(Clone)]
struct DependencyRelationChange {
    point: DependencyRelationPoint,
    before: Option<DependencyRelationValue>,
    after: Option<DependencyRelationValue>,
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
}

#[derive(Debug, Default)]
struct DependencyMaintenanceState {
    cursor: Mutex<Option<DependencyKey>>,
}

impl DependencyFrontier {
    pub(super) fn for_entries(entries: &ShardedOwnerMap) -> Self {
        Self {
            entries: entries.clone(),
            maintenance: std::sync::Arc::new(DependencyMaintenanceState::default()),
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
            .cloned()
    }

    fn next_dirty_key(&self) -> Result<Option<DependencyKey>, DependencyError> {
        let cursor = self.maintenance.cursor.lock().clone();
        let mut after: Option<DependencyKey> = None;
        let mut first: Option<DependencyKey> = None;
        for shard in self.entries.layout.shards.iter() {
            let shard = shard.read();
            if let Some(key) = shard.dependency_dirty.keys().next().cloned() {
                first = Some(match first.take() {
                    Some(current) => current.min(key),
                    None => key,
                });
            }
            if let Some(cursor) = &cursor {
                let next = shard
                    .dependency_dirty
                    .range((Excluded(cursor), Unbounded))
                    .next()
                    .map(|(key, _)| key.clone());
                if let Some(key) = next {
                    after = Some(match after.take() {
                        Some(current) => current.min(key),
                        None => key,
                    });
                }
            }
        }
        Ok(after.or(first))
    }

    fn with_relation<T>(
        &self,
        key: &DependencyKey,
        owner: &RawTxHash,
        read: impl FnOnce(Option<&DependencyRelationSet>) -> T,
    ) -> T {
        let shard = self
            .entries
            .dependency_relation_shard_read(dependency_relation_shard(&self.entries, owner));
        read(dependency_relation_set(&shard, key))
    }

    fn consumers(
        &self,
        key: &DependencyKey,
    ) -> Result<Option<BTreeSet<RawTxHash>>, DependencyError> {
        let mut support = DependencyGateSupport::default();
        support.write(dependency_key_gate(&self.entries, key));
        let gates = self.entries.dependency_gate_cut(support);
        dependency_consumers_under_gate(
            &self.entries,
            &gates,
            key,
            DependencyRelationFilter::Consumers,
            crate::constants::MAX_POOL_MUTATION_CANDIDATES,
        )
    }

    pub(super) fn observe_consumers_bounded(
        &self,
        key: DependencyKey,
        limit: usize,
    ) -> Result<(Option<BTreeSet<RawTxHash>>, ObservedDependencyConsumerRead), DependencyError>
    {
        let mut support = DependencyGateSupport::default();
        support.write(dependency_key_gate(&self.entries, &key));
        let gates = self.entries.dependency_gate_cut(support);
        let visible = dependency_consumers_under_gate(
            &self.entries,
            &gates,
            &key,
            DependencyRelationFilter::Consumers,
            limit,
        )?;
        Ok((
            visible.clone(),
            ObservedDependencyConsumerRead {
                key,
                kind: DependencyConsumerObservationKind::General,
                visible,
                accepted_over_limit: None,
            },
        ))
    }

    pub(super) fn observe_accepted_consumers_bounded_or_over_limit(
        &self,
        key: DependencyKey,
        limit: usize,
    ) -> Result<ObservedAcceptedConsumers, DependencyError> {
        let mut support = DependencyGateSupport::default();
        support.write(dependency_key_gate(&self.entries, &key));
        let gates = self.entries.dependency_gate_cut(support);
        match dependency_consumers_under_gate(
            &self.entries,
            &gates,
            &key,
            DependencyRelationFilter::Accepted,
            limit,
        ) {
            Err(DependencyError::Fanout) => Ok(ObservedAcceptedConsumers::OverLimit(
                ObservedDependencyConsumerRead {
                    key,
                    kind: DependencyConsumerObservationKind::Accepted,
                    visible: None,
                    accepted_over_limit: Some(limit),
                },
            )),
            Ok(visible) => Ok(ObservedAcceptedConsumers::Within {
                visible: visible.clone(),
                receipt: ObservedDependencyConsumerRead {
                    key,
                    kind: DependencyConsumerObservationKind::Accepted,
                    visible,
                    accepted_over_limit: None,
                },
            }),
            Err(error) => Err(error),
        }
    }

    fn waiters(&self, key: &DependencyKey) -> Result<Option<BTreeSet<RawTxHash>>, DependencyError> {
        let mut support = DependencyGateSupport::default();
        support.write(dependency_key_gate(&self.entries, key));
        let gates = self.entries.dependency_gate_cut(support);
        dependency_consumers_under_gate(
            &self.entries,
            &gates,
            key,
            DependencyRelationFilter::Waiters,
            crate::constants::MAX_POOL_MUTATION_CANDIDATES,
        )
    }

    fn next_visible_owner(
        &self,
        key: &DependencyKey,
        scope: DirtyScope,
        cursor: Option<&RawTxHash>,
    ) -> Result<Option<RawTxHash>, DependencyError> {
        let mut support = DependencyGateSupport::default();
        support.write(dependency_key_gate(&self.entries, key));
        let gates = self.entries.dependency_gate_cut(support);
        dependency_next_owner_under_gate(&self.entries, &gates, key, scope, cursor)
    }

    fn level(&self, key: &DependencyKey) -> Option<DependencyLevel> {
        self.routed_shard(self.shard(b"dependency/level", key))
            .read()
            .dependency_levels
            .get(key)
            .copied()
    }

    fn unindexed_level(&self, key: &DependencyKey) -> UnindexedDependencyLevel {
        self.routed_shard(self.shard(b"dependency/unindexed", key))
            .read()
            .dependency_unindexed
    }

    fn consumer_contains(&self, key: &DependencyKey, owner: &RawTxHash) -> bool {
        self.with_relation(key, owner, |relations| {
            relations.is_some_and(|relations| {
                relations.contains_visible_matching(owner, DependencyRelationFilter::Consumers)
            })
        })
    }

    fn waiter_contains(&self, key: &DependencyKey, owner: &RawTxHash) -> bool {
        self.with_relation(key, owner, |relations| {
            relations.is_some_and(|relations| {
                relations.contains_visible_matching(owner, DependencyRelationFilter::Waiters)
            })
        })
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
        let mut removed = Vec::with_capacity(usize::from(before.is_some()));
        let mut added = Vec::with_capacity(usize::from(after.is_some()));
        let mut unchanged_slots = Vec::with_capacity(usize::from(unchanged.is_some()));
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
    pub(in crate::authority) fn dependency_gate_support(
        &self,
        entries: &ShardedOwnerMap,
    ) -> DependencyGateSupport {
        dependency_gate_support_for(entries, &self.relation_changes, &self.control)
    }

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
                DependencyPrepareError::Stale | DependencyPrepareError::Projection => {
                    DependencyError::Projection
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
        self.observed.reserve(evidence.len());
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
        let evidence_set = vec![evidence];
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

    fn is_primary_accepted_insertion_only_shape_with_relations(
        &self,
        relations: &[DependencyRelationChange],
    ) -> bool {
        if self.added.is_empty()
            || !self.removed.is_empty()
            || !self.observed.is_empty()
            || !self.unchanged.is_empty()
            || !self.settlement_evidence.is_empty()
            || !matches!(self.control, DependencyControlDelta::None)
            || self.added.iter().any(|slot| {
                slot.phase != DependencyConsumerPhase::Accepted || slot.waiting.is_some()
            })
        {
            return false;
        }
        let Some(expected_relations) = self.added.iter().try_fold(0usize, |count, slot| {
            count.checked_add(slot.dependencies.len())
        }) else {
            return false;
        };
        relations.len() == expected_relations
            && relations.iter().all(|change| {
                change.before.is_none()
                    && change.after
                        == Some(DependencyRelationValue {
                            phase: DependencyConsumerPhase::Accepted,
                            waiting: false,
                        })
                    && self
                        .added
                        .binary_search_by(|slot| slot.hash.cmp(&change.point.owner))
                        .ok()
                        .and_then(|position| self.added.get(position))
                        .is_some_and(|slot| slot.dependencies.contains(&change.point.key))
            })
    }

    fn is_primary_accepted_insertion_only_shape(&self) -> bool {
        self.is_primary_accepted_insertion_only_shape_with_relations(&self.relation_changes)
    }

    #[cfg(test)]
    pub(in crate::authority) fn primary_accepted_insertion_shape_for_foundation(&self) -> bool {
        self.is_primary_accepted_insertion_only_shape()
    }

    pub(super) fn is_retained_insertion_shape(&self) -> bool {
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
                change.before.is_none()
                    && change.after
                        == Some(DependencyRelationValue {
                            phase: DependencyConsumerPhase::Other,
                            waiting: false,
                        })
            })
    }

    /// A Ready settlement changes only this owner's consumer phase from Other
    /// to Accepted while preserving the exact dependency key set. Its stable
    /// consumer-owned relation rows carry the owner-local freshness fact;
    /// level, dirty and unindexed evidence remain final-cut reads.
    fn is_ready_phase_only_shape_with_relations(
        &self,
        relations: &[DependencyRelationChange],
    ) -> bool {
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
            let Some(total) = expected_relations.checked_add(before.dependencies.len()) else {
                return false;
            };
            expected_relations = total;
        }
        relations.len() == expected_relations
            && relations.iter().all(|change| {
                let Some(slot) = self
                    .removed
                    .binary_search_by(|slot| slot.hash.cmp(&change.point.owner))
                    .ok()
                    .and_then(|position| self.removed.get(position))
                else {
                    return false;
                };
                slot.dependencies.contains(&change.point.key)
                    && change.before
                        == Some(DependencyRelationValue {
                            phase: DependencyConsumerPhase::Other,
                            waiting: false,
                        })
                    && change.after
                        == Some(DependencyRelationValue {
                            phase: DependencyConsumerPhase::Accepted,
                            waiting: false,
                        })
            })
    }

    pub(super) fn is_ready_phase_only_shape(&self) -> bool {
        self.is_ready_phase_only_shape_with_relations(&self.relation_changes)
    }

    fn relation_points(
        slots: &[DependencySlot],
    ) -> Result<Vec<(DependencyRelationPoint, DependencyRelationValue)>, DependencyPrepareError>
    {
        let capacity = slots.iter().try_fold(0usize, |count, slot| {
            count.checked_add(slot.dependencies.len())
        });
        let mut points = Vec::with_capacity(capacity.ok_or(DependencyPrepareError::Projection)?);
        for slot in slots {
            points.extend(slot.dependencies.keys().iter().cloned().map(|key| {
                let value = DependencyRelationValue::for_slot(slot, &key);
                (
                    DependencyRelationPoint {
                        key,
                        owner: slot.hash.clone(),
                    },
                    value,
                )
            }));
        }
        points.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        if points
            .array_windows::<2>()
            .any(|[left, right]| left.0 == right.0)
        {
            return Err(DependencyPrepareError::Projection);
        }
        Ok(points)
    }

    fn compile_relation_changes(
        &self,
    ) -> Result<Vec<DependencyRelationChange>, DependencyPrepareError> {
        let before = Self::relation_points(&self.removed)?;
        let after = Self::relation_points(&self.added)?;
        let capacity = before
            .len()
            .checked_add(after.len())
            .ok_or(DependencyPrepareError::Projection)?;
        let mut changes = Vec::with_capacity(capacity);
        let mut before = before.iter().peekable();
        let mut after = after.iter().peekable();
        loop {
            match (before.peek().copied(), after.peek().copied()) {
                (Some((left, before_value)), Some((right, after_value))) if left == right => {
                    if before_value != after_value {
                        changes.push(DependencyRelationChange {
                            point: left.clone(),
                            before: Some(*before_value),
                            after: Some(*after_value),
                        });
                    }
                    before.next();
                    after.next();
                }
                (Some((left, before_value)), Some((right, _))) if left < right => {
                    changes.push(DependencyRelationChange {
                        point: left.clone(),
                        before: Some(*before_value),
                        after: None,
                    });
                    before.next();
                }
                (Some(_), Some((right, after_value))) => {
                    changes.push(DependencyRelationChange {
                        point: right.clone(),
                        before: None,
                        after: Some(*after_value),
                    });
                    after.next();
                }
                (Some((left, before_value)), None) => {
                    changes.push(DependencyRelationChange {
                        point: left.clone(),
                        before: Some(*before_value),
                        after: None,
                    });
                    before.next();
                }
                (None, Some((right, after_value))) => {
                    changes.push(DependencyRelationChange {
                        point: right.clone(),
                        before: None,
                        after: Some(*after_value),
                    });
                    after.next();
                }
                (None, None) => break,
            }
        }
        Ok(changes)
    }

    #[cfg(test)]
    fn relation_write_support(&self, entries: &ShardedOwnerMap) -> ShardWriteSupport {
        let mut support = ShardWriteSupport::default();
        for change in &self.relation_changes {
            support.insert(dependency_relation_shard(entries, &change.point.owner));
        }
        support
    }

    #[cfg(test)]
    pub(in crate::authority) fn relation_write_support_for_foundation(
        &self,
        entries: &ShardedOwnerMap,
    ) -> ShardWriteSupport {
        self.relation_write_support(entries)
    }

    pub(in crate::authority) fn relation_read_support(
        &self,
        entries: &ShardedOwnerMap,
    ) -> ShardReadSupport {
        let mut support = ShardReadSupport::default();
        for relation in &self.prestate.relations {
            support.insert(dependency_relation_shard(entries, &relation.point.owner));
        }
        for evidence in &self.settlement_evidence {
            evidence.extend_relation_read_support(entries, &mut support);
        }
        support
    }

    pub(in crate::authority) fn sharded_read_support(
        &self,
        entries: &ShardedOwnerMap,
    ) -> ShardReadSupport {
        let mut support = ShardReadSupport::default();
        // Owner-bank support contains only control rows. Relation premises are
        // folded separately so every joint cut has the mechanical R -> O
        // acquisition order.
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
            evidence.extend_owner_read_support(entries, &mut support);
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
            evidence.extend_owner_read_support(entries, &mut support);
        }
        support
    }

    fn primary_accepted_insertion_final_read_support(
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
        support
    }

    pub(in crate::authority) fn shared_independent_final_read_support(
        &self,
        entries: &ShardedOwnerMap,
    ) -> ShardReadSupport {
        if self.is_ready_phase_only_shape() {
            self.ready_phase_final_read_support(entries)
        } else if self.is_primary_accepted_insertion_only_shape() {
            self.primary_accepted_insertion_final_read_support(entries)
        } else {
            self.sharded_read_support(entries)
        }
    }

    fn control_transition(
        &self,
        expected: &DependencyKeyPrestate,
        relations: &[DependencyRelationChange],
    ) -> DependencyControlTransition {
        let mut transition = DependencyControlTransition {
            level: expected.level,
            dirty: expected.dirty.clone(),
            unindexed: None,
        };
        let has_consumers = expected
            .projected_fanout
            .is_none_or(|fanout| fanout.has_consumers);
        if !dependency_relation_changes_for_key(relations, &expected.key).is_empty()
            && !has_consumers
        {
            if let Some(level) = transition.level.take() {
                transition.push_unindexed(level);
            }
            transition.dirty = None;
        }

        match &self.control {
            DependencyControlDelta::Event(event) => {
                if let Some(change) = event
                    .changes
                    .binary_search_by(|change| change.key.cmp(&expected.key))
                    .ok()
                    .and_then(|position| event.changes.get(position))
                {
                    if !has_consumers {
                        transition.push_unindexed(change.level);
                        if let Some(level) = transition.level.take() {
                            transition.push_unindexed(level);
                        }
                        transition.dirty = None;
                    } else {
                        transition.level = Some(change.level);
                        if let Some(dirty) = transition.dirty.as_mut() {
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
                        } else if matches!(change.scope, DirtyScope::AllConsumers)
                            || expected
                                .projected_fanout
                                .is_some_and(|fanout| fanout.has_waiters)
                        {
                            transition.dirty = Some(DirtyDependency {
                                target: change.level.last_change,
                                scope: change.scope,
                                cursor: None,
                                pending: None,
                            });
                        }
                    }
                }
            }
            DependencyControlDelta::Maintenance(maintenance)
                if maintenance.key() == &expected.key =>
            {
                match &maintenance.step {
                    DependencyMaintenanceStep::Advance {
                        expected, cursor, ..
                    } => {
                        if has_consumers {
                            let mut next = expected.clone();
                            next.cursor = Some(cursor.clone());
                            transition.dirty = Some(next);
                        } else {
                            if let Some(level) = transition.level.take() {
                                transition.push_unindexed(level);
                            }
                            transition.dirty = None;
                        }
                    }
                    DependencyMaintenanceStep::Complete { expected, .. } => {
                        if let Some(PendingDependency { target, scope }) = expected.pending {
                            transition.dirty = Some(DirtyDependency {
                                target,
                                scope,
                                cursor: None,
                                pending: None,
                            });
                        } else {
                            transition.dirty = None;
                            if !has_consumers && let Some(level) = transition.level.take() {
                                transition.push_unindexed(level);
                            }
                        }
                    }
                }
            }
            DependencyControlDelta::None | DependencyControlDelta::Maintenance(_) => {}
        }
        transition
    }

    /// Commit the control projection derived under the same gates as the
    /// stable relation changes.
    fn apply_control_rows(
        &self,
        entries: &ShardedOwnerMap,
        cut: &mut ShardedOwnerWriteCut<'_>,
        relations: &[DependencyRelationChange],
    ) -> (Option<DependencyKey>, bool) {
        let mut maintenance_activated = false;
        for expected in &self.prestate.keys {
            let transition = self.control_transition(expected, relations);
            if expected.level != transition.level || expected.dirty != transition.dirty {
                let shard = entries
                    .layout
                    .router
                    .shard(b"dependency/level", &expected.key);
                let row = cut.projection_shard_mut(shard);
                if expected.level != transition.level {
                    replace_control_cell(
                        &mut row.dependency_levels,
                        &expected.key,
                        transition.level,
                    );
                }
                if expected.dirty != transition.dirty {
                    maintenance_activated |= expected.dirty.is_none() && transition.dirty.is_some();
                    replace_control_cell(
                        &mut row.dependency_dirty,
                        &expected.key,
                        transition.dirty,
                    );
                }
            }
            if let Some(level) = transition.unindexed {
                let shard = entries
                    .layout
                    .router
                    .shard(b"dependency/unindexed", &expected.key);
                cut.projection_shard_mut(shard)
                    .dependency_unindexed
                    .merge_unindexed(level);
            }
        }
        let maintenance_cursor = match &self.control {
            DependencyControlDelta::Maintenance(maintenance) => Some(maintenance.key().clone()),
            DependencyControlDelta::None | DependencyControlDelta::Event(_) => None,
        };
        (maintenance_cursor, maintenance_activated)
    }
}

fn dependency_gate_support_for(
    entries: &ShardedOwnerMap,
    relations: &[DependencyRelationChange],
    control: &DependencyControlDelta,
) -> DependencyGateSupport {
    let mut support = DependencyGateSupport::default();
    for change in relations {
        support.read(dependency_key_gate(entries, &change.point.key));
        let waiter_changed = change.before.is_some_and(|value| value.waiting)
            != change.after.is_some_and(|value| value.waiting);
        if change.after.is_none() || waiter_changed {
            support.write(dependency_key_gate(entries, &change.point.key));
        }
    }
    match control {
        DependencyControlDelta::None => {}
        DependencyControlDelta::Event(event) => {
            for change in &event.changes {
                support.write(dependency_key_gate(entries, &change.key));
            }
        }
        DependencyControlDelta::Maintenance(maintenance) => {
            support.write(dependency_key_gate(entries, maintenance.key()));
        }
    }
    support
}

#[expect(
    clippy::expect_used,
    reason = "the exact-key gate and final relation cut revalidate this present row immediately before the infallible Apply mutation"
)]
fn apply_stable_dependency_relations(
    entries: &ShardedOwnerMap,
    cut: &mut ShardedDependencyRelationWriteCut<'_>,
    changes: &[DependencyRelationChange],
) {
    for change in changes {
        let shard =
            cut.projection_shard_mut(dependency_relation_shard(entries, &change.point.owner));
        match change.after {
            Some(after) => {
                let relations = shard.rows.entry(change.point.key.clone()).or_default();
                debug_assert_eq!(
                    relations.entries.get(&change.point.owner).copied(),
                    change.before
                );
                relations.apply(change.point.owner.clone(), Some(after));
            }
            None => {
                let relations = shard
                    .rows
                    .get_mut(&change.point.key)
                    .expect("relation key was revalidated before the final cut opened");
                debug_assert_eq!(
                    relations.entries.get(&change.point.owner).copied(),
                    change.before
                );
                relations.apply(change.point.owner.clone(), None);
                if relations.is_empty() {
                    shard.rows.remove(&change.point.key);
                }
            }
        }
    }
}

impl PreparedDependencyBatch {
    pub(super) fn prepare_primary_replacements(
        frontier: &DependencyFrontier,
        delta: DependencyBatchDelta,
    ) -> Result<Self, DependencyPrepareError> {
        let gates = frontier
            .entries
            .dependency_gate_cut(delta.dependency_gate_support(&frontier.entries));
        Self::prepare_under_gates(frontier, delta, false, &gates)
    }

    pub(super) fn prepare_with_gates(
        frontier: &DependencyFrontier,
        delta: DependencyBatchDelta,
        gates: &DependencyGateCut<'_>,
    ) -> Result<Self, DependencyPrepareError> {
        Self::prepare_under_gates(frontier, delta, false, gates)
    }

    pub(super) fn prepare_shared_independent(
        frontier: &DependencyFrontier,
        delta: DependencyBatchDelta,
        gates: &DependencyGateCut<'_>,
    ) -> Result<Self, DependencyPrepareError> {
        let relation_only =
            delta.is_ready_phase_only_shape() || delta.is_primary_accepted_insertion_only_shape();
        Self::prepare_under_gates(frontier, delta, relation_only, gates)
    }

    fn prepare_under_gates(
        frontier: &DependencyFrontier,
        mut delta: DependencyBatchDelta,
        relation_only: bool,
        gates: &DependencyGateCut<'_>,
    ) -> Result<Self, DependencyPrepareError> {
        let entries = frontier.entries.clone();
        if !delta
            .prestate
            .aggregate_rows_are_fresh_under_gate(&entries, gates, &delta.control)
        {
            return Err(DependencyPrepareError::Stale);
        }
        let control = &delta.control;
        delta
            .prestate
            .bind_projected_fanout_under_gate(&entries, gates, control, &delta.relation_changes)
            .map_err(|error| match error {
                DependencyError::Stale => DependencyPrepareError::Stale,
                DependencyError::Fanout
                | DependencyError::Projection
                | DependencyError::SurvivingAcceptedConsumer => DependencyPrepareError::Projection,
            })?;
        if relation_only && delta.prestate.has_projected_fanout() {
            return Err(DependencyPrepareError::Projection);
        }
        Ok(Self {
            entries,
            maintenance: std::sync::Arc::clone(&frontier.maintenance),
            delta,
        })
    }

    pub(super) fn require_retained_insertion_shape(self) -> Result<Self, DependencyPrepareError> {
        if self.delta.is_retained_insertion_shape() && !self.delta.prestate.has_projected_fanout() {
            Ok(self)
        } else {
            Err(DependencyPrepareError::Projection)
        }
    }

    pub(super) fn extend_final_relation_read_support(&self, support: &mut ShardReadSupport) {
        support.include(self.delta.relation_read_support(&self.entries));
    }

    pub(super) fn extend_final_read_support(&self, support: &mut ShardReadSupport) {
        if self
            .delta
            .is_ready_phase_only_shape_with_relations(&self.delta.relation_changes)
        {
            support.include(self.delta.ready_phase_final_read_support(&self.entries));
        } else if self
            .delta
            .is_primary_accepted_insertion_only_shape_with_relations(&self.delta.relation_changes)
        {
            support.include(
                self.delta
                    .primary_accepted_insertion_final_read_support(&self.entries),
            );
        } else {
            support.include(self.delta.sharded_read_support(&self.entries));
        }
    }

    pub(super) fn extend_final_relation_write_support(&self, support: &mut ShardWriteSupport) {
        for relation in &self.delta.relation_changes {
            support.insert(dependency_relation_shard(
                &self.entries,
                &relation.point.owner,
            ));
        }
    }

    pub(super) fn extend_final_write_support(&self, support: &mut ShardWriteSupport) {
        for expected in &self.delta.prestate.keys {
            let transition = self
                .delta
                .control_transition(expected, &self.delta.relation_changes);
            if expected.level != transition.level || expected.dirty != transition.dirty {
                support.insert(
                    self.entries
                        .layout
                        .router
                        .shard(b"dependency/level", &expected.key),
                );
            }
            if transition.unindexed.is_some() {
                support.insert(
                    self.entries
                        .layout
                        .router
                        .shard(b"dependency/unindexed", &expected.key),
                );
            }
        }
    }

    pub(super) fn prestate_is_fresh(
        &self,
        relation_cut: &ShardedDependencyRelationWriteCut<'_>,
        owner_cut: &ShardedOwnerWriteCut<'_>,
    ) -> bool {
        self.delta
            .prestate
            .is_fresh_in_apply_cut(&self.entries, relation_cut, owner_cut)
            && self
                .delta
                .settlement_evidence
                .iter()
                .all(|evidence| evidence.is_fresh(&self.entries, relation_cut, owner_cut))
    }

    pub(super) fn apply_in_cut(
        self,
        relation_cut: &mut ShardedDependencyRelationWriteCut<'_>,
        owner_cut: &mut ShardedOwnerWriteCut<'_>,
    ) -> DependencyApplyOutcome {
        let Self {
            entries,
            maintenance,
            delta,
        } = self;
        apply_stable_dependency_relations(&entries, relation_cut, &delta.relation_changes);
        let (maintenance_cursor, maintenance_activated) =
            delta.apply_control_rows(&entries, owner_cut, &delta.relation_changes);
        if let Some(cursor) = maintenance_cursor {
            *maintenance.cursor.lock() = Some(cursor);
        }
        if maintenance_activated {
            DependencyApplyOutcome::Activated
        } else {
            DependencyApplyOutcome::Quiet
        }
    }

    /// The enclosing authority write lease excludes every shared writer. Gates
    /// are still acquired first so the physical lock order has one shape.
    pub(super) fn apply_exclusive(self) -> DependencyApplyOutcome {
        let entries = self.entries.clone();
        let gates = entries.dependency_gate_cut(self.delta.dependency_gate_support(&entries));
        let mut relation_reads = ShardReadSupport::default();
        let mut relation_writes = ShardWriteSupport::default();
        self.extend_final_relation_read_support(&mut relation_reads);
        self.extend_final_relation_write_support(&mut relation_writes);
        let mut relation_cut =
            entries.dependency_relation_mixed_cut(relation_reads, relation_writes);
        let mut owner_reads = ShardReadSupport::default();
        let mut owner_writes = ShardWriteSupport::default();
        self.extend_final_read_support(&mut owner_reads);
        self.extend_final_write_support(&mut owner_writes);
        let mut owner_cut = entries.mixed_cut(owner_reads, owner_writes);
        debug_assert!(self.prestate_is_fresh(&relation_cut, &owner_cut));
        let outcome = self.apply_in_cut(&mut relation_cut, &mut owner_cut);
        drop(relation_cut);
        drop(owner_cut);
        drop(gates);
        outcome
    }
}

fn dependency_relation_changes_for_key<'relations>(
    relations: &'relations [DependencyRelationChange],
    key: &DependencyKey,
) -> &'relations [DependencyRelationChange] {
    let first = relations.partition_point(|change| change.point.key < *key);
    let Some(remaining) = relations.get(first..) else {
        return &[];
    };
    let count = remaining.partition_point(|change| change.point.key == *key);
    remaining.get(..count).unwrap_or_default()
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
        let mut keys = Vec::with_capacity(capacity);
        keys.extend(baseline.keys().iter().cloned());
        if let Some(candidate) = candidate {
            keys.extend(candidate.keys().iter().cloned());
        }
        if let Some(missing) = missing {
            keys.extend(missing.keys().iter().cloned());
        }
        keys.sort_unstable();
        keys.dedup();

        let mut relation_support = ShardReadSupport::default();
        let mut owner_support = ShardReadSupport::default();
        if !keys.is_empty() {
            relation_support.insert(dependency_relation_shard(&self.entries, owner));
        }
        for key in &keys {
            owner_support.insert(self.shard(b"dependency/level", key));
            owner_support.insert(self.shard(b"dependency/unindexed", key));
        }
        let mut evidence = Vec::with_capacity(keys.len());
        let relation_cut = self
            .entries
            .dependency_relation_mixed_cut(relation_support, ShardWriteSupport::default());
        let owner_cut = self
            .entries
            .mixed_cut(owner_support, ShardWriteSupport::default());
        for key in keys {
            let consumer_shard = dependency_relation_shard(&self.entries, owner);
            let consumer_row = relation_cut.projection_shard(consumer_shard);
            let owner_phase = consumer_row
                .rows
                .get(&key)
                .and_then(|relations| relations.entries.get(owner).copied())
                .map(|value| value.phase);
            if owner_phase.is_some() != baseline.contains(&key) {
                return Err(DependencyError::Projection);
            }
            let level_shard = self.shard(b"dependency/level", &key);
            let level_row = owner_cut.projection_shard(level_shard);
            let level = level_row.dependency_levels.get(&key).copied();
            let dirty = level_row.dependency_dirty.get(&key).cloned();
            let unindexed_shard = self.shard(b"dependency/unindexed", &key);
            let unindexed = owner_cut
                .projection_shard(unindexed_shard)
                .dependency_unindexed;
            evidence.push(SettlementDependencyKeyEvidence {
                key,
                level,
                dirty,
                unindexed,
                owner_phase,
            });
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

    /// Compile availability and definitive loss from one projected final
    /// state. A key cannot be both; callers must resolve that contradiction
    /// before publishing the level transition.
    pub(super) fn plan_events(
        &self,
        available: Vec<DependencyKey>,
        lost: Vec<DependencyKey>,
        cut: DependencyCut,
    ) -> Result<Option<DependencyEntryControlDelta>, DependencyError> {
        self.plan_events_with_level(available, lost, cut, DependencyError::Projection, |key| {
            self.level(key)
        })
    }

    pub(super) fn plan_shared_events(
        &self,
        available: Vec<DependencyKey>,
        lost: Vec<DependencyKey>,
        cut: DependencyCut,
    ) -> Result<Option<DependencyEntryControlDelta>, DependencyError> {
        self.plan_events_with_level(available, lost, cut, DependencyError::Stale, |key| {
            self.level(key)
        })
    }

    fn plan_events_with_level(
        &self,
        mut available: Vec<DependencyKey>,
        mut lost: Vec<DependencyKey>,
        cut: DependencyCut,
        superseded_cut: DependencyError,
        mut level: impl FnMut(&DependencyKey) -> Option<DependencyLevel>,
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
        let mut changes = Vec::with_capacity(change_count);
        for (key, definitive_loss) in available
            .into_iter()
            .map(|key| (key, false))
            .chain(lost.into_iter().map(|key| (key, true)))
        {
            let previous = level(&key);
            if previous.is_some_and(|level| level.last_change >= cut) {
                return Err(superseded_cut);
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
            DependencyEventPlan { changes },
        )))
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

    /// Compile a primary-owner transition without treating an eager live
    /// projection read as authority. The sealed prestate is revalidated by
    /// `PreparedDependencyBatch` while its dependency gates are held; the later
    /// owner cut decides whether the semantic input is still current.
    pub(super) fn compile_primary_replacements<'entry>(
        &self,
        changes: impl IntoIterator<Item = (Option<&'entry OwnedTx>, Option<&'entry OwnedTx>)>,
    ) -> Result<DependencyBatchDelta, DependencyError> {
        self.compile_replacements(changes, VacancyPolicy::PrimaryVacancyProven, |_| Ok(true))?
            .seal_prestate(self)
    }

    fn plan_replacements_with_additions<'entry>(
        &self,
        changes: impl IntoIterator<Item = (Option<&'entry OwnedTx>, Option<&'entry OwnedTx>)>,
        vacancy: VacancyPolicy,
    ) -> Result<DependencyBatchDelta, DependencyError> {
        self.compile_replacements(changes, vacancy, |slot| Ok(self.contains(slot)))?
            .seal_prestate(self)
    }

    pub(super) fn compile_membership_replacements<'entry>(
        &self,
        changes: impl IntoIterator<Item = (Option<&'entry OwnedTx>, Option<&'entry OwnedTx>)>,
        primary_vacancy_proven: bool,
        control: DependencyEntryControlDelta,
    ) -> Result<DependencyBatchDelta, DependencyError> {
        let vacancy = if primary_vacancy_proven {
            VacancyPolicy::PrimaryVacancyProven
        } else {
            VacancyPolicy::ExistingOwnersOnly
        };
        let mut delta = self.compile_replacements(changes, vacancy, |_| Ok(true))?;
        delta.control = control.into();
        delta.seal_prestate(self)
    }

    fn compile_replacements<'entry>(
        &self,
        changes: impl IntoIterator<Item = (Option<&'entry OwnedTx>, Option<&'entry OwnedTx>)>,
        vacancy: VacancyPolicy,
        mut contains: impl FnMut(&DependencySlot) -> Result<bool, DependencyError>,
    ) -> Result<DependencyBatchDelta, DependencyError> {
        let mut input = changes.into_iter();
        let capacity = input.size_hint().1.unwrap_or(0);
        let mut changes = Vec::with_capacity(capacity);
        let mut observed = Vec::with_capacity(capacity);
        for (before, after) in input.by_ref() {
            let before = before.map(DependencySlot::from_owner).transpose()?;
            let after = after.map(DependencySlot::from_owner).transpose()?;
            if before == after {
                if let Some(slot) = before {
                    if !contains(&slot)? {
                        return Err(DependencyError::Projection);
                    }
                    if observed.len() == observed.capacity() {
                        observed.reserve(1);
                    }
                    observed.push(slot);
                }
                continue;
            }
            if changes.len() == changes.capacity() {
                changes.reserve(1);
            }
            changes.push((before, after));
        }
        let mut removed = Vec::with_capacity(changes.len());
        let mut added = Vec::with_capacity(changes.len());
        for (before, after) in changes {
            if let Some(before) = before {
                if !contains(&before)? {
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
        Ok(DependencyBatchDelta {
            removed,
            added,
            observed,
            unchanged: Vec::new(),
            relation_changes: Vec::new(),
            settlement_evidence: Vec::new(),
            control: DependencyControlDelta::default(),
            prestate: DependencyBatchPrestate::default(),
        })
    }

    fn contains(&self, slot: &DependencySlot) -> bool {
        slot.dependencies
            .keys()
            .iter()
            .all(|key| self.consumer_contains(key, &slot.hash))
            && slot.waiting.as_ref().is_none_or(|observed| {
                observed
                    .keys()
                    .all(|key| self.waiter_contains(key, &slot.hash))
            })
    }

    #[cfg(test)]
    fn attach(&self, slot: &DependencySlot) {
        for key in slot.dependencies.keys() {
            let value = DependencyRelationValue::for_slot(slot, key);
            let shard = dependency_relation_shard(&self.entries, &slot.hash);
            let mut writes = ShardWriteSupport::default();
            writes.insert(shard);
            let mut cut = self
                .entries
                .dependency_relation_mixed_cut(ShardReadSupport::default(), writes);
            let relations = cut
                .projection_shard_mut(shard)
                .rows
                .entry(key.clone())
                .or_default();
            relations.apply(slot.hash.clone(), Some(value));
        }
    }
}

#[cfg(test)]
#[path = "tests/support/dependency.rs"]
pub(in crate::authority) mod test_support;
