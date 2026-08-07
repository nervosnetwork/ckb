use super::*;

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
