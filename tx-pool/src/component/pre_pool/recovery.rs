use super::*;
use ckb_types::core::TransactionView;

impl PrePoolKernel {
    /// Retain the largest valid parent-first prefix that fits an empty
    /// generation. A prefix is closure-safe because every in-cohort parent
    /// precedes its descendants; once one entry cannot be represented, no
    /// later entry is allowed to cross that missing ownership boundary.
    ///
    /// The scratch kernel has the same exact charge/index rules but owns no
    /// additional transaction backing allocations (`TransactionView` clones
    /// share packed data). Its population and destruction are bounded by the
    /// configured recovery envelope.
    pub(crate) fn retain_recovery_prefix_after_clear(
        &mut self,
        txs: Vec<TransactionView>,
        admitted_epoch: u64,
    ) -> Result<RecoveryBatch, PrePoolError> {
        if !self.entries.is_empty() {
            return Err(PrePoolError::Repair(
                "recovery prefix requires an empty generation",
            ));
        }
        let mut probe = Self::new(self.limits);
        probe.next_version = self.next_version;
        probe.next_arrival = self.next_arrival;
        let mut selected = Vec::new();
        for tx in txs {
            match probe.retain_recovery_batch(vec![tx.clone()], admitted_epoch) {
                Ok(batch) if batch.retained == 1 => selected.push(tx),
                Ok(_) => {}
                // Transaction-shaped identity/shape/capacity failures bound
                // the closure. Structural clock/projection failures still
                // escape to the enclosing DefectDomain.
                Err(error) if error.is_transaction_rejection() => break,
                Err(error) => return Err(error),
            }
        }
        self.retain_recovery_batch(selected, admitted_epoch)
    }

