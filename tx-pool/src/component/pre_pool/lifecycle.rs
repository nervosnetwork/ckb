use super::*;

impl PrePoolKernel {
    /// Snapshot terminal publication metadata without changing ownership.
    /// Callers pair this immutable record with an effect-journal capacity
    /// check before executing the matching total terminal transition.
    pub(crate) fn terminal_record(&self, hash: &Byte32) -> Option<TerminalRecord> {
        self.entries.get(hash).map(|entry| TerminalRecord {
            hash: hash.clone(),
            raw: Arc::clone(&entry.raw),
            source: entry.source,
        })
    }

    pub(super) fn validate_entry_shape(
        &self,
        hash: &Byte32,
        entry: &Entry,
    ) -> Result<(), PrePoolError> {
        if entry
            .dependencies
            .iter()
            .any(|key| key.parent_hash() == *hash)
        {
            return Err(PrePoolError::SelfDependency(hash.clone()));
        }
        if entry.dependencies.len() > self.limits.max_dependencies_per_entry {
            return Err(PrePoolError::DependencyLimitExceeded);
        }
        let parents = entry
            .dependencies
            .iter()
            .map(DependencyKey::parent_hash)
            .collect::<BTreeSet<_>>();
        for parent in &parents {
            let existing = self
                .by_parent
                .get(parent)
                .map_or(0, |children| children.len());
            let already = self.entries.get(hash).is_some_and(|old| {
                old.dependencies
                    .iter()
                    .any(|key| key.parent_hash() == *parent)
            });
            if !already && existing >= self.limits.max_dependents_per_parent {
                return Err(PrePoolError::ParentFanoutLimitExceeded(parent.clone()));
            }
        }
        if let EntryState::Wait(wait) = &entry.state
            && (wait.observed.is_empty()
                || wait.observed.len() > self.limits.max_dependencies_per_entry)
        {
            return Err(PrePoolError::Repair(
                "wait owner has an invalid dependency-key count",
            ));
        }
        if let EntryState::Wait(wait) = &entry.state
            && wait
                .observed
                .keys()
                .any(|key| !entry.dependencies.contains(key))
        {
            return Err(PrePoolError::Repair(
                "wait dependency key has no canonical parent",
            ));
        }
        if let EntryState::Ready { inputs, .. } = &entry.state {
            if inputs.is_empty() || inputs.len() > self.limits.max_inputs_per_ready {
                return Err(PrePoolError::ConflictInputLimitExceeded);
            }
            for input in inputs {
                let existing = self
                    .ready_by_input
                    .get(input)
                    .map_or(0, |candidates| candidates.len());
                let already = self.entries.get(hash).is_some_and(|old| match &old.state {
                    EntryState::Ready { inputs, .. } => inputs.contains(input),
                    _ => false,
                });
                if !already && existing >= self.limits.max_candidates_per_input {
                    return Err(PrePoolError::ConflictCandidateLimitExceeded(input.clone()));
                }
            }
        }
        if entry.charge_bytes != self.entry_charge(entry)? {
            return Err(PrePoolError::Repair(
                "entry charge is not derived from its primary state",
            ));
        }
        Ok(())
    }

    pub(super) fn deadline_key(hash: &Byte32, entry: &Entry) -> Option<DeadlineKey> {
        entry.expires_at.map(|expires_at| DeadlineKey {
            expires_at,
            hash: hash.clone(),
            version: entry.version,
        })
    }

