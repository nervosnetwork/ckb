use super::*;

impl PrePoolKernel {
    pub(super) fn causal_keys(entry: &Entry) -> BTreeSet<DependencyKey> {
        entry.dependencies.clone()
    }

    /// Preserve a rejected leased/ready owner as conflict history without an
    /// observable remove/re-admit gap. The same checked replacement accounts
    /// the conflict-history quota before changing any primary/index state.
    pub(crate) fn park_conflict_at(
        &mut self,
        hash: &Byte32,
        version: EntryVersion,
        expected: PrePoolLocation,
    ) -> Result<TerminalRecord, PrePoolError> {
        let old = self.validate_location(hash, version, expected)?.clone();
        let keys = Self::causal_keys(&old);
        let record = TerminalRecord {
            hash: hash.clone(),
            raw: Arc::clone(&old.raw),
            source: old.source,
        };
        self.move_to_wait(hash, WaitReason::Conflict, keys, None)?;
        Ok(record)
    }

    /// Conflict history is optional armor, not an executable prerequisite.
    /// A full history partition deterministically terminalizes the rejected
    /// owner; only a structural defect may escape this boundary.
    pub(crate) fn park_conflict_or_terminalize(
        &mut self,
        hash: &Byte32,
        version: EntryVersion,
        expected: PrePoolLocation,
    ) -> Result<TerminalRecord, PrePoolError> {
        match self.park_conflict_at(hash, version, expected) {
            Ok(record) => Ok(record),
            Err(error) if error.is_capacity_rejection() => {
                self.validate_location(hash, version, expected)?;
                self.remove_entry(hash, TerminalDisposition::Rejected)
            }
            Err(error) => Err(error),
        }
    }

    fn observed_dependencies(
        &self,
        keys: impl IntoIterator<Item = DependencyKey>,
    ) -> BTreeMap<DependencyKey, u128> {
        keys.into_iter()
            .map(|key| {
                let epoch = self.availability_epoch.get(&key).copied().unwrap_or(0);
                (key, epoch)
            })
            .collect()
    }

    pub(super) fn move_to_wait(
        &mut self,
        hash: &Byte32,
        reason: WaitReason,
        keys: BTreeSet<DependencyKey>,
        charge_bytes: Option<usize>,
    ) -> Result<EntryVersion, PrePoolError> {
        if keys.is_empty() {
            return Err(PrePoolError::Repair("empty wait transition"));
        }
        let keys = keys
            .into_iter()
            .map(DependencyKey::into_compact)
            .collect::<BTreeSet<_>>();
        let old = self
            .entries
            .get(hash)
            .cloned()
            .ok_or_else(|| PrePoolError::Missing(hash.clone()))?;
        let mut next = old.clone();
        next.version = self.allocate_version()?;
        next.dependencies.extend(keys.iter().cloned());
        if let Some(charge_bytes) = charge_bytes {
            next.payload_charge_bytes = charge_bytes;
        }
        next.state = EntryState::Wait(WaitState {
            reason,
            observed: self.observed_dependencies(keys),
        });
        next.charge_bytes = self.entry_charge(&next)?;
        self.replace_entry(hash, next.clone())?;
        self.deactivate_if_leased(old.source, &old.state)?;
        Ok(next.version)
    }

    pub(super) fn move_to_resolve(
        &mut self,
        hash: &Byte32,
        lane: ResolveLane,
    ) -> Result<EntryVersion, PrePoolError> {
        let old = self
            .entries
            .get(hash)
            .cloned()
            .ok_or_else(|| PrePoolError::Missing(hash.clone()))?;
        let mut next = old.clone();
        next.version = self.allocate_version()?;
        next.state = EntryState::ResolveQueued { lane };
        next.charge_bytes = self.entry_charge(&next)?;
        self.replace_entry(hash, next.clone())?;
        self.deactivate_if_leased(old.source, &old.state)?;
        Ok(next.version)
    }