    /// Atomically install one parent-first detached-chain recovery cohort.
    ///
    /// Planning touches only the incoming cohort and its exact existing
    /// owners. Every fallible identity, graph, clock and budget predicate is
    /// checked before the first primary/index mutation. The returned session
    /// is the sole completion barrier; the reorg handler retains no payload.
    pub(crate) fn retain_recovery_batch(
        &mut self,
        txs: Vec<TransactionView>,
        admitted_epoch: u64,
    ) -> Result<RecoveryBatch, PrePoolError> {
        if txs.is_empty() {
            return Ok(RecoveryBatch {
                session: self.next_recovery_session,
                retained: 0,
            });
        }

        let session = self.next_recovery_session;
        let next_session = session
            .checked_add(1)
            .ok_or(PrePoolError::VersionExhausted)?;
        let retained = txs.len();
        let mut hashes = HashSet::with_capacity(retained);
        let mut short_ids = HashMap::with_capacity(retained);
        let mut version_cursor = self.next_version;
        let mut arrival_cursor = self.next_arrival;
        let mut planned = Vec::with_capacity(retained);

        for (ordinal, tx) in txs.into_iter().enumerate() {
            let ordinal = u32::try_from(ordinal).map_err(|_| PrePoolError::VersionExhausted)?;
            let hash = crate::util::compact_packed(&tx.hash());
            if !hashes.insert(hash.clone()) {
                continue;
            }
            let short_id = crate::util::compact_packed(&tx.proposal_short_id());
            if let Some(existing_hash) = short_ids.insert(short_id.clone(), hash.clone())
                && existing_hash != hash
            {
                return Err(PrePoolError::ShortIdCollision {
                    short_id,
                    existing_hash,
                });
            }
            let old = self.entries.get(&hash).cloned();
            let version = version_cursor;
            version_cursor = version_cursor
                .checked_add(1)
                .ok_or(PrePoolError::VersionExhausted)?;
            let arrival = if let Some(old) = &old {
                old.arrival
            } else {
                let arrival = arrival_cursor;
                arrival_cursor = arrival_cursor
                    .checked_add(1)
                    .ok_or(PrePoolError::VersionExhausted)?;
                arrival
            };
            let raw = PipelineRawTx::recovery(tx, admitted_epoch);
            let dependencies = conflict_dependency_keys(&raw.tx, std::iter::empty())
                .into_iter()
                .map(DependencyKey::into_compact)
                .collect();
            let mut next = Entry {
                short_id,
                raw: Arc::new(raw),
                source: PrePoolSource::Recovery,
                recovery: Some(RecoveryMeta { session, ordinal }),
                state: EntryState::RecoveryRetained,
                version,
                arrival,
                expires_at: None,
                payload_charge_bytes: 0,
                charge_bytes: 0,
                dependencies,
            };
            next.payload_charge_bytes = next.raw.charge_bytes();
            next.charge_bytes = self.entry_charge(&next)?;
            self.validate_entry_shape(&hash, &next)?;
            planned.push((hash, old, next));
        }

        let planned_hashes = planned
            .iter()
            .map(|(hash, _, _)| hash.clone())
            .collect::<HashSet<_>>();
        for (hash, _, next) in &planned {
            if let Some(existing_hash) = self.by_short_id.get(&next.short_id)
                && existing_hash != hash
                && !planned_hashes.contains(existing_hash)
            {
                return Err(PrePoolError::ShortIdCollision {
                    short_id: next.short_id.clone(),
                    existing_hash: existing_hash.clone(),
                });
            }
        }

        // Validate the aggregate parent projection. Per-entry validation is
        // insufficient when several new children name the same parent.
        let mut parent_counts = HashMap::<Byte32, usize>::new();
        for (_, old, next) in &planned {
            for parent in old
                .iter()
                .flat_map(|entry| entry.dependencies.iter())
                .map(DependencyKey::parent_hash)
                .collect::<BTreeSet<_>>()
            {
                let count = parent_counts
                    .entry(parent.clone())
                    .or_insert_with(|| self.by_parent.get(&parent).map_or(0, BTreeSet::len));
                *count = count
                    .checked_sub(1)
                    .ok_or(PrePoolError::Repair("recovery parent projection underflow"))?;
            }
            for parent in next
                .dependencies
                .iter()
                .map(DependencyKey::parent_hash)
                .collect::<BTreeSet<_>>()
            {
                let count = parent_counts
                    .entry(parent.clone())
                    .or_insert_with(|| self.by_parent.get(&parent).map_or(0, BTreeSet::len));
                *count = count
                    .checked_add(1)
                    .ok_or(PrePoolError::ResidencyChargeOverflow)?;
                if *count > self.limits.max_dependents_per_parent {
                    return Err(PrePoolError::ParentFanoutLimitExceeded(parent));
                }
            }
        }

        let removed = planned
            .iter()
            .try_fold(Residency::default(), |usage, (_, old, _)| {
                old.as_ref().map_or(Ok(usage), |entry| {
                    usage
                        .checked_add(Residency::new(1, entry.charge_bytes))
                        .ok_or(PrePoolError::ResidencyChargeOverflow)
                })
            })?;
        let added = planned
            .iter()
            .try_fold(Residency::default(), |usage, (_, _, next)| {
                usage
                    .checked_add(Residency::new(1, next.charge_bytes))
                    .ok_or(PrePoolError::ResidencyChargeOverflow)
            })?;
        let final_total = self
            .total_usage
            .checked_sub(removed)
            .and_then(|usage| usage.checked_add(added))
            .ok_or(PrePoolError::Repair("recovery total usage arithmetic"))?;
        if !final_total.fits(self.limits.total) {
            return Err(PrePoolError::TotalBudgetExceeded);
        }

        // Recovery owners are trusted and non-conflict. Replacing an old
        // remote/conflict owner can only reduce those sub-partitions, but the
        // exact subtraction still proves the current primary/index equation.
        let removed_remote =
            planned
                .iter()
                .try_fold(Residency::default(), |usage, (_, old, _)| {
                    old.as_ref()
                        .filter(|entry| entry.source.peer().is_some())
                        .map_or(Ok(usage), |entry| {
                            usage
                                .checked_add(Residency::new(1, entry.charge_bytes))
                                .ok_or(PrePoolError::ResidencyChargeOverflow)
                        })
                })?;
        self.remote_usage
            .checked_sub(removed_remote)
            .ok_or(PrePoolError::Repair("recovery remote usage arithmetic"))?;
        let removed_conflict =
            planned
                .iter()
                .try_fold(Residency::default(), |usage, (_, old, _)| {
                    old.as_ref()
                        .filter(|entry| Self::is_conflict(entry))
                        .map_or(Ok(usage), |entry| {
                            usage
                                .checked_add(Residency::new(1, entry.charge_bytes))
                                .ok_or(PrePoolError::ResidencyChargeOverflow)
                        })
                })?;
        self.conflict_usage
            .checked_sub(removed_conflict)
            .ok_or(PrePoolError::Repair("recovery conflict usage arithmetic"))?;
        let active_removals = planned
            .iter()
            .filter_map(|(_, old, _)| {
                old.as_ref()
                    .and_then(|entry| Self::active_owner(entry.source, &entry.state))
            })
            .collect::<Vec<_>>();
        self.active_work
            .checked_sub(active_removals.len())
            .ok_or(PrePoolError::Repair("recovery active work projection"))?;
        let mut removal_counts = HashMap::<WorkOwner, usize>::new();
        for owner in &active_removals {
            let count = removal_counts.entry(*owner).or_default();
            *count = count
                .checked_add(1)
                .ok_or(PrePoolError::Repair("recovery active owner projection"))?;
        }
        for (owner, removed) in removal_counts {
            self.active_by_owner
                .get(&owner)
                .copied()
                .ok_or(PrePoolError::Repair("recovery active owner missing"))?
                .checked_sub(removed)
                .ok_or(PrePoolError::Repair("recovery active owner projection"))?;
        }

        // Total Apply: all fallible cohort predicates above precede mutation.
        for (hash, old, _) in &planned {
            if let Some(old) = old {
                let active = Self::active_owner(old.source, &old.state);
                self.detach_indexes(hash, old);
                self.apply_usage_delta(Some(old), None);
                self.entries.remove(hash);
                self.apply_active_transition(active, None);
            }
        }
        for (hash, _, next) in &planned {
            self.apply_usage_delta(None, Some(next));
            self.entries.insert(hash.clone(), next.clone());
            self.attach_indexes(hash, next);
        }
        self.next_version = version_cursor;
        self.next_arrival = arrival_cursor;
        self.next_recovery_session = next_session;

        Ok(RecoveryBatch {
            session,
            retained: planned.len(),
        })
    }