    pub(super) fn attach_indexes(&mut self, hash: &Byte32, entry: &Entry) {
        debug_assert_eq!(self.by_short_id.get(&entry.short_id), None);
        self.by_short_id
            .insert(entry.short_id.clone(), hash.clone());
        if let Some(peer) = entry.source.peer() {
            self.by_peer.entry(peer).or_default().insert(hash.clone());
        }
        for parent in entry
            .dependencies
            .iter()
            .map(DependencyKey::parent_hash)
            .collect::<BTreeSet<_>>()
        {
            self.by_parent
                .entry(parent)
                .or_default()
                .insert(hash.clone());
        }
        if let Some(key) = entry.work_key(hash, self.limits.verify_fee_rate_ordering) {
            let lane = match entry.state {
                EntryState::ResolveQueued { lane } => Self::lane_for_resolve(lane),
                EntryState::VerifyQueued { .. } => WorkLane::Verify,
                _ => unreachable!("work key exists only for a queued state"),
            };
            self.queues[lane.index()]
                .insert(key)
                .expect("validated entry has one exact queue key");
            self.refresh_owner_runnable(entry.source.into());
        }
        if let EntryState::Wait(wait) = &entry.state {
            let edge = WaitEdge {
                hash: hash.clone(),
                version: entry.version,
            };
            for key in wait.observed.keys() {
                self.waiters
                    .entry(key.clone())
                    .or_default()
                    .insert(edge.clone());
            }
        }
        if let EntryState::Ready { inputs, rank, .. } = &entry.state {
            self.ready.insert(rank.clone());
            for input in inputs {
                self.ready_by_input
                    .entry(input.clone())
                    .or_default()
                    .insert(rank.clone());
            }
        }
        if let Some(deadline) = Self::deadline_key(hash, entry) {
            self.deadlines.insert(deadline);
        }
    }

    fn detach_indexes(&mut self, hash: &Byte32, entry: &Entry) {
        self.by_short_id.remove(&entry.short_id);
        if let Some(peer) = entry.source.peer()
            && let Some(hashes) = self.by_peer.get_mut(&peer)
        {
            hashes.remove(hash);
            if hashes.is_empty() {
                self.by_peer.remove(&peer);
            }
        }
        for parent in entry
            .dependencies
            .iter()
            .map(DependencyKey::parent_hash)
            .collect::<BTreeSet<_>>()
        {
            if let Some(children) = self.by_parent.get_mut(&parent) {
                children.remove(hash);
                if children.is_empty() {
                    self.by_parent.remove(&parent);
                }
            }
        }
        if let Some(key) = entry.work_key(hash, self.limits.verify_fee_rate_ordering) {
            let lane = match entry.state {
                EntryState::ResolveQueued { lane } => Self::lane_for_resolve(lane),
                EntryState::VerifyQueued { .. } => WorkLane::Verify,
                _ => unreachable!("work key exists only for a queued state"),
            };
            self.queues[lane.index()]
                .remove(&key)
                .expect("primary queued state owns its exact queue key");
        }
        if let EntryState::Wait(wait) = &entry.state {
            let edge = WaitEdge {
                hash: hash.clone(),
                version: entry.version,
            };
            for key in wait.observed.keys() {
                if let Some(edges) = self.waiters.get_mut(key) {
                    edges.remove(&edge);
                    if edges.is_empty() {
                        self.waiters.remove(key);
                        if !self.dirty.contains_key(key) {
                            self.availability_epoch.remove(key);
                        }
                    }
                }
            }
        }
        if let EntryState::Ready { inputs, rank, .. } = &entry.state {
            self.ready.remove(rank);
            for input in inputs {
                if let Some(candidates) = self.ready_by_input.get_mut(input) {
                    candidates.remove(rank);
                    if candidates.is_empty() {
                        self.ready_by_input.remove(input);
                    }
                }
            }
        }
        if let Some(deadline) = Self::deadline_key(hash, entry) {
            self.deadlines.remove(&deadline);
        }
    }

    pub(super) fn replace_entry(&mut self, hash: &Byte32, next: Entry) -> Result<(), PrePoolError> {
        self.validate_entry_shape(hash, &next)?;
        let old = self
            .entries
            .get(hash)
            .cloned()
            .ok_or_else(|| PrePoolError::Missing(hash.clone()))?;
        self.check_usage_delta(Some(&old), Some(&next))?;
        if old.short_id != next.short_id
            && let Some(existing_hash) = self.by_short_id.get(&next.short_id)
        {
            return Err(PrePoolError::ShortIdCollision {
                short_id: next.short_id,
                existing_hash: existing_hash.clone(),
            });
        }
        self.detach_indexes(hash, &old);
        self.apply_usage_delta(Some(&old), Some(&next));
        self.entries.insert(hash.clone(), next.clone());
        self.attach_indexes(hash, &next);
        self.refresh_owner_runnable(old.source.into());
        self.refresh_owner_runnable(next.source.into());
        Ok(())
    }