    pub(crate) fn requeue_resolve(
        &mut self,
        lease: &ResolveLease,
    ) -> Result<EntryVersion, PrePoolError> {
        self.validate_location(&lease.hash, lease.version, PrePoolLocation::ResolveLeased)?;
        self.move_to_resolve(&lease.hash, lease.lane)
    }

    pub(crate) fn wait_resolve(
        &mut self,
        lease: &ResolveLease,
        keys: BTreeSet<DependencyKey>,
    ) -> Result<EntryVersion, PrePoolError> {
        self.validate_location(&lease.hash, lease.version, PrePoolLocation::ResolveLeased)?;
        self.move_to_wait(&lease.hash, WaitReason::Missing, keys, None)
    }

    pub(crate) fn verification_retry_resolution(
        &mut self,
        lease: &VerifyLease,
        keys: BTreeSet<DependencyKey>,
    ) -> Result<(EntryVersion, PrePoolSource), PrePoolError> {
        let source = self
            .validate_location(&lease.hash, lease.version, PrePoolLocation::VerifyLeased)?
            .source;
        let version = if keys.is_empty() {
            self.move_to_resolve(&lease.hash, ResolveLane::Ordered)?
        } else {
            self.move_to_wait(&lease.hash, WaitReason::Missing, keys, None)?
        };
        Ok((version, source))
    }