    /// Lease the next retained item of one recovery session. The raw payload
    /// remains owned by the entry while direct validation awaits.
    pub(crate) fn checkout_recovery(
        &mut self,
        session: u128,
    ) -> Result<Option<ResolveLease>, PrePoolError> {
        let Some(key) = self
            .recovery
            .iter()
            .find(|key| key.meta.session == session)
            .cloned()
        else {
            return Ok(None);
        };
        let old = self
            .validate_location(&key.hash, key.version, PrePoolLocation::RecoveryRetained)?
            .clone();
        let version = self.allocate_version()?;
        let mut next = old.clone();
        next.version = version;
        next.state = EntryState::ResolveLeased;
        next.charge_bytes = self.entry_charge(&next)?;
        self.check_usage_delta(Some(&old), Some(&next))?;
        // The retained reorg handler is a single fixed trusted borrower, not
        // part of the attacker-scalable worker set. Its payload is already
        // charged by the entry, so Remote active-work saturation cannot delay
        // chain convergence.
        self.replace_entry(&key.hash, next.clone())?;
        Ok(Some(ResolveLease {
            hash: key.hash,
            lane: ResolveLane::Ordered,
            version,
            payload: Arc::clone(&next.raw),
        }))
    }

    #[cfg(test)]
    pub(crate) fn recovery_session_pending(&self, session: u128) -> bool {
        self.entries.values().any(|entry| {
            entry
                .recovery
                .is_some_and(|metadata| metadata.session == session)
        })
    }

    #[cfg(test)]
    pub(crate) fn exhaust_recovery_sessions_for_test(&mut self) {
        self.next_recovery_session = u128::MAX;
    }

    pub(crate) fn recovery_snapshot(&self) -> Vec<RecoverySnapshotItem> {
        let mut items = self
            .entries
            .values()
            .filter_map(|entry| {
                entry.recovery.map(|meta| RecoverySnapshotItem {
                    tx: entry.raw.tx.clone(),
                    meta,
                })
            })
            .collect::<Vec<_>>();
        items.sort_unstable_by_key(|item| item.meta);
        items
    }
}
