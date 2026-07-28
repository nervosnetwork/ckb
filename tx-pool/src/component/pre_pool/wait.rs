use super::lifecycle::MutationSet;
use super::lifecycle::PreparedKernelMutation;
use super::*;

impl PrePoolKernel {
    pub(super) fn causal_keys(entry: &Entry) -> BTreeSet<DependencyKey> {
        entry.dependencies.clone()
    }

    /// Preserve a rejected leased/ready owner as conflict history without an
    /// observable remove/re-admit gap. The same checked replacement accounts
    /// the conflict-history quota before changing any primary/index state.
    fn park_validated_conflict(&mut self, hash: &Byte32) -> Result<TerminalRecord, PrePoolError> {
        let (keys, record) = {
            let old = self.entries.get(hash).ok_or_else(|| {
                PrePoolError::ProjectionInconsistent("validated lease lost its primary")
            })?;
            (
                Self::causal_keys(old),
                TerminalRecord {
                    hash: hash.clone(),
                    raw: Arc::clone(&old.raw),
                    source: old.source,
                },
            )
        };
        self.move_to_wait(hash, WaitReason::Conflict, keys, None)?;
        Ok(record)
    }

    /// Conflict history is optional armor, not an executable prerequisite.
    /// A full history partition deterministically terminalizes the rejected
    /// owner; only a structural defect may escape this boundary.
    pub(crate) fn park_resolve_conflict_or_terminalize(
        &mut self,
        lease: &ResolveLease,
    ) -> Result<TerminalRecord, PrePoolError> {
        self.validate_resolve_lease(lease)?;
        match self.park_validated_conflict(&lease.hash) {
            Ok(record) => Ok(record),
            Err(error) if error.is_capacity_rejection() => {
                self.validate_resolve_lease(lease)?;
                self.remove_unavailable_entry(&lease.hash)
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) fn park_verify_conflict_or_terminalize(
        &mut self,
        lease: &VerifyLease,
    ) -> Result<TerminalRecord, PrePoolError> {
        self.validate_verify_lease(lease)?;
        match self.park_validated_conflict(&lease.hash) {
            Ok(record) => Ok(record),
            Err(error) if error.is_capacity_rejection() => {
                self.validate_verify_lease(lease)?;
                self.remove_unavailable_entry(&lease.hash)
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn observed_dependencies(
        &self,
        keys: impl IntoIterator<Item = DependencyKey>,
    ) -> Result<ObservedDependencies, PrePoolError> {
        let observed = keys
            .into_iter()
            .map(|key| {
                let epoch = self.availability_epoch.get(&key).copied().unwrap_or(0);
                (key, epoch)
            })
            .collect();
        ObservedDependencies::new(observed)
    }

    pub(super) fn move_to_wait(
        &mut self,
        hash: &Byte32,
        reason: WaitReason,
        keys: BTreeSet<DependencyKey>,
        charge_bytes: Option<usize>,
    ) -> Result<(), PrePoolError> {
        let keys = keys
            .into_iter()
            .map(DependencyKey::into_compact)
            .collect::<BTreeSet<_>>();
        let mut next = self
            .entries
            .get(hash)
            .cloned()
            .ok_or_else(|| PrePoolError::Missing(hash.clone()))?
            .into_draft();
        let mut revision_cursor = self.next_revision;
        next.revision = EntryRevision::take(&mut revision_cursor)?;
        next.dependencies.extend(keys.iter().cloned());
        if let Some(charge_bytes) = charge_bytes {
            next.payload_charge_bytes = charge_bytes;
        }
        next.state = EntryState::Wait(WaitState {
            reason,
            observed: self.observed_dependencies(keys)?,
        });
        self.replace_entry(
            hash,
            next,
            revision_cursor,
            self.next_arrival,
            super::lifecycle::ReplacementMode::Ordinary,
        )?;
        Ok(())
    }

    pub(super) fn move_to_resolve(
        &mut self,
        hash: &Byte32,
        lane: ResolveLane,
    ) -> Result<(), PrePoolError> {
        let mut next = self
            .entries
            .get(hash)
            .cloned()
            .ok_or_else(|| PrePoolError::Missing(hash.clone()))?
            .into_draft();
        let mut revision_cursor = self.next_revision;
        next.revision = EntryRevision::take(&mut revision_cursor)?;
        next.state = EntryState::ResolveQueued { lane };
        self.replace_entry(
            hash,
            next,
            revision_cursor,
            self.next_arrival,
            super::lifecycle::ReplacementMode::Ordinary,
        )?;
        Ok(())
    }

    pub(crate) fn requeue_resolve(&mut self, lease: &ResolveLease) -> Result<(), PrePoolError> {
        self.validate_resolve_lease(lease)?;
        self.move_to_resolve(&lease.hash, lease.lane)
    }

    pub(crate) fn wait_resolve(
        &mut self,
        lease: &ResolveLease,
        keys: BTreeSet<DependencyKey>,
    ) -> Result<(), PrePoolError> {
        self.validate_resolve_lease(lease)?;
        self.move_to_wait(&lease.hash, WaitReason::Missing, keys, None)
    }

    pub(crate) fn verification_retry_resolution(
        &mut self,
        lease: &VerifyLease,
        keys: BTreeSet<DependencyKey>,
    ) -> Result<(), PrePoolError> {
        self.validate_verify_lease(lease)?;
        if keys.is_empty() {
            self.move_to_resolve(&lease.hash, ResolveLane::Ordered)
        } else {
            self.move_to_wait(&lease.hash, WaitReason::Missing, keys, None)
        }
    }

    /// Return the exact dependency levels observed by consumers of `parents`.
    /// The reverse index bounds this work by the configured parent fan-out;
    /// callers use the keys to publish one level change after their atomic
    /// primary/cohort Apply.
    pub(super) fn dependency_keys_for_parents(
        &self,
        parents: &HashSet<Byte32>,
    ) -> BTreeSet<DependencyKey> {
        let mut keys = BTreeSet::new();
        for parent in parents {
            let Some(children) = self.by_parent.get(parent) else {
                continue;
            };
            for child in children {
                let Some(entry) = self.entries.get(child) else {
                    continue;
                };
                keys.extend(
                    entry
                        .dependencies
                        .iter()
                        .filter(|key| parents.contains(&key.parent_hash()))
                        .cloned(),
                );
            }
        }
        keys
    }

    /// Record a dependency level change. This never scans the waiter fan-out while a
    /// TxPool write guard is held; bounded maintenance resumes each ordered
    /// bucket by its last processed exact edge.
    pub(super) fn plan_dependency_changes_for_cohort(
        &self,
        keys: impl IntoIterator<Item = DependencyKey>,
        cohort: &super::lifecycle::CohortPlan,
    ) -> Result<DependencyChangePlan, PrePoolError> {
        let mut unique = BTreeSet::new();
        unique.extend(keys.into_iter().map(DependencyKey::into_compact));
        let mut planned = Vec::with_capacity(unique.len());
        for key in unique {
            if !self.dirty.contains_key(&key) && self.projected_waiter_count(&key, cohort)? == 0 {
                continue;
            }
            let epoch = self
                .availability_epoch
                .get(&key)
                .copied()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or(PrePoolError::CounterExhausted)?;
            planned.push((key, epoch));
        }
        Ok(DependencyChangePlan(planned))
    }

    pub(super) fn apply_dependency_change_plan(
        &mut self,
        DependencyChangePlan(planned): DependencyChangePlan,
    ) {
        for (key, epoch) in planned {
            self.availability_epoch.insert(key.clone(), epoch);
            if let Some(dirty) = self.dirty.get_mut(&key) {
                dirty.pending_epoch = Some(epoch);
            } else if self
                .waiters
                .get(&key)
                .is_some_and(|edges| !edges.is_empty())
            {
                self.dirty.insert(
                    key.clone(),
                    DirtyDependency {
                        target_epoch: epoch,
                        cursor: None,
                        pending_epoch: None,
                    },
                );
                self.dirty_order.push_back(key);
            }
        }
    }

    pub(crate) fn wait_wake_pending(&self) -> bool {
        !self.dirty.is_empty()
    }

    pub(crate) fn drain_wait_wakes(&mut self, limit: usize) -> Result<usize, PrePoolError> {
        let mut examined = 0;
        while examined < limit {
            let Some(key) = self.dirty_order.front().cloned() else {
                break;
            };
            let Some(dirty) = self.dirty.get(&key).cloned() else {
                self.dirty_order.pop_front();
                continue;
            };
            let next = self.waiters.get(&key).and_then(|edges| {
                dirty.cursor.as_ref().map_or_else(
                    || edges.iter().next().cloned(),
                    |cursor| {
                        edges
                            .range((
                                std::ops::Bound::Excluded(cursor),
                                std::ops::Bound::Unbounded,
                            ))
                            .next()
                            .cloned()
                    },
                )
            });
            let Some(edge) = next else {
                self.dirty_order.pop_front();
                if let Some(pending_epoch) = dirty.pending_epoch {
                    self.dirty.insert(
                        key.clone(),
                        DirtyDependency {
                            target_epoch: pending_epoch,
                            cursor: None,
                            pending_epoch: None,
                        },
                    );
                    self.dirty_order.push_back(key);
                } else {
                    self.dirty.remove(&key);
                    if self.waiters.get(&key).is_none_or(|edges| edges.is_empty()) {
                        self.availability_epoch.remove(&key);
                    }
                }
                continue;
            };

            examined = examined
                .checked_add(1)
                .ok_or(PrePoolError::CounterExhausted)?;
            let should_wake = self.entries.get(&edge.hash).is_some_and(|entry| {
                entry.revision == edge.revision
                    && match &entry.state {
                        EntryState::Wait(wait) => wait
                            .observed
                            .get(&key)
                            .is_some_and(|observed| *observed < dirty.target_epoch),
                        _ => false,
                    }
            });
            if should_wake {
                let old = self.entries.get(&edge.hash).cloned().ok_or(
                    PrePoolError::ProjectionInconsistent("wake edge lost its validated primary"),
                )?;
                let mut next = old.into_draft();
                let mut next_revision = self.next_revision;
                next.revision = EntryRevision::take(&mut next_revision)?;
                next.state = EntryState::ResolveQueued {
                    lane: ResolveLane::Ordered,
                };
                match self.replace_entry(
                    &edge.hash,
                    next,
                    next_revision,
                    self.next_arrival,
                    super::lifecycle::ReplacementMode::Ordinary,
                ) {
                    Ok(()) => {}
                    Err(error) if error.is_retryable_capacity_rejection() => {
                        return Err(PrePoolError::ProjectionInconsistent(
                            "wait wake exceeded its continuously reserved budget",
                        ));
                    }
                    Err(error) => return Err(error),
                }
            }
            // Cursor publication is derived bookkeeping, not part of the
            // primary wake transaction. Advance it only after the optional
            // replacement has completed its total Plan/Apply transition.
            self.dirty_order.pop_front();
            if let Some(current) = self.dirty.get_mut(&key) {
                current.cursor = Some(edge);
            }
            self.dirty_order.push_back(key);
        }
        Ok(examined)
    }

    /// Demote resolved/verified consumers immediately to the one resolve
    /// state. No Invalidated location or later terminal cascade exists.
    pub(super) fn unavailable_replacements(
        &self,
        parents: &HashSet<Byte32>,
        revision_cursor: &mut EntryRevision,
    ) -> Result<Vec<(Byte32, StoredEntry)>, PrePoolError> {
        let mut affected = BTreeSet::new();
        for parent in parents {
            if let Some(children) = self.by_parent.get(parent) {
                affected.extend(children.iter().cloned());
            }
        }
        let mut replacements = Vec::with_capacity(affected.len());
        for hash in affected {
            let Some(entry) = self.entries.get(&hash).cloned() else {
                continue;
            };
            let mut keys = Self::causal_keys(&entry)
                .into_iter()
                .filter(|key| parents.contains(&key.parent_hash()))
                .collect::<BTreeSet<_>>();
            if keys.is_empty() {
                return Err(PrePoolError::ProjectionInconsistent(
                    "parent reverse index has no matching causal dependency",
                ));
            }
            if let EntryState::Wait(wait) = &entry.state {
                keys.extend(wait.observed.keys().cloned());
            }
            let revision = EntryRevision::take(revision_cursor)?;
            let mut next = entry.into_draft();
            next.revision = revision;
            next.dependencies.extend(keys.iter().cloned());
            next.state = EntryState::Wait(WaitState {
                reason: WaitReason::Missing,
                observed: self.observed_dependencies(keys)?,
            });
            let next = StoredEntry::prepare(next, self.limits)?;
            replacements.push((hash, next));
        }
        Ok(replacements)
    }

    /// Compile the pre-pool half of a cross-authority dependency change.
    /// Parent-loss demotions and newly available dependency levels share one
    /// cohort and one exclusive Apply capability; the accepted-pool caller can
    /// therefore validate both authorities before mutating either one.
    pub(crate) fn prepare_dependency_reconciliation(
        &mut self,
        unavailable_parents: &HashSet<Byte32>,
        available: impl IntoIterator<Item = DependencyKey>,
    ) -> Result<PreparedKernelMutation<'_>, PrePoolError> {
        let mut changed_keys = self.dependency_keys_for_parents(unavailable_parents);
        changed_keys.extend(available);
        let mut revision_cursor = self.next_revision;
        let mut desired = MutationSet::default();
        for (_, entry) in
            self.unavailable_replacements(unavailable_parents, &mut revision_cursor)?
        {
            desired.set_entry(entry);
        }
        self.prepare_cohort(desired, revision_cursor, self.next_arrival, changed_keys)
    }

    pub(crate) fn retain_conflict(
        &mut self,
        raw: PipelineRawTx,
        source: PrePoolSource,
        keys: BTreeSet<DependencyKey>,
        expires_at: Option<u64>,
    ) -> Result<(bool, Vec<TerminalRecord>), PrePoolError> {
        let hash = raw.tx.hash();
        let keys = keys
            .into_iter()
            .map(DependencyKey::into_compact)
            .collect::<BTreeSet<_>>();
        if self.entries.contains_key(&hash) {
            let mut next = self
                .entries
                .get(&hash)
                .cloned()
                .ok_or_else(|| PrePoolError::Missing(hash.clone()))?
                .into_draft();
            let trusted_refresh = source == PrePoolSource::Proposal
                && (next.source != PrePoolSource::Proposal
                    || next.raw.tx.witness_hash() != raw.tx.witness_hash());
            if trusted_refresh {
                next.source = source;
                next.payload_charge_bytes = raw.charge_bytes();
                next.raw = Arc::new(raw);
                next.expires_at = None;
            }
            let mut revision_cursor = self.next_revision;
            next.revision = EntryRevision::take(&mut revision_cursor)?;
            next.dependencies.extend(keys.iter().cloned());
            next.state = EntryState::Wait(WaitState {
                reason: WaitReason::Conflict,
                observed: self.observed_dependencies(keys)?,
            });
            self.replace_entry(
                &hash,
                next,
                revision_cursor,
                self.next_arrival,
                super::lifecycle::ReplacementMode::Ordinary,
            )?;
            return Ok((false, Vec::new()));
        }

        let short_id = raw.tx.proposal_short_id();
        if let Some(existing_hash) = self.by_short_id.get(&short_id) {
            return Err(PrePoolError::ShortIdCollision(
                short_id,
                existing_hash.clone(),
            ));
        }
        let mut revision_cursor = self.next_revision;
        let revision = EntryRevision::take(&mut revision_cursor)?;
        let mut arrival_cursor = self.next_arrival;
        let arrival = Arrival::take(&mut arrival_cursor)?;
        let mut dependencies = conflict_dependency_keys(&raw.tx, std::iter::empty());
        dependencies.extend(keys.iter().cloned());
        let payload_charge_bytes = raw.charge_bytes();
        let entry = Entry {
            raw: Arc::new(raw),
            source,
            state: EntryState::Wait(WaitState {
                reason: WaitReason::Conflict,
                observed: self.observed_dependencies(keys)?,
            }),
            revision,
            arrival,
            expires_at,
            payload_charge_bytes,
            dependencies,
        };
        let entry = StoredEntry::prepare(entry, self.limits)?;
        self.validate_entry_shape(&hash, &entry)?;
        let usage_plan = self.plan_usage_delta(None, Some(&entry))?;
        let mut queue_lengths = self.queues.map(FairQueue::len);
        self.apply_queue_transition(&mut queue_lengths, None, Some(&entry))?;
        self.apply_usage_plan(usage_plan);
        self.attach_indexes(&hash, &entry);
        self.entries.insert(entry.hash().clone(), entry);
        for (lane, len) in queue_lengths.into_entries() {
            self.queues.get_mut(lane).set_len(len);
        }
        self.next_revision = revision_cursor;
        self.next_arrival = arrival_cursor;
        Ok((true, Vec::new()))
    }
}
