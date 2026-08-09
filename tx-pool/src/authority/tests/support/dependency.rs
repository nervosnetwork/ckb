use super::*;

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
        Some(Self {
            levels: collected,
            unindexed: UnindexedDependencyLevel {
                last_change: unindexed.last_change,
                last_definitive_loss: unindexed.last_definitive_loss,
            },
            ..Self::default()
        })
    }

    pub(in crate::authority) fn snapshot(&self) -> DependencySnapshot {
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
            DirtyScope::ExistingWaiters => self.waiters.get(key),
            DirtyScope::AllConsumers => self.consumers.get(key),
        };
        let rank = self.dirty.iter().try_fold(0usize, |total, (key, dirty)| {
            let remaining = edges(key, dirty.scope).map_or(0, |owners| {
                owners
                    .iter()
                    .filter(|owner| dirty.cursor.as_ref().is_none_or(|cursor| *owner > cursor))
                    .count()
            });
            let current = remaining
                .checked_add(1)
                .ok_or(DependencyMaintenanceRankError::Arithmetic)?;
            let pending = match dirty.pending {
                Some(pending) => edges(key, pending.scope)
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