    /// Record a level change. This never scans the waiter fan-out while a
    /// TxPool write guard is held; bounded maintenance resumes each ordered
    /// bucket by its last processed exact edge.
    pub(crate) fn note_available(
        &mut self,
        keys: impl IntoIterator<Item = DependencyKey>,
    ) -> Result<(), PrePoolError> {
        let mut unique = BTreeSet::new();
        unique.extend(keys.into_iter().map(DependencyKey::into_compact));
        for key in unique {
            if !self.dirty.contains_key(&key)
                && self.waiters.get(&key).is_none_or(|edges| edges.is_empty())
            {
                continue;
            }
            let epoch = self
                .availability_epoch
                .get(&key)
                .copied()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or(PrePoolError::VersionExhausted)?;
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
        Ok(())
    }

    pub(crate) fn wait_wake_pending(&self) -> bool {
        !self.dirty.is_empty()
    }

    pub(crate) fn drain_wait_wakes(&mut self, limit: usize) -> Result<usize, PrePoolError> {
        let mut examined = 0;
        while examined < limit {
            let Some(key) = self.dirty_order.pop_front() else {
                break;
            };
            let Some(dirty) = self.dirty.get(&key).cloned() else {
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

            examined += 1;
            if let Some(current) = self.dirty.get_mut(&key) {
                current.cursor = Some(edge.clone());
            }
            self.dirty_order.push_back(key.clone());

            let should_wake = self.entries.get(&edge.hash).is_some_and(|entry| {
                entry.version == edge.version
                    && match &entry.state {
                        EntryState::Wait(wait) => wait
                            .observed
                            .get(&key)
                            .is_some_and(|observed| *observed < dirty.target_epoch),
                        _ => false,
                    }
            });
            if should_wake {
                match self.move_to_resolve(&edge.hash, ResolveLane::Ordered) {
                    Ok(_) => {}
                    Err(error) if error.is_retryable_capacity_rejection() => {
                        return Err(PrePoolError::Repair(
                            "wait wake increased a continuously reserved budget",
                        ));
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(examined)
    }

    /// Demote resolved/verified consumers immediately to the one resolve
    /// state. No Invalidated location or later terminal cascade exists.
    pub(crate) fn parents_unavailable(
        &mut self,
        parents: &HashSet<Byte32>,
    ) -> Result<(), PrePoolError> {
        let mut affected = BTreeSet::new();
        for parent in parents {
            if let Some(children) = self.by_parent.get(parent) {
                affected.extend(children.iter().cloned());
            }
        }
        for hash in affected {
            let Some(entry) = self.entries.get(&hash) else {
                continue;
            };
            let mut keys = Self::causal_keys(entry)
                .into_iter()
                .filter(|key| parents.contains(&key.parent_hash()))
                .collect::<BTreeSet<_>>();
            if keys.is_empty() {
                return Err(PrePoolError::Repair(
                    "parent index has no canonical dependency key",
                ));
            }
            if let EntryState::Wait(wait) = &entry.state {
                keys.extend(wait.observed.keys().cloned());
            }
            self.move_to_wait(&hash, WaitReason::Missing, keys, None)?;
        }
        Ok(())
    }

    pub(crate) fn retain_conflict(
        &mut self,
        raw: PipelineRawTx,
        source: PrePoolSource,
        keys: BTreeSet<DependencyKey>,
        expires_at: Option<u64>,
    ) -> Result<(bool, Vec<TerminalRecord>), PrePoolError> {
        let hash = raw.tx.hash();
        if keys.is_empty() {
            return Err(PrePoolError::Repair("conflict history without wake keys"));
        }
        let keys = keys
            .into_iter()
            .map(DependencyKey::into_compact)
            .collect::<BTreeSet<_>>();
        if self.entries.contains_key(&hash) {
            let old = self.entries.get(&hash).cloned().unwrap();
            let mut next = old.clone();
            let trusted_refresh = source == PrePoolSource::Proposal
                && (next.source != PrePoolSource::Proposal
                    || next.raw.tx.witness_hash() != raw.tx.witness_hash());
            if trusted_refresh {
                next.source = source;
                next.payload_charge_bytes = raw.charge_bytes();
                next.raw = Arc::new(raw);
                next.expires_at = None;
            }
            next.version = self.allocate_version()?;
            next.dependencies.extend(keys.iter().cloned());
            next.state = EntryState::Wait(WaitState {
                reason: WaitReason::Conflict,
                observed: self.observed_dependencies(keys),
            });
            next.charge_bytes = self.entry_charge(&next)?;
            self.replace_entry(&hash, next)?;
            self.deactivate_if_leased(old.source, &old.state)?;
            return Ok((false, Vec::new()));
        }

        let short_id = raw.tx.proposal_short_id();
        if let Some(existing_hash) = self.by_short_id.get(&short_id) {
            return Err(PrePoolError::ShortIdCollision {
                short_id,
                existing_hash: existing_hash.clone(),
            });
        }
        let version = self.allocate_version()?;
        let arrival = self.allocate_arrival()?;
        let mut dependencies = conflict_dependency_keys(&raw.tx, std::iter::empty());
        dependencies.extend(keys.iter().cloned());
        let payload_charge_bytes = raw.charge_bytes();
        let mut entry = Entry {
            short_id,
            raw: Arc::new(raw),
            source,
            state: EntryState::Wait(WaitState {
                reason: WaitReason::Conflict,
                observed: self.observed_dependencies(keys),
            }),
            version,
            arrival,
            expires_at,
            payload_charge_bytes,
            charge_bytes: 0,
            dependencies,
        };
        entry.charge_bytes = self.entry_charge(&entry)?;
        self.validate_entry_shape(&hash, &entry)?;
        self.check_usage_delta(None, Some(&entry))?;
        self.apply_usage_delta(None, Some(&entry));
        self.entries.insert(hash.clone(), entry.clone());
        self.attach_indexes(&hash, &entry);
        Ok((true, Vec::new()))
    }

    pub(crate) fn remove_conflict_hash(&mut self, hash: &Byte32) -> bool {
        let conflict = self.entries.get(hash).is_some_and(|entry| {
            matches!(
                entry.state,
                EntryState::Wait(WaitState {
                    reason: WaitReason::Conflict,
                    ..
                })
            )
        });
        conflict
            && self
                .remove_entry(hash, TerminalDisposition::Removed)
                .is_ok()
    }
}
