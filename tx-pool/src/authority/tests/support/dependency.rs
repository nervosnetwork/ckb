use super::*;

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
    levels: BTreeMap<DependencyKey, DependencyLevel>,
    dirty: BTreeMap<DependencyKey, DirtyDependency>,
    dirty_cursor: Option<DependencyKey>,
    unindexed: UnindexedDependencyLevel,
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

    pub(in crate::authority) fn apply_control_in_exact_cut_for_reference(
        &self,
        control: DependencyEntryControlDelta,
    ) {
        let delta = Self::empty_batch_for_foundation(control.into())
            .seal_prestate(self)
            .expect("the reference control prestate seals");
        let _outcome = PreparedDependencyBatch::prepare_primary_replacements(self, delta)
            .expect("the reference control rows prepare")
            .apply_exclusive();
    }

    pub(in crate::authority) fn unindexed_definitive_loss_for_reference(
        &self,
        key: &DependencyKey,
    ) -> Option<DependencyCut> {
        self.unindexed_level(key).last_definitive_loss
    }

    pub(in crate::authority) fn snapshot(&self) -> DependencySnapshot {
        let relations = self.entries.dependency_relations_read_all();
        let shards: [ckb_util::parking_lot::RwLockReadGuard<
            '_,
            crate::authority::shard::AuthorityShard,
        >; crate::authority::shard::AUTHORITY_SHARD_COUNT] =
            std::array::from_fn(|shard| self.entries.layout.shards[shard].read());
        let mut consumers: BTreeMap<DependencyKey, BTreeSet<RawTxHash>> = BTreeMap::new();
        let mut waiters: BTreeMap<DependencyKey, BTreeSet<RawTxHash>> = BTreeMap::new();
        let mut levels = BTreeMap::new();
        let mut dirty = BTreeMap::new();
        let mut unindexed = UnindexedDependencyLevel::default();
        for shard in relations.shards() {
            for (key, relation) in &shard.rows {
                let visible: BTreeSet<RawTxHash> = relation
                    .entries
                    .iter()
                    .filter(|(_, value)| DependencyRelationFilter::Consumers.matches(**value))
                    .map(|(owner, _)| owner.clone())
                    .collect();
                if !visible.is_empty() {
                    consumers.entry(key.clone()).or_default().extend(visible);
                }
                let visible_waiters: BTreeSet<RawTxHash> = relation
                    .entries
                    .iter()
                    .filter(|(_, value)| DependencyRelationFilter::Waiters.matches(**value))
                    .map(|(owner, _)| owner.clone())
                    .collect();
                if !visible_waiters.is_empty() {
                    waiters
                        .entry(key.clone())
                        .or_default()
                        .extend(visible_waiters);
                }
            }
        }
        for shard in &shards {
            levels.extend(
                shard
                    .dependency_levels
                    .iter()
                    .map(|(key, cell)| (key.clone(), *cell)),
            );
            dirty.extend(
                shard
                    .dependency_dirty
                    .iter()
                    .map(|(key, cell)| (key.clone(), cell.clone())),
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
        let relations = self.entries.dependency_relations_read_all();
        if relations.shards().any(|shard| {
            shard
                .rows
                .values()
                .any(|row| !row.accepted_participants_are_exact())
        }) {
            return false;
        }
        drop(relations);
        let expected_entries = crate::authority::shard::ShardedOwnerMap::new(
            crate::authority::shard::AuthorityShardRouter::new(),
        );
        let expected = Self::for_entries(&expected_entries);
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
}
