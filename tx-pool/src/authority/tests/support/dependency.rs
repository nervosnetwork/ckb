use super::*;
use crate::authority::{shard::AuthorityShardRouter, state::ApplySequence};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) struct DependencyEvidenceLevelInput {
    pub(in crate::authority) key: DependencyKey,
    pub(in crate::authority) last_change: DependencyCut,
    pub(in crate::authority) last_definitive_loss: Option<DependencyCut>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::authority) struct UnindexedDependencyLevelInput {
    pub(in crate::authority) last_change: Option<DependencyCut>,
    pub(in crate::authority) last_definitive_loss: Option<DependencyCut>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::authority) struct DependencyMaintenanceRank(usize);

impl DependencyMaintenanceRank {
    pub(in crate::authority) const fn value(self) -> usize {
        self.0
    }

    pub(in crate::authority) const fn strictly_decreases_to(self, after: Self) -> bool {
        after.0 < self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum DependencyMaintenanceRankError {
    Arithmetic,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) struct DependencySnapshot {
    consumers: BTreeMap<DependencyKey, BTreeSet<RawTxHash>>,
    waiters: BTreeMap<DependencyKey, BTreeSet<RawTxHash>>,
    keys_by_origin: BTreeMap<DependencyOrigin, BTreeSet<DependencyKey>>,
    levels: BTreeMap<DependencyKey, DependencyLevel>,
    dirty: BTreeMap<DependencyKey, DirtyDependency>,
    dirty_cursor: Option<DependencyKey>,
    unindexed: UnindexedDependencyLevel,
}

impl DependencySnapshot {
    /// Collapse the canonical per-owner sequence interval onto the single
    /// stamp of its observationally equivalent atomic batch, then compare the
    /// complete dependency state. This retains exact keys, consumers,
    /// waiters, dirty progress and loss kinds; only the internal serialization
    /// points inside the no-interleave reference are quotiented.
    pub(in crate::authority) fn equivalent_after_atomic_stamp_compaction(
        &self,
        canonical: &Self,
        batch: DependencyCut,
        canonical_next: DependencyCut,
    ) -> bool {
        fn compact(
            cut: DependencyCut,
            batch: DependencyCut,
            canonical_next: DependencyCut,
        ) -> DependencyCut {
            if cut >= batch && cut < canonical_next {
                batch
            } else {
                cut
            }
        }

        let mut canonical = canonical.clone();
        for level in canonical.levels.values_mut() {
            level.last_change = compact(level.last_change, batch, canonical_next);
            level.last_definitive_loss = level
                .last_definitive_loss
                .map(|cut| compact(cut, batch, canonical_next));
        }
        for dirty in canonical.dirty.values_mut() {
            dirty.target = compact(dirty.target, batch, canonical_next);
            if let Some(pending) = &mut dirty.pending {
                pending.target = compact(pending.target, batch, canonical_next);
            }
        }
        canonical.unindexed.last_change = canonical
            .unindexed
            .last_change
            .map(|cut| compact(cut, batch, canonical_next));
        canonical.unindexed.last_definitive_loss = canonical
            .unindexed
            .last_definitive_loss
            .map(|cut| compact(cut, batch, canonical_next));
        self == &canonical
    }
}

impl DirtyScope {
    fn requires_definitive_loss(self) -> bool {
        match self {
            Self::ExistingWaiters => false,
            Self::AllConsumers => true,
        }
    }
}

impl DependencyFrontier {
    pub(in crate::authority) fn physical_origin_key_exists_for_foundation(
        &self,
        origin: &DependencyOrigin,
        key: &DependencyKey,
    ) -> bool {
        let shard = dependency_origin_shard(&self.entries, origin);
        self.entries.layout.shards[shard]
            .read()
            .dependency_relations
            .get(origin)
            .and_then(|row| row.key(key))
            .is_some()
    }

    fn empty_batch_for_foundation(control: DependencyControlDelta) -> DependencyBatchDelta {
        DependencyBatchDelta {
            removed: Vec::new(),
            added: Vec::new(),
            observed: Vec::new(),
            unchanged: Vec::new(),
            relation_changes: Vec::new(),
            settlement_evidence: Vec::new(),
            control,
            prestate: DependencyBatchPrestate::default(),
        }
    }

    pub(in crate::authority) fn relation_publish_before_cleanup_for_foundation() -> bool {
        let owner = RawTxHash(ckb_types::packed::Byte32::new([0xa1; 32]));
        let visibility = StagedIngressVisibility::hidden();
        let mut set = DependencyRelationSet::default();
        if set
            .stage(owner.clone(), DependencyRelationAction::Insert, &visibility)
            .is_err()
            || !set.contains_physical(&owner)
            || set.contains_visible(&owner)
        {
            return false;
        }
        visibility.activate_for_dependency_foundation();
        let published_before_cleanup =
            set.contains_physical(&owner) && set.contains_visible(&owner) && set.staged_len() == 1;
        let cleaned = set
            .finish_owned_stage(&owner, DependencyRelationAction::Insert, &visibility)
            .is_ok_and(|owned| owned)
            && set.contains_visible(&owner)
            && set.staged_len() == 0;
        published_before_cleanup && cleaned
    }

    pub(in crate::authority) fn relation_stage_is_exclusive_until_cleanup_for_foundation() -> bool {
        let owner = RawTxHash(ckb_types::packed::Byte32::new([0xa2; 32]));
        let predecessor = StagedIngressVisibility::hidden();
        let successor = StagedIngressVisibility::hidden();
        let mut set = DependencyRelationSet::default();
        if set
            .stage(
                owner.clone(),
                DependencyRelationAction::Insert,
                &predecessor,
            )
            .is_err()
        {
            return false;
        }
        let hidden_takeover_rejected = set
            .stage(owner.clone(), DependencyRelationAction::Insert, &successor)
            .is_err()
            && set.owns_stage(&owner, DependencyRelationAction::Insert, &predecessor);
        predecessor.activate_for_dependency_foundation();
        let published_takeover_rejected = set
            .stage(owner.clone(), DependencyRelationAction::Retire, &successor)
            .is_err();
        let predecessor_cleaned = set
            .finish_owned_stage(&owner, DependencyRelationAction::Insert, &predecessor)
            .is_ok_and(|owned| owned)
            && set.contains_visible(&owner)
            && set.staged_len() == 0;
        let successor_staged_after_cleanup = set
            .stage(owner.clone(), DependencyRelationAction::Retire, &successor)
            .is_ok()
            && set.owns_stage(&owner, DependencyRelationAction::Retire, &successor);
        let successor_rolled_back = set
            .finish_owned_stage(&owner, DependencyRelationAction::Retire, &successor)
            .is_ok_and(|owned| owned)
            && set.contains_visible(&owner)
            && set.staged_len() == 0;
        hidden_takeover_rejected
            && published_takeover_rejected
            && predecessor_cleaned
            && successor_staged_after_cleanup
            && successor_rolled_back
    }

    pub(in crate::authority) fn origin_stage_orders_for_foundation() -> bool {
        let first = RawTxHash(ckb_types::packed::Byte32::new([0xa3; 32]));
        let second = RawTxHash(ckb_types::packed::Byte32::new([0xa4; 32]));
        let successor = RawTxHash(ckb_types::packed::Byte32::new([0xa5; 32]));
        let first_wave = StagedIngressVisibility::hidden();
        let mut set = DependencyRelationSet::default();
        if set
            .stage(first.clone(), DependencyRelationAction::Insert, &first_wave)
            .is_err()
            || set
                .stage(
                    second.clone(),
                    DependencyRelationAction::Insert,
                    &first_wave,
                )
                .is_err()
        {
            return false;
        }
        first_wave.activate_for_dependency_foundation();
        for owner in [&first, &second] {
            if !set
                .finish_owned_stage(owner, DependencyRelationAction::Insert, &first_wave)
                .is_ok_and(|owned| owned)
            {
                return false;
            }
        }
        let transition = StagedIngressVisibility::hidden();
        if set
            .stage(first.clone(), DependencyRelationAction::Retire, &transition)
            .is_err()
            || set
                .stage(
                    successor.clone(),
                    DependencyRelationAction::Insert,
                    &transition,
                )
                .is_err()
            || !set.contains_visible(&first)
            || set.contains_visible(&successor)
        {
            return false;
        }
        transition.activate_for_dependency_foundation();
        !set.contains_visible(&first)
            && set.contains_visible(&successor)
            && set
                .finish_owned_stage(&first, DependencyRelationAction::Retire, &transition)
                .is_ok_and(|owned| owned)
            && set
                .finish_owned_stage(&successor, DependencyRelationAction::Insert, &transition)
                .is_ok_and(|owned| owned)
            && !set.contains_physical(&first)
            && set.contains_visible(&second)
            && set.contains_visible(&successor)
    }

    pub(in crate::authority) fn relation_stage_budget_saturates_for_foundation() -> bool {
        let entries = ShardedOwnerMap::new(AuthorityShardRouter::new());
        let frontier = Self::for_entries(&entries, 2);
        let Ok(_held) = frontier.stage_bank.try_acquire(2) else {
            return false;
        };
        frontier.stage_bank.try_acquire(1).err() == Some(DependencyStageError::Capacity)
    }

    pub(in crate::authority) fn origin_mid_collect_change_is_stale_for_foundation() -> bool {
        let owner = RawTxHash(ckb_types::packed::Byte32::new([0xa6; 32]));
        let visibility = StagedIngressVisibility::hidden();
        let mut set = DependencyRelationSet::default();
        if set
            .stage(owner, DependencyRelationAction::Insert, &visibility)
            .is_err()
        {
            return false;
        }
        let mut receipt = DependencyVisibilityReceipt::default();
        if set.observe_has_visible(&mut receipt) != Ok(false) || !receipt.is_current() {
            return false;
        }
        visibility.activate_for_dependency_foundation();
        !receipt.is_current()
    }

    pub(in crate::authority) fn control_mid_collect_change_is_stale_for_foundation() -> bool {
        let visibility = StagedIngressVisibility::hidden();
        let before = DependencyLevel {
            last_change: DependencyCut(ApplySequence(1)),
            last_definitive_loss: None,
        };
        let after = DependencyLevel {
            last_change: DependencyCut(ApplySequence(2)),
            last_definitive_loss: Some(DependencyCut(ApplySequence(2))),
        };
        let cell =
            DependencyControlCell::Staged(std::sync::Arc::new(StagedDependencyControlState {
                before: Some(before),
                after: Some(after),
                visibility: visibility.clone(),
            }));
        let mut receipt = DependencyVisibilityReceipt::default();
        if receipt.observe_control(&cell) != Ok(Some(&before)) || !receipt.is_current() {
            return false;
        }
        visibility.activate_for_dependency_foundation();
        cell.logical() == Some(&after) && !receipt.is_current()
    }

    pub(in crate::authority) fn dirty_successor_skips_hidden_control_for_foundation() -> bool {
        let entries = ShardedOwnerMap::new(AuthorityShardRouter::new());
        let frontier = Self::for_entries(&entries, usize::MAX);
        let mut keys = [
            DependencyKey::Cell(ckb_types::packed::OutPoint::new(
                ckb_types::packed::Byte32::new([0xb1; 32]),
                0,
            )),
            DependencyKey::Cell(ckb_types::packed::OutPoint::new(
                ckb_types::packed::Byte32::new([0xb2; 32]),
                0,
            )),
        ];
        keys.sort_unstable();
        let hidden_key = keys[0].clone();
        let stable_key = keys[1].clone();
        let dirty = DirtyDependency {
            target: DependencyCut(ApplySequence(1)),
            scope: DirtyScope::AllConsumers,
            cursor: None,
            pending: None,
        };
        let visibility = StagedIngressVisibility::hidden();
        let hidden_shard = frontier.shard(b"dependency/level", &hidden_key);
        {
            let mut shard = frontier.entries.layout.shards[hidden_shard].write();
            shard.dependency_dirty.insert(
                hidden_key.clone(),
                DependencyControlCell::Staged(std::sync::Arc::new(StagedDependencyControlState {
                    before: None,
                    after: Some(dirty.clone()),
                    visibility: visibility.clone(),
                })),
            );
            shard.dependency_dirty_staged = 1;
        }
        let stable_shard = frontier.shard(b"dependency/level", &stable_key);
        frontier.entries.layout.shards[stable_shard]
            .write()
            .dependency_dirty
            .insert(stable_key.clone(), DependencyControlCell::Stable(dirty));
        if frontier.next_dirty_key() != Ok(Some(stable_key)) {
            return false;
        }
        visibility.activate_for_dependency_foundation();
        frontier.next_dirty_key() == Ok(Some(hidden_key))
    }

    pub(in crate::authority) fn relation_control_ordering_for_foundation() -> bool {
        let entries = ShardedOwnerMap::new(AuthorityShardRouter::new());
        let frontier = Self::for_entries(&entries, usize::MAX);
        let key = DependencyKey::Cell(ckb_types::packed::OutPoint::new(
            ckb_types::packed::Byte32::new([0xa7; 32]),
            0,
        ));
        let owner = RawTxHash(ckb_types::packed::Byte32::new([0xa8; 32]));
        let origin = key.origin();
        let shard = dependency_origin_shard(&frontier.entries, &origin);
        if !frontier.entries.layout.shards[shard]
            .write()
            .dependency_relations
            .entry(origin)
            .or_default()
            .stable_insert(
                key.clone(),
                DependencyRelationTarget::OtherConsumer,
                owner.clone(),
            )
        {
            return false;
        }
        let cut = DependencyCut(ApplySequence(7));
        let control = match frontier.plan_events(Vec::new(), vec![key.clone()], cut) {
            Ok(Some(control)) => control,
            Ok(None) | Err(_) => return false,
        };
        frontier.apply_control_in_exact_cut_for_reference(control);
        let ticket = match frontier.next_maintenance() {
            Ok(Some(ticket)) if ticket.hash.as_ref() == Some(&owner) => ticket,
            Ok(None) | Ok(Some(_)) | Err(_) => return false,
        };
        let visibility = StagedIngressVisibility::hidden();
        {
            let mut row = frontier.entries.layout.shards[shard].write();
            let Some(set) = row
                .dependency_relations
                .get_mut(&key.origin())
                .and_then(|origin| origin.keys.get_mut(&key))
                .map(|row| &mut row.consumers.other)
            else {
                return false;
            };
            if set
                .stage(owner, DependencyRelationAction::Retire, &visibility)
                .is_err()
            {
                return false;
            }
        }
        let before_publish = frontier.maintenance_ticket_is_current(&ticket);
        visibility.activate_for_dependency_foundation();
        before_publish && !frontier.maintenance_ticket_is_current(&ticket)
    }

    pub(in crate::authority) fn direct_event_level_rebase_is_stale_for_foundation() -> bool {
        let entries = ShardedOwnerMap::new(AuthorityShardRouter::new());
        let frontier = Self::for_entries(&entries, usize::MAX);
        let key = DependencyKey::Cell(ckb_types::packed::OutPoint::new(
            ckb_types::packed::Byte32::new([0xe1; 32]),
            0,
        ));
        let control = frontier
            .plan_events(
                vec![key.clone()],
                Vec::new(),
                DependencyCut(ApplySequence(1)),
            )
            .expect("the first dependency event plans")
            .expect("one key produces one event");
        frontier
            .replace_level(
                key,
                DependencyLevel {
                    last_change: DependencyCut(ApplySequence(2)),
                    last_definitive_loss: Some(DependencyCut(ApplySequence(2))),
                },
            )
            .expect("the fixture level row is stable");
        matches!(
            Self::empty_batch_for_foundation(control.into()).seal_prestate(&frontier),
            Err(DependencyError::Stale)
        )
    }

    pub(in crate::authority) fn direct_event_origin_growth_is_stale_for_foundation() -> bool {
        let entries = ShardedOwnerMap::new(AuthorityShardRouter::new());
        let frontier = Self::for_entries(&entries, usize::MAX);
        let hash = ckb_types::packed::Byte32::new([0xe2; 32]);
        let key = DependencyKey::Cell(ckb_types::packed::OutPoint::new(hash.clone(), 0));
        let origin = DependencyOrigin::Transaction(RawTxHash(hash.clone()));
        let control = frontier
            .plan_events_with_origin_expectation(
                vec![key],
                Vec::new(),
                DependencyCut(ApplySequence(1)),
                origin.clone(),
                None,
            )
            .expect("the direct dependency event plans")
            .expect("one output produces one event");
        let later = DependencyKey::Cell(ckb_types::packed::OutPoint::new(hash, 1));
        let owner = RawTxHash(ckb_types::packed::Byte32::new([0xe7; 32]));
        let shard = dependency_origin_shard(&frontier.entries, &origin);
        frontier.entries.layout.shards[shard]
            .write()
            .dependency_relations
            .entry(origin)
            .or_default()
            .stable_insert(later, DependencyRelationTarget::OtherConsumer, owner);
        matches!(
            Self::empty_batch_for_foundation(control.into()).seal_prestate(&frontier),
            Err(DependencyError::Stale)
        )
    }

    pub(in crate::authority) fn actual_order_activation_for_foundation() -> (bool, bool, bool, bool)
    {
        let entries = ShardedOwnerMap::new(AuthorityShardRouter::new());
        let frontier = Self::for_entries(&entries, usize::MAX);
        let old_key = DependencyKey::Cell(ckb_types::packed::OutPoint::new(
            ckb_types::packed::Byte32::new([0xe3; 32]),
            0,
        ));
        let new_key = DependencyKey::Cell(ckb_types::packed::OutPoint::new(
            ckb_types::packed::Byte32::new([0xe4; 32]),
            0,
        ));
        for (key, owner) in [
            (
                old_key.clone(),
                RawTxHash(ckb_types::packed::Byte32::new([0xe5; 32])),
            ),
            (
                new_key.clone(),
                RawTxHash(ckb_types::packed::Byte32::new([0xe6; 32])),
            ),
        ] {
            let origin = key.origin();
            let shard = dependency_origin_shard(&frontier.entries, &origin);
            frontier.entries.layout.shards[shard]
                .write()
                .dependency_relations
                .entry(origin)
                .or_default()
                .stable_insert(key, DependencyRelationTarget::OtherConsumer, owner);
        }
        let old_level = DependencyLevel {
            last_change: DependencyCut(ApplySequence(1)),
            last_definitive_loss: None,
        };
        frontier
            .replace_level(old_key.clone(), old_level)
            .expect("the fixture level row is stable");
        let old_dirty = DirtyDependency {
            target: old_level.last_change,
            scope: DirtyScope::ExistingWaiters,
            cursor: None,
            pending: None,
        };
        frontier
            .dirty_insert(old_key.clone(), old_dirty.clone())
            .expect("the fixture dirty row is stable");
        let before = frontier.maintenance_pending();
        let complete = frontier
            .seal_shared_maintenance(DependencyMaintenancePlan {
                step: DependencyMaintenanceStep::Complete {
                    key: old_key,
                    expected: old_dirty,
                },
            })
            .expect("the old final dirty row seals");
        let _finalization = StagedDependencyBatch::stage_primary_replacements(&frontier, complete)
            .expect("the old final dirty row stages")
            .publish_exclusive();
        let after_complete = frontier.maintenance_pending();

        let event = frontier
            .plan_events(Vec::new(), vec![new_key], DependencyCut(ApplySequence(2)))
            .expect("the disjoint activation plans")
            .expect("the disjoint consumer creates one event");
        let event = Self::empty_batch_for_foundation(event.into())
            .seal_prestate(&frontier)
            .expect("the disjoint activation prestate seals");
        let activated = matches!(
            StagedDependencyBatch::stage_primary_replacements(&frontier, event,)
                .expect("the disjoint activation stages")
                .publish_exclusive(),
            DependencyFinalization::Activated
        );
        (
            before,
            after_complete,
            frontier.maintenance_pending(),
            activated,
        )
    }

    pub(in crate::authority) fn apply_control_in_exact_cut_for_reference(
        &self,
        control: DependencyEntryControlDelta,
    ) {
        let delta = Self::empty_batch_for_foundation(control.into())
            .seal_prestate(self)
            .expect("the reference control prestate seals");
        let _finalization = StagedDependencyBatch::stage_primary_replacements(self, delta)
            .expect("the reference control rows stage")
            .publish_exclusive();
    }

    /// Build only the immutable level cut consumed by currentness methods.
    /// Producer/owner reachability is covered by the real authority lifecycle
    /// tests; this adapter does not exercise mutation or maintenance.
    pub(in crate::authority) fn from_evidence_cut_for_foundation(
        levels: impl IntoIterator<Item = DependencyEvidenceLevelInput>,
        unindexed: UnindexedDependencyLevelInput,
    ) -> Option<Self> {
        if unindexed
            .last_definitive_loss
            .is_some_and(|loss| unindexed.last_change.is_none_or(|change| loss > change))
        {
            return None;
        }
        let mut collected = BTreeMap::new();
        for input in levels {
            if input
                .last_definitive_loss
                .is_some_and(|loss| loss > input.last_change)
                || collected
                    .insert(
                        input.key,
                        DependencyLevel {
                            last_change: input.last_change,
                            last_definitive_loss: input.last_definitive_loss,
                        },
                    )
                    .is_some()
            {
                return None;
            }
        }
        let entries = crate::authority::shard::ShardedOwnerMap::new(
            crate::authority::shard::AuthorityShardRouter::new(),
        );
        let frontier = Self::for_entries(&entries, usize::MAX);
        for (key, level) in collected {
            frontier
                .replace_level(key, level)
                .expect("the fixture level row is stable");
        }
        for shard in &frontier.entries.layout.shards[..] {
            shard.write().dependency_unindexed = UnindexedDependencyLevel {
                last_change: unindexed.last_change,
                last_definitive_loss: unindexed.last_definitive_loss,
            };
        }
        Some(frontier)
    }

    pub(in crate::authority) fn unindexed_definitive_loss_for_reference(
        &self,
        key: &DependencyKey,
    ) -> Option<DependencyCut> {
        self.unindexed_level(key).last_definitive_loss
    }

    pub(in crate::authority) fn snapshot(&self) -> DependencySnapshot {
        let shards: [ckb_util::parking_lot::RwLockReadGuard<
            '_,
            crate::authority::shard::AuthorityShard,
        >; crate::authority::shard::AUTHORITY_SHARD_COUNT] =
            std::array::from_fn(|shard| self.entries.layout.shards[shard].read());
        let mut consumers = BTreeMap::new();
        let mut waiters = BTreeMap::new();
        let mut keys_by_origin = BTreeMap::new();
        let mut levels = BTreeMap::new();
        let mut dirty = BTreeMap::new();
        let mut unindexed = UnindexedDependencyLevel::default();
        for shard in &shards {
            for (origin, origin_row) in &shard.dependency_relations {
                let mut origin_keys = BTreeSet::new();
                for (key, relation) in &origin_row.keys {
                    let visible: BTreeSet<RawTxHash> =
                        relation.consumers.iter_visible().cloned().collect();
                    if !visible.is_empty() {
                        consumers.insert(key.clone(), visible);
                        origin_keys.insert(key.clone());
                    }
                    let visible_waiters: BTreeSet<RawTxHash> =
                        relation.waiters.iter_visible().cloned().collect();
                    if !visible_waiters.is_empty() {
                        waiters.insert(key.clone(), visible_waiters);
                    }
                }
                if !origin_keys.is_empty() {
                    keys_by_origin.insert(origin.clone(), origin_keys);
                }
            }
            levels.extend(shard.dependency_levels.iter().filter_map(|(key, cell)| {
                cell.logical().copied().map(|level| (key.clone(), level))
            }));
            dirty.extend(
                shard.dependency_dirty.iter().filter_map(|(key, cell)| {
                    cell.logical_cloned().map(|dirty| (key.clone(), dirty))
                }),
            );
            unindexed.last_change = match (
                unindexed.last_change,
                shard.dependency_unindexed.last_change,
            ) {
                (Some(left), Some(right)) => Some(left.max(right)),
                (left, right) => left.or(right),
            };
            unindexed.last_definitive_loss = match (
                unindexed.last_definitive_loss,
                shard.dependency_unindexed.last_definitive_loss,
            ) {
                (Some(left), Some(right)) => Some(left.max(right)),
                (left, right) => left.or(right),
            };
        }
        DependencySnapshot {
            consumers,
            waiters,
            keys_by_origin,
            levels,
            dirty,
            dirty_cursor: self.maintenance.cursor.lock().clone(),
            unindexed,
        }
    }

    pub(in crate::authority) fn next_maintenance_observation(
        &self,
    ) -> Result<Option<(DependencyKey, Option<RawTxHash>)>, DependencyError> {
        Ok(self
            .next_maintenance()?
            .map(|ticket| (ticket.key, ticket.hash)))
    }

    /// Exact edge/marker count for a fixed projection and a finite upper bound
    /// when one owner requeue also retires waiter edges from pending epochs.
    /// This is a test-only ghost projection and never runs under a production
    /// authority guard.
    pub(in crate::authority) fn maintenance_rank(
        &self,
    ) -> Result<DependencyMaintenanceRank, DependencyMaintenanceRankError> {
        let edges = |key: &DependencyKey, scope: DirtyScope| match scope {
            DirtyScope::ExistingWaiters => self.waiters(key),
            DirtyScope::AllConsumers => self.consumers(key),
        };
        let snapshot = self.snapshot();
        let rank = snapshot
            .dirty
            .iter()
            .try_fold(0usize, |total, (key, dirty)| {
                let remaining = edges(key, dirty.scope)
                    .map_err(|_| DependencyMaintenanceRankError::Arithmetic)?
                    .as_ref()
                    .map_or(0, |owners| {
                        owners
                            .iter()
                            .filter(|owner| {
                                dirty.cursor.as_ref().is_none_or(|cursor| *owner > cursor)
                            })
                            .count()
                    });
                let current = remaining
                    .checked_add(1)
                    .ok_or(DependencyMaintenanceRankError::Arithmetic)?;
                let pending = match dirty.pending {
                    Some(pending) => edges(key, pending.scope)
                        .map_err(|_| DependencyMaintenanceRankError::Arithmetic)?
                        .as_ref()
                        .map_or(0, BTreeSet::len)
                        .checked_add(1)
                        .ok_or(DependencyMaintenanceRankError::Arithmetic)?,
                    None => 0,
                };
                total
                    .checked_add(current)
                    .and_then(|rank| rank.checked_add(pending))
                    .ok_or(DependencyMaintenanceRankError::Arithmetic)
            })?;
        Ok(DependencyMaintenanceRank(rank))
    }

    pub(in crate::authority) fn semantically_matches(
        &self,
        entries: &crate::authority::shard::ShardedOwnerMap,
    ) -> bool {
        let expected_entries = crate::authority::shard::ShardedOwnerMap::new(
            crate::authority::shard::AuthorityShardRouter::new(),
        );
        let expected = Self::for_entries(&expected_entries, usize::MAX);
        let snapshot = entries.snapshot_for_test();
        for (_, owner) in &snapshot {
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
        let current = self.snapshot();
        let expected = expected.snapshot();
        current.consumers == expected.consumers
            && current.waiters == expected.waiters
            && current.keys_by_origin == expected.keys_by_origin
            && current
                .levels
                .keys()
                .all(|key| current.consumers.contains_key(key))
            && current.dirty.iter().all(|(key, dirty)| {
                current.consumers.contains_key(key)
                    && current.levels.get(key).is_some_and(|level| {
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
            && current.unindexed.last_definitive_loss.is_none_or(|loss| {
                current
                    .unindexed
                    .last_change
                    .is_some_and(|change| loss <= change)
            })
            && current.waiters.iter().all(|(key, waiters)| {
                current
                    .consumers
                    .get(key)
                    .is_some_and(|consumers| waiters.is_subset(consumers))
            })
    }

    // ---- round0016 Partner-R deterministic race-matrix canaries ----
    // Every predicate below pins one frozen race-matrix cell on a bare
    // frontier with no threads, so each interleaving is exact and each
    // failure is deterministic. The named tests in tests/dependency.rs
    // assert them; each claim is restated at its assertion site.

    fn foundation_dependency_keys(keys: &[DependencyKey]) -> KnownDependencies {
        use ckb_types::prelude::{Builder, Entity};
        let mut tx = ckb_types::core::TransactionBuilder::default();
        for key in keys {
            match key {
                DependencyKey::Cell(out_point) => {
                    tx = tx.cell_dep(
                        ckb_types::packed::CellDep::new_builder()
                            .out_point(out_point.clone())
                            .build(),
                    );
                }
                DependencyKey::Header(hash) => {
                    tx = tx.header_dep(hash.clone());
                }
            }
        }
        KnownDependencies::from_transaction(&tx.build())
            .expect("foundation dependency keys canonicalize")
    }

    fn foundation_slot(
        discriminator: u8,
        phase: DependencyConsumerPhase,
        dependencies: &[DependencyKey],
        waiting: Option<ObservedDependencies>,
    ) -> DependencySlot {
        DependencySlot {
            hash: RawTxHash(ckb_types::packed::Byte32::new([discriminator; 32])),
            phase,
            dependencies: Self::foundation_dependency_keys(dependencies),
            waiting,
        }
    }

    fn foundation_key(discriminator: u8) -> DependencyKey {
        DependencyKey::Cell(ckb_types::packed::OutPoint::new(
            ckb_types::packed::Byte32::new([discriminator; 32]),
            0,
        ))
    }

    fn added_only_stage(
        frontier: &DependencyFrontier,
        slot: DependencySlot,
    ) -> Result<StagedDependencyBatch, DependencyStageError> {
        let delta = DependencyBatchDelta {
            removed: Vec::new(),
            added: vec![slot],
            observed: Vec::new(),
            unchanged: Vec::new(),
            relation_changes: Vec::new(),
            settlement_evidence: Vec::new(),
            control: DependencyControlDelta::None,
            prestate: DependencyBatchPrestate::default(),
        }
        .seal_prestate(frontier)
        .expect("the added-only foundation delta seals");
        StagedDependencyBatch::stage_primary_replacements(frontier, delta)
    }

    fn removed_only_stage(
        frontier: &DependencyFrontier,
        slot: DependencySlot,
    ) -> Result<StagedDependencyBatch, DependencyStageError> {
        let delta = DependencyBatchDelta {
            removed: vec![slot],
            added: Vec::new(),
            observed: Vec::new(),
            unchanged: Vec::new(),
            relation_changes: Vec::new(),
            settlement_evidence: Vec::new(),
            control: DependencyControlDelta::None,
            prestate: DependencyBatchPrestate::default(),
        }
        .seal_prestate(frontier)
        .expect("the removed-only foundation delta seals");
        StagedDependencyBatch::stage_primary_replacements(frontier, delta)
    }

    fn visible_consumers(
        frontier: &DependencyFrontier,
        key: &DependencyKey,
        owners: &[u8],
    ) -> bool {
        let expected: BTreeSet<RawTxHash> = owners
            .iter()
            .map(|owner| RawTxHash(ckb_types::packed::Byte32::new([*owner; 32])))
            .collect();
        frontier
            .consumers_for(key)
            .is_ok_and(|visible| visible.unwrap_or_default() == expected)
    }

    /// C1 (batch level): a hidden staged relation cell rejects every
    /// successor batch touching the same owner cell — insert-over-insert and
    /// retire-over-staged-insert alike — until the owning batch reaches its
    /// exact cleanup, after which the successor stages and both terminals
    /// (publish and rollback) leave the logical view exact.
    pub(in crate::authority) fn batch_stage_excludes_same_cell_successor_until_cleanup_for_foundation()
    -> bool {
        let entries = ShardedOwnerMap::new(AuthorityShardRouter::new());
        let frontier = Self::for_entries(&entries, usize::MAX);
        let key = Self::foundation_key(0x11);
        let slot_a = || {
            Self::foundation_slot(
                0xaa,
                DependencyConsumerPhase::Other,
                std::slice::from_ref(&key),
                None,
            )
        };
        let Ok(first) = Self::added_only_stage(&frontier, slot_a()) else {
            return false;
        };
        if !Self::visible_consumers(&frontier, &key, &[]) {
            return false;
        }
        // Hidden: same-cell insert successor rejected.
        if Self::added_only_stage(&frontier, slot_a()).is_ok() {
            return false;
        }
        // Hidden: same-cell retire successor rejected (staged is not stable).
        if Self::removed_only_stage(&frontier, slot_a()).is_ok() {
            return false;
        }
        if !matches!(first.publish_exclusive(), DependencyFinalization::Quiet)
            || !Self::visible_consumers(&frontier, &key, &[0xaa])
        {
            return false;
        }
        // Exact publish cleanup re-admits the same-cell successor.
        let Ok(second) = Self::removed_only_stage(&frontier, slot_a()) else {
            return false;
        };
        if !Self::visible_consumers(&frontier, &key, &[0xaa]) {
            return false;
        }
        // Rollback cleanup is exact and re-admits again.
        drop(second);
        if !Self::visible_consumers(&frontier, &key, &[0xaa]) {
            return false;
        }
        let Ok(third) = Self::removed_only_stage(&frontier, slot_a()) else {
            return false;
        };
        if !matches!(third.publish_exclusive(), DependencyFinalization::Quiet) {
            return false;
        }
        Self::visible_consumers(&frontier, &key, &[])
            && frontier.snapshot().levels.is_empty()
            && !frontier.maintenance_pending()
    }

    /// C2 + reverse completion: disjoint owner entries may overlap on one
    /// key — across targets and across actions — while each staged cell stays
    /// exclusive per owner. Publication order between the two batches is
    /// interchangeable and cleanup conserves every relation exactly.
    pub(in crate::authority) fn disjoint_owner_overlap_and_reverse_completion_for_foundation()
    -> bool {
        let entries = ShardedOwnerMap::new(AuthorityShardRouter::new());
        let frontier = Self::for_entries(&entries, usize::MAX);
        let key = Self::foundation_key(0x12);
        let slot_a = Self::foundation_slot(
            0xaa,
            DependencyConsumerPhase::Other,
            std::slice::from_ref(&key),
            None,
        );
        // Published history: stable consumer A on the key.
        let Ok(history) = Self::added_only_stage(&frontier, slot_a.clone()) else {
            return false;
        };
        if !matches!(history.publish_exclusive(), DependencyFinalization::Quiet)
            || !Self::visible_consumers(&frontier, &key, &[0xaa])
        {
            return false;
        }
        // Batch W: one owner stages a consumer insert and a waiter insert on
        // the same key (waiting owners always consume their waiting keys).
        let observed = frontier.observe_missing(
            &MissingDependencies::new(vec![key.clone()], 1024).expect("one missing key is bounded"),
            KnownDependencies::default(),
            DependencyCut(ApplySequence(1)),
        );
        let slot_w = Self::foundation_slot(
            0xcc,
            DependencyConsumerPhase::Other,
            std::slice::from_ref(&key),
            Some(observed),
        );
        let Ok(batch_w) = Self::added_only_stage(&frontier, slot_w) else {
            return false;
        };
        // Batch R: a second batch retires the stable owner A on the same key
        // while W stays hidden — disjoint owner, same consumer set.
        let Ok(batch_r) = Self::removed_only_stage(&frontier, slot_a.clone()) else {
            return false;
        };
        // Both hidden: the logical view is exactly the published history.
        if !Self::visible_consumers(&frontier, &key, &[0xaa])
            || frontier.has_waiter_outside(&key, &[]) != Ok(false)
        {
            return false;
        }
        // Reverse completion: publish the retire before the insert.
        if !matches!(batch_r.publish_exclusive(), DependencyFinalization::Quiet)
            || !Self::visible_consumers(&frontier, &key, &[])
        {
            return false;
        }
        // The hidden insert survived the reverse order and finalizes exactly.
        if !matches!(batch_w.publish_exclusive(), DependencyFinalization::Quiet)
            || !Self::visible_consumers(&frontier, &key, &[0xcc])
            || frontier.has_waiter_outside(&key, &[]) != Ok(true)
        {
            return false;
        }
        // Same-owner restage over a visible cell still requires exact
        // succession: a second retire of the now-absent A is stale.
        Self::removed_only_stage(&frontier, slot_a).is_err()
    }

    /// C3: an event batch compiled with a negative fanout fact (no consumers
    /// for an orphan key) carries that fact to the final cut; late consumer
    /// growth before publication flips the apply-time OCC to stale, and the
    /// rollback after the lost race leaves no residue.
    pub(in crate::authority) fn event_negative_fanout_late_growth_is_stale_at_apply_for_foundation()
    -> bool {
        let entries = ShardedOwnerMap::new(AuthorityShardRouter::new());
        let frontier = Self::for_entries(&entries, usize::MAX);
        let key = Self::foundation_key(0x13);
        let control = frontier
            .plan_events(
                vec![key.clone()],
                Vec::new(),
                DependencyCut(ApplySequence(1)),
            )
            .expect("the orphan availability event plans")
            .expect("one orphan key produces one event");
        let delta = Self::empty_batch_for_foundation(control.into())
            .seal_prestate(&frontier)
            .expect("the orphan event prestate seals");
        let Ok(batch) = StagedDependencyBatch::stage_primary_replacements(&frontier, delta) else {
            return false;
        };
        // The negative fanout fact was compiled and is fresh at stage time.
        if batch.fanout_absence_observation_for_foundation() != (1, true) {
            return false;
        }
        // Late consumer growth interposes before the final cut.
        let origin = key.origin();
        let owner = RawTxHash(ckb_types::packed::Byte32::new([0xee; 32]));
        let shard = dependency_origin_shard(&frontier.entries, &origin);
        frontier.entries.layout.shards[shard]
            .write()
            .dependency_relations
            .entry(origin)
            .or_default()
            .stable_insert(key.clone(), DependencyRelationTarget::OtherConsumer, owner);
        if batch.fanout_absence_observation_for_foundation() != (1, false) {
            return false;
        }
        // The full apply-time OCC agrees: this batch can no longer commit.
        let mut reads = ShardReadSupport::default();
        batch.extend_final_read_support(&mut reads);
        let cut = frontier
            .entries
            .mixed_cut(reads, ShardWriteSupport::default());
        if batch.prestate_is_fresh(&cut) {
            return false;
        }
        drop(cut);
        // Rollback after the lost race is exact: no level, no dirty, no
        // unindexed fold, and the interleaved consumer stays untouched.
        drop(batch);
        let snapshot = frontier.snapshot();
        snapshot.levels.is_empty()
            && snapshot.dirty.is_empty()
            && snapshot.unindexed.last_change.is_none()
            && Self::visible_consumers(&frontier, &key, &[0xee])
    }

    /// C6: the stage bank returns its units exactly once per batch on both
    /// terminals — rollback and publish — including units grown for staged
    /// control rows; saturation during the stage is exact, and no terminal
    /// over-returns.
    pub(in crate::authority) fn stage_capacity_returns_exactly_once_for_foundation() -> bool {
        // (a) relation-only batch: 2 units.
        let entries = ShardedOwnerMap::new(AuthorityShardRouter::new());
        let frontier = Self::for_entries(&entries, 2);
        let slot_a = || {
            let keys = [Self::foundation_key(0x21), Self::foundation_key(0x22)];
            Self::foundation_slot(0xaa, DependencyConsumerPhase::Other, &keys, None)
        };
        let Ok(batch) = Self::added_only_stage(&frontier, slot_a()) else {
            return false;
        };
        if frontier.stage_bank.try_acquire(1).is_ok() {
            return false;
        }
        drop(batch);
        let Ok(returned) = frontier.stage_bank.try_acquire(2) else {
            return false;
        };
        drop(returned);
        let Ok(batch) = Self::added_only_stage(&frontier, slot_a()) else {
            return false;
        };
        if !matches!(batch.publish_exclusive(), DependencyFinalization::Quiet) {
            return false;
        }
        // Exactly-once return after publish: capacity is 2, not more.
        if frontier.stage_bank.try_acquire(3).is_ok() {
            return false;
        }
        let Ok(returned) = frontier.stage_bank.try_acquire(2) else {
            return false;
        };
        drop(returned);

        // (b) control batch: 0 relations acquire 1 unit, staged level+dirty
        // grow the permit by 2; publish returns all 3 exactly once.
        let entries = ShardedOwnerMap::new(AuthorityShardRouter::new());
        let frontier = Self::for_entries(&entries, 3);
        let key = Self::foundation_key(0x23);
        let consumer = Self::foundation_slot(
            0xbb,
            DependencyConsumerPhase::Other,
            std::slice::from_ref(&key),
            None,
        );
        frontier.attach(&consumer);
        let level = DependencyLevel {
            last_change: DependencyCut(ApplySequence(1)),
            last_definitive_loss: None,
        };
        if frontier.replace_level(key.clone(), level).is_err() {
            return false;
        }
        let dirty = DirtyDependency {
            target: DependencyCut(ApplySequence(1)),
            scope: DirtyScope::ExistingWaiters,
            cursor: None,
            pending: None,
        };
        if frontier.dirty_insert(key.clone(), dirty).is_err() {
            return false;
        }
        let control = frontier
            .plan_events(
                vec![key.clone()],
                Vec::new(),
                DependencyCut(ApplySequence(2)),
            )
            .expect("the consumer availability event plans")
            .expect("one key produces one event");
        let delta = Self::empty_batch_for_foundation(control.into())
            .seal_prestate(&frontier)
            .expect("the consumer event prestate seals");
        let Ok(batch) = StagedDependencyBatch::stage_primary_replacements(&frontier, delta) else {
            return false;
        };
        if batch.staged_levels.len() != 1 || batch.staged_dirty.len() != 1 {
            return false;
        }
        if frontier.stage_bank.try_acquire(1).is_ok() {
            return false;
        }
        if matches!(batch.publish_exclusive(), DependencyFinalization::Poisoned) {
            return false;
        }
        if frontier.stage_bank.try_acquire(4).is_ok() {
            return false;
        }
        let Ok(returned) = frontier.stage_bank.try_acquire(3) else {
            return false;
        };
        drop(returned);
        // The published event landed exactly: level advanced, dirty pending
        // merged for the consumer, no physical residue.
        let snapshot = frontier.snapshot();
        snapshot.levels.get(&key).is_some_and(|level| {
            *level
                == DependencyLevel {
                    last_change: DependencyCut(ApplySequence(2)),
                    last_definitive_loss: None,
                }
        })
    }

    /// C7: a staged batch whose generation payload was swapped out (the
    /// clear-pool/reorg replacement shape) cannot splice into the live
    /// generation: finalization is fail-stop poisoned on the retired
    /// maintenance state, the live payload receives no writes, and the
    /// retired payload keeps its hidden staged cells until it is dropped.
    pub(in crate::authority) fn generation_swap_strands_old_batch_without_splice_for_foundation()
    -> bool {
        let entries = ShardedOwnerMap::new(AuthorityShardRouter::new());
        let frontier = Self::for_entries(&entries, usize::MAX);
        let key = Self::foundation_key(0x14);
        let slot_a = Self::foundation_slot(
            0xaa,
            DependencyConsumerPhase::Other,
            std::slice::from_ref(&key),
            None,
        );
        let Ok(batch) = Self::added_only_stage(&frontier, slot_a.clone()) else {
            return false;
        };
        // Generation replacement: the live map swaps payloads with the
        // retired carrier, and a fresh generation binds to the live map.
        let carrier = ShardedOwnerMap::new(AuthorityShardRouter::new());
        entries.swap_generation_payload_with(&carrier);
        let successor = Self::for_entries(&entries, usize::MAX);
        let finalization = batch.finalize();
        if !matches!(finalization, DependencyFinalization::Poisoned)
            || !frontier.maintenance.is_poisoned()
        {
            return false;
        }
        // No splice: the live payload carries no dependency rows at all.
        let live_clean = entries.layout.shards.iter().all(|shard| {
            let shard = shard.read();
            shard.dependency_relations.is_empty()
                && shard.dependency_levels.is_empty()
                && shard.dependency_dirty.is_empty()
                && shard.dependency_dirty_staged == 0
        });
        if !live_clean {
            return false;
        }
        // The retired carrier still owns the hidden staged cell.
        let origin = key.origin();
        let retired = &carrier.layout.shards[dependency_origin_shard(&carrier, &origin)];
        let retired = retired.read();
        let cell_retained = retired
            .dependency_relations
            .get(&origin)
            .and_then(|row| row.key(&key))
            .is_some_and(|row| {
                row.consumers
                    .members(DependencyConsumerPhase::Other)
                    .contains_physical(&RawTxHash(ckb_types::packed::Byte32::new([0xaa; 32])))
            });
        drop(retired);
        if !cell_retained {
            return false;
        }
        // The successor generation is fully functional on the live map.
        let Ok(fresh) = Self::added_only_stage(&successor, slot_a) else {
            return false;
        };
        matches!(fresh.publish_exclusive(), DependencyFinalization::Quiet)
            && Self::visible_consumers(&successor, &key, &[0xaa])
            && !successor.maintenance.is_poisoned()
    }

    /// C8: the sealed retained fast path cannot be constructed for any shape
    /// carrying Event, Maintenance, settlement, Waiting, Retire or Accepted
    /// semantics; every rejected seal rolls its staged rows back exactly.
    pub(in crate::authority) fn sealed_retained_rejects_every_non_eligible_shape_for_foundation()
    -> bool {
        // (a) Event control with staged level/dirty rows.
        let entries = ShardedOwnerMap::new(AuthorityShardRouter::new());
        let frontier = Self::for_entries(&entries, usize::MAX);
        let key = Self::foundation_key(0x15);
        let consumer = Self::foundation_slot(
            0xbb,
            DependencyConsumerPhase::Other,
            std::slice::from_ref(&key),
            None,
        );
        frontier.attach(&consumer);
        let control = frontier
            .plan_events(
                vec![key.clone()],
                Vec::new(),
                DependencyCut(ApplySequence(1)),
            )
            .expect("the consumer event plans")
            .expect("one key produces one event");
        let delta = Self::empty_batch_for_foundation(control.into())
            .seal_prestate(&frontier)
            .expect("the consumer event prestate seals");
        let Ok(batch) = StagedDependencyBatch::stage_primary_replacements(&frontier, delta) else {
            return false;
        };
        if batch.seal_scheduler_retained().is_ok() {
            return false;
        }
        // The rejected seal dropped the batch: rollback restored the view.
        if !Self::visible_consumers(&frontier, &key, &[0xbb])
            || !frontier.snapshot().levels.is_empty()
        {
            return false;
        }

        // (b) Maintenance control with a staged dirty row.
        let dirty = DirtyDependency {
            target: DependencyCut(ApplySequence(1)),
            scope: DirtyScope::ExistingWaiters,
            cursor: None,
            pending: None,
        };
        if frontier.dirty_insert(key.clone(), dirty.clone()).is_err() {
            return false;
        }
        let delta = frontier
            .seal_shared_maintenance(DependencyMaintenancePlan {
                step: DependencyMaintenanceStep::Complete {
                    key: key.clone(),
                    expected: dirty.clone(),
                },
            })
            .expect("the maintenance completion seals");
        let Ok(batch) = StagedDependencyBatch::stage_primary_replacements(&frontier, delta) else {
            return false;
        };
        if batch.seal_scheduler_retained().is_ok() {
            return false;
        }
        if frontier.snapshot().dirty.get(&key) != Some(&dirty) {
            return false;
        }

        // (c) Waiting shape: a slot carrying observed missing dependencies.
        let key = Self::foundation_key(0x16);
        let observed = frontier.observe_missing(
            &MissingDependencies::new(vec![key.clone()], 1024).expect("one missing key is bounded"),
            KnownDependencies::default(),
            DependencyCut(ApplySequence(1)),
        );
        let waiting = Self::foundation_slot(
            0xcc,
            DependencyConsumerPhase::Other,
            std::slice::from_ref(&key),
            Some(observed),
        );
        let Ok(batch) = Self::added_only_stage(&frontier, waiting) else {
            return false;
        };
        if batch.seal_scheduler_retained().is_ok() {
            return false;
        }
        if !Self::visible_consumers(&frontier, &key, &[])
            || frontier.has_waiter_outside(&key, &[]) != Ok(false)
        {
            return false;
        }

        // (d) Accepted consumer phase.
        let key = Self::foundation_key(0x17);
        let accepted = Self::foundation_slot(
            0xdd,
            DependencyConsumerPhase::Accepted,
            std::slice::from_ref(&key),
            None,
        );
        let Ok(batch) = Self::added_only_stage(&frontier, accepted) else {
            return false;
        };
        if batch.seal_scheduler_retained().is_ok() {
            return false;
        }
        if !Self::visible_consumers(&frontier, &key, &[]) {
            return false;
        }

        // (e) Retire shape over a published stable row.
        let key = Self::foundation_key(0x18);
        let slot_e = Self::foundation_slot(
            0xee,
            DependencyConsumerPhase::Other,
            std::slice::from_ref(&key),
            None,
        );
        let Ok(history) = Self::added_only_stage(&frontier, slot_e.clone()) else {
            return false;
        };
        if !matches!(history.publish_exclusive(), DependencyFinalization::Quiet) {
            return false;
        }
        let Ok(batch) = Self::removed_only_stage(&frontier, slot_e) else {
            return false;
        };
        if batch.seal_scheduler_retained().is_ok() {
            return false;
        }
        if !Self::visible_consumers(&frontier, &key, &[0xee]) {
            return false;
        }

        // (f) Settlement evidence: the shape predicate itself refuses any
        // batch carrying per-owner endpoint evidence.
        let mut delta = DependencyBatchDelta {
            removed: Vec::new(),
            added: vec![Self::foundation_slot(
                0xf1,
                DependencyConsumerPhase::Other,
                &[Self::foundation_key(0x19)],
                None,
            )],
            observed: Vec::new(),
            unchanged: Vec::new(),
            relation_changes: Vec::new(),
            settlement_evidence: vec![SettlementDependencyEvidence {
                owner: RawTxHash(ckb_types::packed::Byte32::new([0xf1; 32])),
                keys: Vec::new(),
            }],
            control: DependencyControlDelta::None,
            prestate: DependencyBatchPrestate::default(),
        };
        if delta.is_scheduler_sealed_retained_shape() {
            return false;
        }
        // Positive control: the pure-Other retained shape still seals.
        delta.settlement_evidence.clear();
        if !delta.is_scheduler_sealed_retained_shape() {
            return false;
        }
        let key = Self::foundation_key(0x1a);
        let pure = Self::foundation_slot(
            0xf2,
            DependencyConsumerPhase::Other,
            std::slice::from_ref(&key),
            None,
        );
        let delta = DependencyBatchDelta {
            removed: Vec::new(),
            added: vec![pure],
            observed: Vec::new(),
            unchanged: Vec::new(),
            relation_changes: Vec::new(),
            settlement_evidence: Vec::new(),
            control: DependencyControlDelta::None,
            prestate: DependencyBatchPrestate::default(),
        }
        .seal_prestate(&frontier)
        .expect("the pure retained shape seals its prestate");
        let Ok(batch) = StagedDependencyBatch::stage_primary_replacements_with_visibility(
            &frontier,
            delta,
            StagedIngressVisibility::hidden(),
        ) else {
            return false;
        };
        let Ok(sealed) = batch.seal_scheduler_retained() else {
            return false;
        };
        drop(sealed);
        Self::visible_consumers(&frontier, &key, &[])
    }

    /// CLEANUP x RETIRE: publishing the last consumer's retire prunes the
    /// orphan key physically — level rows fold into unindexed evidence, dirty
    /// and relation rows disappear, and the key accepts a fresh insert
    /// afterwards with no residue.
    pub(in crate::authority) fn last_consumer_retire_prunes_orphan_rows_exactly_for_foundation()
    -> bool {
        let entries = ShardedOwnerMap::new(AuthorityShardRouter::new());
        let frontier = Self::for_entries(&entries, usize::MAX);
        let key = Self::foundation_key(0x1b);
        let slot_a = Self::foundation_slot(
            0xaa,
            DependencyConsumerPhase::Other,
            std::slice::from_ref(&key),
            None,
        );
        frontier.attach(&slot_a);
        let level = DependencyLevel {
            last_change: DependencyCut(ApplySequence(3)),
            last_definitive_loss: None,
        };
        if frontier.replace_level(key.clone(), level).is_err() {
            return false;
        }
        let dirty = DirtyDependency {
            target: DependencyCut(ApplySequence(3)),
            scope: DirtyScope::ExistingWaiters,
            cursor: None,
            pending: None,
        };
        if frontier.dirty_insert(key.clone(), dirty).is_err() {
            return false;
        }
        let Ok(batch) = Self::removed_only_stage(&frontier, slot_a.clone()) else {
            return false;
        };
        if !matches!(batch.publish_exclusive(), DependencyFinalization::Quiet) {
            return false;
        }
        // Physical — not merely logical — removal of every orphan row.
        let origin = key.origin();
        let relation_shard = dependency_origin_shard(&frontier.entries, &origin);
        let level_shard = frontier
            .entries
            .layout
            .router
            .shard(b"dependency/level", &key);
        let unindexed_shard = frontier
            .entries
            .layout
            .router
            .shard(b"dependency/unindexed", &key);
        let relation_rows_gone = frontier.entries.layout.shards[relation_shard]
            .read()
            .dependency_relations
            .is_empty();
        let level_row_gone = frontier.entries.layout.shards[level_shard]
            .read()
            .dependency_levels
            .is_empty();
        let dirty_row_gone = frontier.entries.layout.shards[level_shard]
            .read()
            .dependency_dirty
            .is_empty();
        let unindexed_folded = frontier.entries.layout.shards[unindexed_shard]
            .read()
            .dependency_unindexed
            .last_change
            == Some(DependencyCut(ApplySequence(3)));
        if !(relation_rows_gone && level_row_gone && dirty_row_gone && unindexed_folded) {
            return false;
        }
        if frontier.maintenance_pending() || !Self::visible_consumers(&frontier, &key, &[]) {
            return false;
        }
        // The pruned key accepts a fresh insert with no residue.
        let Ok(fresh) = Self::added_only_stage(&frontier, slot_a) else {
            return false;
        };
        matches!(fresh.publish_exclusive(), DependencyFinalization::Quiet)
            && Self::visible_consumers(&frontier, &key, &[0xaa])
    }

    /// Rollback-residue documentation: the two rollback routes differ
    /// physically. A dropped hidden batch normalizes through `finish_rows`
    /// and prunes its empty key rows exactly. A stage that fails after its
    /// relations were applied (here: bank capacity exhausted by the control
    /// growth) rolls back through set-level `finish_owned_stage` and leaves a
    /// logically inert empty key-row shell, which the next exact cleanup on
    /// the key removes. This pins the named residual seam — and its bound —
    /// rather than hiding it.
    pub(in crate::authority) fn failed_stage_prunes_physical_scaffold_before_retry_for_foundation()
    -> bool {
        // Phase 1: Drop rollback prunes exactly — no residue at all.
        let entries = ShardedOwnerMap::new(AuthorityShardRouter::new());
        let frontier = Self::for_entries(&entries, usize::MAX);
        let dropped_key = Self::foundation_key(0x1c);
        let dropped_slot = Self::foundation_slot(
            0xaa,
            DependencyConsumerPhase::Other,
            std::slice::from_ref(&dropped_key),
            None,
        );
        let Ok(batch) = Self::added_only_stage(&frontier, dropped_slot) else {
            return false;
        };
        drop(batch);
        let dropped_origin = dropped_key.origin();
        if !frontier.entries.layout.shards
            [dependency_origin_shard(&frontier.entries, &dropped_origin)]
        .read()
        .dependency_relations
        .is_empty()
        {
            return false;
        }

        // Phase 2: one batch stages two relation inserts plus an event control
        // on both keys: 2 relation units + 4 control units exceed bank
        // capacity 3, so the stage fails after both relations were applied.
        let entries = ShardedOwnerMap::new(AuthorityShardRouter::new());
        let frontier = Self::for_entries(&entries, 3);
        let key1 = Self::foundation_key(0x1d);
        let key2 = Self::foundation_key(0x1e);
        let consumer1 = Self::foundation_slot(
            0xb1,
            DependencyConsumerPhase::Other,
            std::slice::from_ref(&key1),
            None,
        );
        let consumer2 = Self::foundation_slot(
            0xb2,
            DependencyConsumerPhase::Other,
            std::slice::from_ref(&key2),
            None,
        );
        frontier.attach(&consumer1);
        frontier.attach(&consumer2);
        let level = DependencyLevel {
            last_change: DependencyCut(ApplySequence(1)),
            last_definitive_loss: None,
        };
        let dirty = DirtyDependency {
            target: DependencyCut(ApplySequence(1)),
            scope: DirtyScope::ExistingWaiters,
            cursor: None,
            pending: None,
        };
        for key in [&key1, &key2] {
            if frontier.replace_level(key.clone(), level).is_err()
                || frontier.dirty_insert(key.clone(), dirty.clone()).is_err()
            {
                return false;
            }
        }
        let control = frontier
            .plan_events(
                vec![key1.clone(), key2.clone()],
                Vec::new(),
                DependencyCut(ApplySequence(2)),
            )
            .expect("the two-key consumer event plans")
            .expect("two keys produce one event");
        let slot_a = Self::foundation_slot(
            0xaa,
            DependencyConsumerPhase::Other,
            &[key1.clone(), key2.clone()],
            None,
        );
        let delta = DependencyBatchDelta {
            removed: Vec::new(),
            added: vec![slot_a.clone()],
            observed: Vec::new(),
            unchanged: Vec::new(),
            relation_changes: Vec::new(),
            settlement_evidence: Vec::new(),
            control: control.into(),
            prestate: DependencyBatchPrestate::default(),
        }
        .seal_prestate(&frontier)
        .expect("the mixed delta seals");
        let staged = StagedDependencyBatch::stage_primary_replacements(&frontier, delta);
        if staged.err() != Some(DependencyStageError::Capacity) {
            return false;
        }
        // The logical view is exactly the pre-stage state on both keys...
        let snapshot = frontier.snapshot();
        if !Self::visible_consumers(&frontier, &key1, &[0xb1])
            || !Self::visible_consumers(&frontier, &key2, &[0xb2])
            || snapshot.levels.get(&key1) != Some(&level)
            || snapshot.levels.get(&key2) != Some(&level)
            || snapshot.dirty.get(&key1) != Some(&dirty)
            || snapshot.dirty.get(&key2) != Some(&dirty)
        {
            return false;
        }
        // ...and exact rollback removes every failed owner cell. These two
        // keys retain their pre-existing consumers, so their live key rows
        // remain without any transitional or staged residue.
        let failed_owner = RawTxHash(ckb_types::packed::Byte32::new([0xaa; 32]));
        for key in [&key1, &key2] {
            let origin = key.origin();
            let exact = frontier.entries.layout.shards
                [dependency_origin_shard(&frontier.entries, &origin)]
            .read()
            .dependency_relations
            .get(&origin)
            .is_some_and(|origin| {
                origin.transitional_len() == 0
                    && origin.key(key).is_some_and(|row| {
                        let set = row.consumers.members(DependencyConsumerPhase::Other);
                        set.staged_len() == 0 && !set.entries.contains_key(&failed_owner)
                    })
            });
            if !exact {
                return false;
            }
        }
        // A clean insert/publish/retire sequence still reuses the same logical
        // facts, and last-consumer retires prune every physical row while
        // folding the levels into unindexed evidence.
        let Ok(fresh) = Self::added_only_stage(&frontier, slot_a.clone()) else {
            return false;
        };
        if !matches!(fresh.publish_exclusive(), DependencyFinalization::Quiet)
            || !Self::visible_consumers(&frontier, &key1, &[0xaa, 0xb1])
            || !Self::visible_consumers(&frontier, &key2, &[0xaa, 0xb2])
        {
            return false;
        }
        let Ok(retire) = Self::removed_only_stage(&frontier, slot_a) else {
            return false;
        };
        if !matches!(retire.publish_exclusive(), DependencyFinalization::Quiet) {
            return false;
        }
        for (key, consumer) in [(&key1, &consumer1), (&key2, &consumer2)] {
            let Ok(retire) = Self::removed_only_stage(&frontier, (*consumer).clone()) else {
                return false;
            };
            if !matches!(retire.publish_exclusive(), DependencyFinalization::Quiet) {
                return false;
            }
            let origin = key.origin();
            let shard = dependency_origin_shard(&frontier.entries, &origin);
            if !frontier.entries.layout.shards[shard]
                .read()
                .dependency_relations
                .is_empty()
            {
                return false;
            }
        }
        let snapshot = frontier.snapshot();
        snapshot.consumers.is_empty()
            && snapshot.levels.is_empty()
            && snapshot.dirty.is_empty()
            && snapshot.keys_by_origin.is_empty()
    }
}