    pub(super) fn remove_entry(
        &mut self,
        hash: &Byte32,
        _disposition: TerminalDisposition,
    ) -> Result<TerminalRecord, PrePoolError> {
        let entry = self
            .entries
            .remove(hash)
            .ok_or_else(|| PrePoolError::Missing(hash.clone()))?;
        self.detach_indexes(hash, &entry);
        self.apply_usage_delta(Some(&entry), None);
        self.deactivate_if_leased(entry.source, &entry.state)?;
        self.refresh_owner_runnable(entry.source.into());
        Ok(TerminalRecord {
            hash: hash.clone(),
            raw: entry.raw,
            source: entry.source,
        })
    }

    pub(super) fn validate_location(
        &self,
        hash: &Byte32,
        version: EntryVersion,
        expected: PrePoolLocation,
    ) -> Result<&Entry, PrePoolError> {
        let entry = self
            .entries
            .get(hash)
            .ok_or_else(|| PrePoolError::Missing(hash.clone()))?;
        if entry.version != version {
            return Err(PrePoolError::Stale {
                hash: hash.clone(),
                expected: version,
                actual: entry.version,
            });
        }
        let actual = entry.state.location();
        if actual != expected {
            return Err(PrePoolError::LocationMismatch {
                hash: hash.clone(),
                expected,
                actual,
            });
        }
        Ok(entry)
    }

    fn activate(&mut self, source: PrePoolSource) -> Result<(), PrePoolError> {
        if self.active_work >= self.limits.max_active_work {
            return Err(PrePoolError::ActiveWorkLimitExceeded);
        }
        let owner = source.into();
        let active = self
            .active_by_owner
            .get(&owner)
            .copied()
            .unwrap_or_default();
        if active >= self.owner_active_limit(owner) {
            return Err(match owner {
                WorkOwner::Remote(peer) => PrePoolError::PeerActiveWorkLimitExceeded(peer),
                WorkOwner::Trusted => PrePoolError::ActiveWorkLimitExceeded,
            });
        }
        self.active_work += 1;
        self.active_by_owner.insert(owner, active + 1);
        self.refresh_owner_runnable(owner);
        Ok(())
    }

    fn deactivate(&mut self, source: PrePoolSource) -> Result<(), PrePoolError> {
        let owner = source.into();
        self.active_work = self
            .active_work
            .checked_sub(1)
            .ok_or(PrePoolError::Repair("active work underflow"))?;
        let active = self
            .active_by_owner
            .get(&owner)
            .copied()
            .ok_or(PrePoolError::Repair("active owner missing"))?
            .checked_sub(1)
            .ok_or(PrePoolError::Repair("active owner underflow"))?;
        if active == 0 {
            self.active_by_owner.remove(&owner);
        } else {
            self.active_by_owner.insert(owner, active);
        }
        self.refresh_owner_runnable(owner);
        Ok(())
    }

    pub(super) fn deactivate_if_leased(
        &mut self,
        source: PrePoolSource,
        state: &EntryState,
    ) -> Result<(), PrePoolError> {
        if matches!(
            state,
            EntryState::ResolveLeased | EntryState::VerifyLeased { .. }
        ) {
            self.deactivate(source)?;
        }
        Ok(())
    }

    fn owner_active_limit(&self, owner: WorkOwner) -> usize {
        match owner {
            WorkOwner::Remote(_) => self.limits.max_active_work_per_peer,
            WorkOwner::Trusted => self.limits.max_active_work,
        }
    }

    fn transfer_active_source(
        &mut self,
        old_source: PrePoolSource,
        new_source: PrePoolSource,
        state: &EntryState,
    ) -> Result<(), PrePoolError> {
        if old_source == new_source
            || !matches!(
                state,
                EntryState::ResolveLeased | EntryState::VerifyLeased { .. }
            )
        {
            return Ok(());
        }
        let old_owner = WorkOwner::from(old_source);
        let new_owner = WorkOwner::from(new_source);
        let old_active = self
            .active_by_owner
            .get(&old_owner)
            .copied()
            .ok_or(PrePoolError::Repair("promoted active owner missing"))?;
        let new_active = self
            .active_by_owner
            .get(&new_owner)
            .copied()
            .unwrap_or_default();
        if new_active >= self.owner_active_limit(new_owner) {
            return Err(PrePoolError::Repair("promoted active owner exceeds limit"));
        }
        if old_active == 1 {
            self.active_by_owner.remove(&old_owner);
        } else {
            self.active_by_owner.insert(old_owner, old_active - 1);
        }
        self.active_by_owner.insert(new_owner, new_active + 1);
        self.refresh_owner_runnable(old_owner);
        self.refresh_owner_runnable(new_owner);
        Ok(())
    }

    fn refresh_owner_runnable(&mut self, owner: WorkOwner) {
        let active = self
            .active_by_owner
            .get(&owner)
            .copied()
            .unwrap_or_default();
        let runnable = self.active_work < self.limits.max_active_work
            && active < self.owner_active_limit(owner);
        for lane in [WorkLane::Ingress, WorkLane::Resolve, WorkLane::Verify] {
            self.queues[lane.index()].set_runnable(owner, runnable);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn admit(
        &mut self,
        hash: Byte32,
        short_id: ProposalShortId,
        raw: PipelineRawTx,
        lane: ResolveLane,
        source: PrePoolSource,
        expires_at: Option<u64>,
        raw_bytes: usize,
        dependencies: BTreeSet<DependencyKey>,
    ) -> Result<EntryVersion, PrePoolError> {
        let hash = crate::util::compact_packed(&hash);
        let short_id = crate::util::compact_packed(&short_id);
        if self.entries.contains_key(&hash) {
            return Err(PrePoolError::DuplicateHash(hash));
        }
        if let Some(existing_hash) = self.by_short_id.get(&short_id) {
            return Err(PrePoolError::ShortIdCollision {
                short_id,
                existing_hash: existing_hash.clone(),
            });
        }
        let dependencies = dependencies
            .into_iter()
            .map(DependencyKey::into_compact)
            .collect::<BTreeSet<_>>();
        let version = self.allocate_version()?;
        let arrival = self.allocate_arrival()?;
        let mut entry = Entry {
            short_id,
            raw: Arc::new(raw),
            source,
            state: EntryState::ResolveQueued { lane },
            version,
            arrival,
            expires_at,
            payload_charge_bytes: raw_bytes,
            charge_bytes: 0,
            dependencies,
        };
        entry.charge_bytes = self.entry_charge(&entry)?;
        self.validate_entry_shape(&hash, &entry)?;
        self.check_usage_delta(None, Some(&entry))?;
        self.apply_usage_delta(None, Some(&entry));
        self.entries.insert(hash.clone(), entry.clone());
        self.attach_indexes(&hash, &entry);
        Ok(version)
    }

    pub(crate) fn promote_source(&mut self, hash: &Byte32) -> Result<EntryVersion, PrePoolError> {
        let old = self
            .entries
            .get(hash)
            .cloned()
            .ok_or_else(|| PrePoolError::Missing(hash.clone()))?;
        if old.source == PrePoolSource::Proposal {
            return Ok(old.version);
        }
        let mut next = old.clone();
        next.source = PrePoolSource::Proposal;
        next.expires_at = None;
        if let EntryState::Wait(wait) = &next.state
            && wait.reason == WaitReason::Missing
        {
            next.version = self.allocate_version()?;
            next.state = EntryState::ResolveQueued {
                lane: ResolveLane::Ordered,
            };
        }
        if let EntryState::Ready { rank, .. } = &mut next.state {
            next.version = self.allocate_version()?;
            rank.source_class = PrePoolSource::Proposal.priority();
            rank.version = next.version;
        }
        next.charge_bytes = self.entry_charge(&next)?;
        self.replace_entry(hash, next.clone())?;
        self.transfer_active_source(old.source, next.source, &old.state)?;
        Ok(next.version)
    }

    pub(crate) fn replace_raw_payload(
        &mut self,
        hash: &Byte32,
        raw: PipelineRawTx,
        raw_bytes: usize,
        lane: ResolveLane,
    ) -> Result<EntryVersion, PrePoolError> {
        let old = self
            .entries
            .get(hash)
            .cloned()
            .ok_or_else(|| PrePoolError::Missing(hash.clone()))?;
        let mut next = old.clone();
        next.raw = Arc::new(raw);
        next.source = PrePoolSource::Proposal;
        next.expires_at = None;
        next.version = self.allocate_version()?;
        next.payload_charge_bytes = raw_bytes;
        next.state = EntryState::ResolveQueued { lane };
        next.charge_bytes = self.entry_charge(&next)?;
        self.replace_entry(hash, next.clone())?;
        // Commit the fully validated primary/index replacement before
        // releasing the active-work charge. A fallible replacement must not
        // leave a leased entry with no matching active reservation.
        self.deactivate_if_leased(old.source, &old.state)?;
        Ok(next.version)
    }

    pub(crate) fn checkout_resolve(
        &mut self,
        lane: ResolveLane,
    ) -> Result<Option<ResolveLease>, PrePoolError> {
        let work_lane = Self::lane_for_resolve(lane);
        let Some(key) = self.queues[work_lane.index()]
            .peek(WorkCapability::Any)
            .cloned()
        else {
            return Ok(None);
        };
        let entry = self
            .entries
            .get(&key.hash)
            .cloned()
            .ok_or_else(|| PrePoolError::Missing(key.hash.clone()))?;
        self.validate_location(&key.hash, key.version, PrePoolLocation::ResolveQueued)?;
        let version = self.allocate_version()?;
        let mut next = entry.clone();
        next.version = version;
        next.state = EntryState::ResolveLeased;
        next.charge_bytes = self.entry_charge(&next)?;
        self.check_usage_delta(Some(&entry), Some(&next))?;
        let popped = self.queues[work_lane.index()].pop(WorkCapability::Any)?;
        if popped.as_ref() != Some(&key) {
            return Err(PrePoolError::Repair(
                "resolve head changed inside kernel lock",
            ));
        }
        if let Err(error) = self.activate(entry.source) {
            self.queues[work_lane.index()].insert(key)?;
            return Err(error);
        }
        // The queue key was removed directly by pop. Replace only the primary
        // and non-queue indexes to avoid attempting a second exact removal.
        self.by_short_id.remove(&entry.short_id);
        self.detach_nonqueue_indexes(&key.hash, &entry);
        self.apply_usage_delta(Some(&entry), Some(&next));
        self.entries.insert(key.hash.clone(), next.clone());
        self.attach_nonqueue_indexes(&key.hash, &next);
        Ok(Some(ResolveLease {
            hash: key.hash,
            lane,
            version,
            payload: Arc::clone(&next.raw),
        }))
    }

    pub(crate) fn terminalize_resolve(
        &mut self,
        lease: &ResolveLease,
        disposition: TerminalDisposition,
    ) -> Result<TerminalRecord, PrePoolError> {
        self.validate_location(&lease.hash, lease.version, PrePoolLocation::ResolveLeased)?;
        self.remove_entry(&lease.hash, disposition)
    }

    pub(crate) fn terminalize_verify(
        &mut self,
        lease: &VerifyLease,
        disposition: TerminalDisposition,
    ) -> Result<TerminalRecord, PrePoolError> {
        self.validate_location(&lease.hash, lease.version, PrePoolLocation::VerifyLeased)?;
        self.remove_entry(&lease.hash, disposition)
    }

    pub(crate) fn complete_resolve(
        &mut self,
        lease: &ResolveLease,
        resolved: ResolvedTx,
        charge_bytes: usize,
        schedule: VerifySchedule,
        discovered_dependencies: BTreeSet<DependencyKey>,
    ) -> Result<EntryVersion, PrePoolError> {
        let old = self
            .validate_location(&lease.hash, lease.version, PrePoolLocation::ResolveLeased)?
            .clone();
        let mut next = old.clone();
        next.dependencies.extend(
            discovered_dependencies
                .into_iter()
                .map(DependencyKey::into_compact),
        );
        next.version = self.allocate_version()?;
        next.payload_charge_bytes = charge_bytes;
        next.state = EntryState::VerifyQueued {
            payload: Arc::new(resolved),
            schedule,
        };
        next.charge_bytes = self.entry_charge(&next)?;
        self.replace_entry(&lease.hash, next.clone())?;
        self.deactivate(old.source)?;
        Ok(next.version)
    }

    pub(crate) fn checkout_verify(
        &mut self,
        capability: WorkCapability,
    ) -> Result<Option<VerifyLease>, PrePoolError> {
        let lane = WorkLane::Verify;
        let Some(key) = self.queues[lane.index()].peek(capability).cloned() else {
            return Ok(None);
        };
        let entry = self
            .entries
            .get(&key.hash)
            .cloned()
            .ok_or_else(|| PrePoolError::Missing(key.hash.clone()))?;
        self.validate_location(&key.hash, key.version, PrePoolLocation::VerifyQueued)?;
        let version = self.allocate_version()?;
        let payload = match &entry.state {
            EntryState::VerifyQueued { payload, .. } => Arc::clone(payload),
            _ => unreachable!(),
        };
        let mut next = entry.clone();
        next.version = version;
        next.state = EntryState::VerifyLeased {
            payload: Arc::clone(&payload),
        };
        next.charge_bytes = self.entry_charge(&next)?;
        self.check_usage_delta(Some(&entry), Some(&next))?;
        let popped = self.queues[lane.index()].pop(capability)?;
        if popped.as_ref() != Some(&key) {
            return Err(PrePoolError::Repair(
                "verify head changed inside kernel lock",
            ));
        }
        if let Err(error) = self.activate(entry.source) {
            self.queues[lane.index()].insert(key)?;
            return Err(error);
        }
        self.by_short_id.remove(&entry.short_id);
        self.detach_nonqueue_indexes(&key.hash, &entry);
        self.apply_usage_delta(Some(&entry), Some(&next));
        self.entries.insert(key.hash.clone(), next.clone());
        self.attach_nonqueue_indexes(&key.hash, &next);
        Ok(Some(VerifyLease {
            hash: key.hash,
            version,
            payload,
        }))
    }

    pub(crate) fn complete_verify(
        &mut self,
        lease: &VerifyLease,
        verified: PipelineVerifiedTx,
        charge_bytes: usize,
        candidate: VerifiedCandidate,
    ) -> Result<EntryVersion, PrePoolError> {
        let old = self
            .validate_location(&lease.hash, lease.version, PrePoolLocation::VerifyLeased)?
            .clone();
        if candidate.inputs.len() > self.limits.max_inputs_per_ready {
            return Err(PrePoolError::ConflictInputLimitExceeded);
        }
        let version = self.allocate_version()?;
        let rank = ReadyKey {
            source_class: old.source.priority(),
            fee: candidate.fee,
            tx_size: candidate.tx_size,
            arrival: old.arrival,
            hash: lease.hash.clone(),
            version,
        };
        let mut next = old.clone();
        next.version = version;
        next.payload_charge_bytes = charge_bytes;
        next.state = EntryState::Ready {
            payload: Arc::new(verified),
            inputs: candidate.inputs,
            rank,
        };
        next.charge_bytes = self.entry_charge(&next)?;
        self.replace_entry(&lease.hash, next)?;
        self.deactivate(old.source)?;
        Ok(version)
    }

    fn detach_nonqueue_indexes(&mut self, hash: &Byte32, entry: &Entry) {
        self.by_short_id.remove(&entry.short_id);
        if let Some(peer) = entry.source.peer()
            && let Some(hashes) = self.by_peer.get_mut(&peer)
        {
            hashes.remove(hash);
            if hashes.is_empty() {
                self.by_peer.remove(&peer);
            }
        }
        for parent in entry
            .dependencies
            .iter()
            .map(DependencyKey::parent_hash)
            .collect::<BTreeSet<_>>()
        {
            if let Some(children) = self.by_parent.get_mut(&parent) {
                children.remove(hash);
                if children.is_empty() {
                    self.by_parent.remove(&parent);
                }
            }
        }
        if let Some(deadline) = Self::deadline_key(hash, entry) {
            self.deadlines.remove(&deadline);
        }
    }

    fn attach_nonqueue_indexes(&mut self, hash: &Byte32, entry: &Entry) {
        self.by_short_id
            .insert(entry.short_id.clone(), hash.clone());
        if let Some(peer) = entry.source.peer() {
            self.by_peer.entry(peer).or_default().insert(hash.clone());
        }
        for parent in entry
            .dependencies
            .iter()
            .map(DependencyKey::parent_hash)
            .collect::<BTreeSet<_>>()
        {
            self.by_parent
                .entry(parent)
                .or_default()
                .insert(hash.clone());
        }
        if let Some(deadline) = Self::deadline_key(hash, entry) {
            self.deadlines.insert(deadline);
        }
    }

    pub(crate) fn force_terminalize(
        &mut self,
        hash: &Byte32,
        disposition: TerminalDisposition,
    ) -> Result<Option<TerminalRecord>, PrePoolError> {
        if !self.entries.contains_key(hash) {
            return Ok(None);
        }
        self.remove_entry(hash, disposition).map(Some)
    }

    pub(crate) fn force_terminalize_many(
        &mut self,
        hashes: &[Byte32],
        disposition: TerminalDisposition,
    ) -> Result<Vec<TerminalRecord>, PrePoolError> {
        let mut unique = hashes.to_vec();
        unique.sort_unstable();
        unique.dedup();
        let present = unique
            .into_iter()
            .filter(|hash| self.entries.contains_key(hash))
            .collect::<Vec<_>>();
        present
            .into_iter()
            .map(|hash| self.remove_entry(&hash, disposition))
            .collect()
    }

    fn due_hashes(&self, now: u64, limit: usize, include_ready: bool) -> Vec<Byte32> {
        self.deadlines
            .iter()
            .take_while(|deadline| deadline.expires_at <= now)
            .filter(|deadline| {
                self.entries.get(&deadline.hash).is_some_and(|entry| {
                    entry.version == deadline.version
                        && (include_ready || !matches!(entry.state, EntryState::Ready { .. }))
                })
            })
            .take(limit)
            .map(|deadline| deadline.hash.clone())
            .collect()
    }

    pub(crate) fn due_terminal_records(
        &self,
        now: u64,
        limit: usize,
        include_ready: bool,
    ) -> Vec<TerminalRecord> {
        self.due_hashes(now, limit, include_ready)
            .iter()
            .filter_map(|hash| self.terminal_record(hash))
            .collect()
    }

    pub(crate) fn expire_due(
        &mut self,
        now: u64,
        limit: usize,
        include_ready: bool,
    ) -> Result<Vec<TerminalRecord>, PrePoolError> {
        self.due_hashes(now, limit, include_ready)
            .into_iter()
            .map(|hash| self.remove_entry(&hash, TerminalDisposition::Expired))
            .collect()
    }

    pub(crate) fn clear_terminal_records(&self) -> Vec<TerminalRecord> {
        self.entries
            .keys()
            .filter_map(|hash| self.terminal_record(hash))
            .collect()
    }

    pub(crate) fn clear(&mut self) -> Result<Vec<TerminalRecord>, PrePoolError> {
        let hashes = self.entries.keys().cloned().collect::<Vec<_>>();
        hashes
            .into_iter()
            .map(|hash| self.remove_entry(&hash, TerminalDisposition::Cleared))
            .collect()
    }

    pub(crate) fn work_is_ready(
        &self,
        lane: WorkLane,
        capability: WorkCapability,
    ) -> Result<bool, PrePoolError> {
        Ok(match lane {
            WorkLane::Commit => !self.ready.is_empty(),
            _ => self.queues[lane.index()].peek(capability).is_some(),
        })
    }
}
