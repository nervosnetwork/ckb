use super::*;

struct ActivePlan {
    total: usize,
    owner_updates: [Option<(WorkOwner, usize)>; 2],
}

/// A bounded multi-entry state change compiled entirely from the current
/// primary map. Every identity, shape, fan-out and budget predicate is checked
/// before Apply; Apply only detaches old projections, installs the exact final
/// counters, and attaches the planned primaries.
pub(super) struct CohortPlan {
    changes: Vec<EntryChange>,
    total_usage: Residency,
    remote_usage: Residency,
    conflict_usage: Residency,
    peer_updates: HashMap<PeerIndex, Residency>,
    active_work: usize,
    owner_updates: BTreeMap<WorkOwner, usize>,
    affected_owners: BTreeSet<WorkOwner>,
    next_version: EntryVersion,
    next_arrival: u128,
}

struct EntryChange {
    hash: Byte32,
    old: Option<Entry>,
    next: Option<Entry>,
}

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

    fn validate_entry_intrinsic(&self, hash: &Byte32, entry: &Entry) -> Result<(), PrePoolError> {
        if entry.raw.tx.hash() != *hash || entry.raw.tx.proposal_short_id() != entry.short_id {
            return Err(PrePoolError::Repair(
                "primary identity differs from retained transaction",
            ));
        }
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
        if let EntryState::Wait(wait) = &entry.state
            && wait.observed.is_empty()
        {
            return Err(PrePoolError::Repair("wait owner has no dependency key"));
        }
        if let EntryState::Wait(wait) = &entry.state
            && wait.observed.len() > self.limits.max_dependencies_per_entry
        {
            return Err(PrePoolError::DependencyLimitExceeded);
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
        if let EntryState::Ready { inputs, rank, .. } = &entry.state {
            if inputs.is_empty() || inputs.len() > self.limits.max_inputs_per_ready {
                return Err(PrePoolError::ConflictInputLimitExceeded);
            }
            if rank.hash != *hash
                || rank.version != entry.version
                || rank.arrival != entry.arrival
                || rank.source_class != entry.source.priority()
            {
                return Err(PrePoolError::Repair(
                    "ready rank differs from its primary owner",
                ));
            }
        }
        if entry.charge_bytes != self.entry_charge(entry)? {
            return Err(PrePoolError::Repair(
                "entry charge is not derived from its primary state",
            ));
        }
        Ok(())
    }

    pub(super) fn validate_entry_shape(
        &self,
        hash: &Byte32,
        entry: &Entry,
    ) -> Result<(), PrePoolError> {
        self.validate_entry_intrinsic(hash, entry)?;
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
        if let EntryState::Ready { inputs, .. } = &entry.state {
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
        Ok(())
    }

    /// Compile a bounded cohort against exact current primary ownership.
    /// `desired` contains one final primary per changed hash (`None` removes
    /// it). Clocks are supplied by the caller's local planning cursors and are
    /// installed only after a successful total Apply.
    pub(super) fn plan_cohort(
        &self,
        desired: Vec<(Byte32, Option<Entry>)>,
        next_version: EntryVersion,
        next_arrival: u128,
    ) -> Result<CohortPlan, PrePoolError> {
        let mut seen = HashSet::with_capacity(desired.len());
        let mut changes = Vec::with_capacity(desired.len());
        for (hash, next) in desired {
            if !seen.insert(hash.clone()) {
                return Err(PrePoolError::Repair("duplicate hash in cohort plan"));
            }
            if let Some(next) = &next {
                self.validate_entry_intrinsic(&hash, next)?;
            }
            let old = self.entries.get(&hash).cloned();
            if old.is_none() && next.is_none() {
                continue;
            }
            changes.push(EntryChange { hash, old, next });
        }

        let changed_hashes = changes
            .iter()
            .map(|change| change.hash.clone())
            .collect::<HashSet<_>>();
        let mut final_short_ids = HashMap::<ProposalShortId, Byte32>::new();
        for change in &changes {
            if let Some(old) = &change.old
                && self.by_short_id.get(&old.short_id) != Some(&change.hash)
            {
                return Err(PrePoolError::Repair("short-id projection drift"));
            }
            let Some(next) = &change.next else {
                continue;
            };
            if let Some(existing_hash) =
                final_short_ids.insert(next.short_id.clone(), change.hash.clone())
                && existing_hash != change.hash
            {
                return Err(PrePoolError::ShortIdCollision {
                    short_id: next.short_id.clone(),
                    existing_hash,
                });
            }
            if let Some(existing_hash) = self.by_short_id.get(&next.short_id)
                && existing_hash != &change.hash
                && !changed_hashes.contains(existing_hash)
            {
                return Err(PrePoolError::ShortIdCollision {
                    short_id: next.short_id.clone(),
                    existing_hash: existing_hash.clone(),
                });
            }
        }

        let mut parent_counts = HashMap::<Byte32, usize>::new();
        let mut input_counts = HashMap::<OutPoint, usize>::new();
        for change in &changes {
            for parent in change
                .old
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
                    .ok_or(PrePoolError::Repair("parent projection underflow"))?;
            }
            if let Some(EntryState::Ready { inputs, .. }) =
                change.old.as_ref().map(|entry| &entry.state)
            {
                for input in inputs {
                    let count = input_counts
                        .entry(input.clone())
                        .or_insert_with(|| self.ready_by_input.get(input).map_or(0, BTreeSet::len));
                    *count = count
                        .checked_sub(1)
                        .ok_or(PrePoolError::Repair("ready-input projection underflow"))?;
                }
            }
        }
        for change in &changes {
            let Some(next) = &change.next else {
                continue;
            };
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
            if let EntryState::Ready { inputs, .. } = &next.state {
                for input in inputs {
                    let count = input_counts
                        .entry(input.clone())
                        .or_insert_with(|| self.ready_by_input.get(input).map_or(0, BTreeSet::len));
                    *count = count
                        .checked_add(1)
                        .ok_or(PrePoolError::ResidencyChargeOverflow)?;
                    if *count > self.limits.max_candidates_per_input {
                        return Err(PrePoolError::ConflictCandidateLimitExceeded(input.clone()));
                    }
                }
            }
        }

        let mut total_usage = self.total_usage;
        let mut remote_usage = self.remote_usage;
        let mut conflict_usage = self.conflict_usage;
        let mut peer_updates = HashMap::<PeerIndex, Residency>::new();
        let mut owner_updates = BTreeMap::<WorkOwner, usize>::new();
        let mut affected_owners = BTreeSet::new();
        let mut active_work = self.active_work;
        for change in &changes {
            if let Some(old) = &change.old {
                let charge = Residency::new(1, old.charge_bytes);
                total_usage = total_usage
                    .checked_sub(charge)
                    .ok_or(PrePoolError::Repair("cohort total usage underflow"))?;
                if let Some(peer) = old.source.peer() {
                    remote_usage = remote_usage
                        .checked_sub(charge)
                        .ok_or(PrePoolError::Repair("cohort remote usage underflow"))?;
                    let usage = peer_updates
                        .entry(peer)
                        .or_insert_with(|| self.peer_usage.get(&peer).copied().unwrap_or_default());
                    *usage = usage
                        .checked_sub(charge)
                        .ok_or(PrePoolError::Repair("cohort peer usage underflow"))?;
                }
                if Self::is_conflict(old) {
                    conflict_usage = conflict_usage
                        .checked_sub(charge)
                        .ok_or(PrePoolError::Repair("cohort conflict usage underflow"))?;
                }
                if let Some(owner) = Self::active_owner(old.source, &old.state) {
                    active_work = active_work
                        .checked_sub(1)
                        .ok_or(PrePoolError::Repair("cohort active-work underflow"))?;
                    let active = owner_updates.entry(owner).or_insert_with(|| {
                        self.active_by_owner
                            .get(&owner)
                            .copied()
                            .unwrap_or_default()
                    });
                    *active = active
                        .checked_sub(1)
                        .ok_or(PrePoolError::Repair("cohort active-owner underflow"))?;
                }
                affected_owners.insert(old.source.into());
            }
            if let Some(next) = &change.next {
                let charge = Residency::new(1, next.charge_bytes);
                total_usage = total_usage
                    .checked_add(charge)
                    .ok_or(PrePoolError::ResidencyChargeOverflow)?;
                if let Some(peer) = next.source.peer() {
                    remote_usage = remote_usage
                        .checked_add(charge)
                        .ok_or(PrePoolError::RemoteBudgetExceeded)?;
                    let usage = peer_updates
                        .entry(peer)
                        .or_insert_with(|| self.peer_usage.get(&peer).copied().unwrap_or_default());
                    *usage = usage
                        .checked_add(charge)
                        .ok_or(PrePoolError::PeerBudgetExceeded(peer))?;
                }
                if Self::is_conflict(next) {
                    conflict_usage = conflict_usage
                        .checked_add(charge)
                        .ok_or(PrePoolError::ConflictHistoryBudgetExceeded)?;
                }
                if let Some(owner) = Self::active_owner(next.source, &next.state) {
                    active_work = active_work
                        .checked_add(1)
                        .ok_or(PrePoolError::ActiveWorkLimitExceeded)?;
                    let active = owner_updates.entry(owner).or_insert_with(|| {
                        self.active_by_owner
                            .get(&owner)
                            .copied()
                            .unwrap_or_default()
                    });
                    *active = active
                        .checked_add(1)
                        .ok_or_else(|| Self::active_limit_error(owner))?;
                }
                affected_owners.insert(next.source.into());
            }
        }
        if !total_usage.fits(self.limits.total) {
            return Err(PrePoolError::TotalBudgetExceeded);
        }
        if !remote_usage.fits(self.limits.remote) {
            return Err(PrePoolError::RemoteBudgetExceeded);
        }
        if !conflict_usage.fits(self.limits.conflict_history) {
            return Err(PrePoolError::ConflictHistoryBudgetExceeded);
        }
        if active_work > self.limits.max_active_work {
            return Err(PrePoolError::ActiveWorkLimitExceeded);
        }

        for (peer, usage) in &peer_updates {
            if !usage.fits(self.limits.per_peer) {
                return Err(PrePoolError::PeerBudgetExceeded(*peer));
            }
        }
        for (owner, active) in &owner_updates {
            if *active > self.owner_active_limit(*owner) {
                return Err(Self::active_limit_error(*owner));
            }
        }

        Ok(CohortPlan {
            changes,
            total_usage,
            remote_usage,
            conflict_usage,
            peer_updates,
            active_work,
            owner_updates,
            affected_owners,
            next_version,
            next_arrival,
        })
    }

    /// Total half of [`Self::plan_cohort`]. Any failure here is an internal
    /// invariant violation; no transaction-shaped input is interpreted after
    /// the first projection mutation.
    pub(super) fn apply_cohort(&mut self, plan: CohortPlan) {
        self.total_usage = plan.total_usage;
        self.remote_usage = plan.remote_usage;
        self.conflict_usage = plan.conflict_usage;
        self.active_work = plan.active_work;
        for (peer, usage) in plan.peer_updates {
            if usage == Residency::default() {
                self.peer_usage.remove(&peer);
            } else {
                self.peer_usage.insert(peer, usage);
            }
        }
        for (owner, active) in plan.owner_updates {
            if active == 0 {
                self.active_by_owner.remove(&owner);
            } else {
                self.active_by_owner.insert(owner, active);
            }
        }

        for change in &plan.changes {
            if let Some(old) = &change.old {
                self.detach_indexes(&change.hash, old);
                let removed = self.entries.remove(&change.hash);
                assert!(removed.is_some(), "cohort old primary was prevalidated");
            }
        }
        for change in &plan.changes {
            if let Some(next) = &change.next {
                let previous = self.entries.insert(change.hash.clone(), next.clone());
                assert!(previous.is_none(), "cohort hash was detached before attach");
                self.attach_indexes(&change.hash, next);
            }
        }
        self.next_version = plan.next_version;
        self.next_arrival = plan.next_arrival;
        for owner in plan.affected_owners {
            self.refresh_owner_runnable(owner);
        }
    }

    pub(super) fn deadline_key(hash: &Byte32, entry: &Entry) -> Option<DeadlineKey> {
        entry.expires_at.map(|expires_at| DeadlineKey {
            expires_at,
            hash: hash.clone(),
            version: entry.version,
        })
    }

    pub(super) fn attach_indexes(&mut self, hash: &Byte32, entry: &Entry) {
        self.attach_common_indexes(hash, entry);
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
                    .insert(edge.clone())
                    .then_some(())
                    .expect("wait edge is uniquely derived from primary state");
            }
        }
        if let EntryState::Ready { inputs, rank, .. } = &entry.state {
            assert!(
                self.ready.insert(rank.clone()),
                "ready rank is uniquely derived from primary state"
            );
            for input in inputs {
                assert!(
                    self.ready_by_input
                        .entry(input.clone())
                        .or_default()
                        .insert(rank.clone()),
                    "ready input rank is uniquely derived from primary state"
                );
            }
        }
    }

    fn attach_common_indexes(&mut self, hash: &Byte32, entry: &Entry) {
        assert!(
            self.by_short_id
                .insert(entry.short_id.clone(), hash.clone())
                .is_none(),
            "short-id slot was prevalidated vacant"
        );
        if let Some(peer) = entry.source.peer() {
            assert!(
                self.by_peer.entry(peer).or_default().insert(hash.clone()),
                "peer projection is uniquely derived from primary state"
            );
        }
        for parent in entry
            .dependencies
            .iter()
            .map(DependencyKey::parent_hash)
            .collect::<BTreeSet<_>>()
        {
            assert!(
                self.by_parent
                    .entry(parent)
                    .or_default()
                    .insert(hash.clone()),
                "parent projection is uniquely derived from primary state"
            );
        }
        if let Some(deadline) = Self::deadline_key(hash, entry) {
            assert!(
                self.deadlines.insert(deadline),
                "deadline is uniquely derived from primary state"
            );
        }
    }

    pub(super) fn detach_indexes(&mut self, hash: &Byte32, entry: &Entry) {
        self.detach_common_indexes(hash, entry);
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
                let edges = self
                    .waiters
                    .get_mut(key)
                    .expect("wait primary owns a dependency bucket");
                assert!(
                    edges.remove(&edge),
                    "wait primary owns its exact dependency edge"
                );
                if edges.is_empty() {
                    self.waiters.remove(key);
                    if !self.dirty.contains_key(key) {
                        self.availability_epoch.remove(key);
                    }
                }
            }
        }
        if let EntryState::Ready { inputs, rank, .. } = &entry.state {
            assert!(self.ready.remove(rank), "ready primary owns its exact rank");
            for input in inputs {
                let candidates = self
                    .ready_by_input
                    .get_mut(input)
                    .expect("ready primary owns an input bucket");
                assert!(
                    candidates.remove(rank),
                    "ready primary owns its exact input rank"
                );
                if candidates.is_empty() {
                    self.ready_by_input.remove(input);
                }
            }
        }
    }

    fn detach_common_indexes(&mut self, hash: &Byte32, entry: &Entry) {
        assert_eq!(
            self.by_short_id.remove(&entry.short_id).as_ref(),
            Some(hash),
            "primary owns its exact short-id slot"
        );
        if let Some(peer) = entry.source.peer() {
            let hashes = self
                .by_peer
                .get_mut(&peer)
                .expect("remote primary owns a peer bucket");
            assert!(
                hashes.remove(hash),
                "primary owns its exact peer projection"
            );
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
            let children = self
                .by_parent
                .get_mut(&parent)
                .expect("dependent primary owns a parent bucket");
            assert!(
                children.remove(hash),
                "primary owns its exact parent projection"
            );
            if children.is_empty() {
                self.by_parent.remove(&parent);
            }
        }
        if let Some(deadline) = Self::deadline_key(hash, entry) {
            assert!(
                self.deadlines.remove(&deadline),
                "primary owns its exact deadline"
            );
        }
    }

    pub(super) fn replace_entry(&mut self, hash: &Byte32, next: Entry) -> Result<(), PrePoolError> {
        self.validate_entry_shape(hash, &next)?;
        let old = self
            .entries
            .get(hash)
            .cloned()
            .ok_or_else(|| PrePoolError::Missing(hash.clone()))?;
        let usage_plan = self.plan_usage_delta(Some(&old), Some(&next))?;
        let old_active = Self::active_owner(old.source, &old.state);
        let next_active = Self::active_owner(next.source, &next.state);
        let active_plan = self.plan_active_transition(old_active, next_active)?;
        if old.short_id != next.short_id
            && let Some(existing_hash) = self.by_short_id.get(&next.short_id)
        {
            return Err(PrePoolError::ShortIdCollision {
                short_id: next.short_id,
                existing_hash: existing_hash.clone(),
            });
        }
        self.detach_indexes(hash, &old);
        self.apply_usage_plan(usage_plan);
        self.entries.insert(hash.clone(), next.clone());
        self.attach_indexes(hash, &next);
        self.apply_active_plan(active_plan);
        self.refresh_owner_runnable(old.source.into());
        self.refresh_owner_runnable(next.source.into());
        Ok(())
    }

    pub(super) fn remove_entry(&mut self, hash: &Byte32) -> Result<TerminalRecord, PrePoolError> {
        let entry = self
            .entries
            .get(hash)
            .cloned()
            .ok_or_else(|| PrePoolError::Missing(hash.clone()))?;
        let old_active = Self::active_owner(entry.source, &entry.state);
        let active_plan = self.plan_active_transition(old_active, None)?;
        let usage_plan = self.plan_usage_delta(Some(&entry), None)?;
        self.entries
            .remove(hash)
            .expect("remove prevalidated primary entry");
        self.detach_indexes(hash, &entry);
        self.apply_usage_plan(usage_plan);
        self.apply_active_plan(active_plan);
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

    pub(super) fn active_owner(source: PrePoolSource, state: &EntryState) -> Option<WorkOwner> {
        if matches!(
            state,
            EntryState::ResolveLeased | EntryState::VerifyLeased { .. }
        ) {
            Some(source.into())
        } else {
            None
        }
    }

    fn owner_active_limit(&self, owner: WorkOwner) -> usize {
        match owner {
            WorkOwner::Remote(_) => self.limits.max_active_work_per_peer,
            WorkOwner::Trusted => self.limits.max_active_work,
        }
    }

    fn active_limit_error(owner: WorkOwner) -> PrePoolError {
        match owner {
            WorkOwner::Remote(peer) => PrePoolError::PeerActiveWorkLimitExceeded(peer),
            WorkOwner::Trusted => PrePoolError::ActiveWorkLimitExceeded,
        }
    }

    fn plan_active_transition(
        &self,
        old: Option<WorkOwner>,
        new: Option<WorkOwner>,
    ) -> Result<ActivePlan, PrePoolError> {
        let active_work = self
            .active_work
            .checked_sub(usize::from(old.is_some()))
            .and_then(|value| value.checked_add(usize::from(new.is_some())))
            .ok_or(PrePoolError::Repair("active work projection arithmetic"))?;
        if active_work > self.limits.max_active_work {
            return Err(PrePoolError::ActiveWorkLimitExceeded);
        }
        let project_owner = |owner| {
            let active = self
                .active_by_owner
                .get(&owner)
                .copied()
                .unwrap_or_default()
                .checked_sub(usize::from(old == Some(owner)))
                .and_then(|value| value.checked_add(usize::from(new == Some(owner))))
                .ok_or(PrePoolError::Repair("active owner projection arithmetic"))?;
            if active > self.owner_active_limit(owner) {
                return Err(Self::active_limit_error(owner));
            }
            Ok(active)
        };
        let old_update = old
            .map(|owner| project_owner(owner).map(|active| (owner, active)))
            .transpose()?;
        let new_update = new
            .filter(|owner| Some(*owner) != old)
            .map(|owner| project_owner(owner).map(|active| (owner, active)))
            .transpose()?;
        Ok(ActivePlan {
            total: active_work,
            owner_updates: [old_update, new_update],
        })
    }

    fn apply_active_plan(&mut self, plan: ActivePlan) {
        self.active_work = plan.total;
        for (owner, active) in plan.owner_updates.into_iter().flatten() {
            if active == 0 {
                self.active_by_owner.remove(&owner);
            } else {
                self.active_by_owner.insert(owner, active);
            }
            self.refresh_owner_runnable(owner);
        }
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
        let usage_plan = self.plan_usage_delta(None, Some(&entry))?;
        self.apply_usage_plan(usage_plan);
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
        let usage_plan = self.plan_usage_delta(Some(&entry), Some(&next))?;
        let old_active = Self::active_owner(entry.source, &entry.state);
        let next_active = Self::active_owner(next.source, &next.state);
        let active_plan = self.plan_active_transition(old_active, next_active)?;
        let popped = self.queues[work_lane.index()].pop(WorkCapability::Any)?;
        if popped.as_ref() != Some(&key) {
            return Err(PrePoolError::Repair(
                "resolve head changed inside kernel lock",
            ));
        }
        // The queue key was removed directly by pop. Replace only the primary
        // and non-queue indexes to avoid attempting a second exact removal.
        self.replace_after_queue_pop(&key.hash, &entry, &next, usage_plan, active_plan);
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
    ) -> Result<TerminalRecord, PrePoolError> {
        self.validate_location(&lease.hash, lease.version, PrePoolLocation::ResolveLeased)?;
        self.remove_entry(&lease.hash)
    }

    pub(crate) fn terminalize_verify(
        &mut self,
        lease: &VerifyLease,
    ) -> Result<TerminalRecord, PrePoolError> {
        self.validate_location(&lease.hash, lease.version, PrePoolLocation::VerifyLeased)?;
        self.remove_entry(&lease.hash)
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
        let usage_plan = self.plan_usage_delta(Some(&entry), Some(&next))?;
        let old_active = Self::active_owner(entry.source, &entry.state);
        let next_active = Self::active_owner(next.source, &next.state);
        let active_plan = self.plan_active_transition(old_active, next_active)?;
        let popped = self.queues[lane.index()].pop(capability)?;
        if popped.as_ref() != Some(&key) {
            return Err(PrePoolError::Repair(
                "verify head changed inside kernel lock",
            ));
        }
        self.replace_after_queue_pop(&key.hash, &entry, &next, usage_plan, active_plan);
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
        Ok(version)
    }

    fn replace_after_queue_pop(
        &mut self,
        hash: &Byte32,
        old: &Entry,
        next: &Entry,
        usage_plan: UsagePlan,
        active_plan: ActivePlan,
    ) {
        self.detach_common_indexes(hash, old);
        self.apply_usage_plan(usage_plan);
        self.entries.insert(hash.clone(), next.clone());
        self.attach_common_indexes(hash, next);
        self.apply_active_plan(active_plan);
    }

    pub(crate) fn force_terminalize(
        &mut self,
        hash: &Byte32,
    ) -> Result<Option<TerminalRecord>, PrePoolError> {
        if !self.entries.contains_key(hash) {
            return Ok(None);
        }
        self.remove_entry(hash).map(Some)
    }

    pub(crate) fn force_terminalize_many(
        &mut self,
        hashes: &[Byte32],
    ) -> Result<Vec<TerminalRecord>, PrePoolError> {
        let mut unique = hashes.to_vec();
        unique.sort_unstable();
        unique.dedup();
        let present = unique
            .into_iter()
            .filter(|hash| self.entries.contains_key(hash))
            .collect::<Vec<_>>();
        let records = present
            .iter()
            .map(|hash| {
                self.terminal_record(hash)
                    .expect("present cohort member has a terminal record")
            })
            .collect();
        let desired = present.into_iter().map(|hash| (hash, None)).collect();
        let plan = self.plan_cohort(desired, self.next_version, self.next_arrival)?;
        self.apply_cohort(plan);
        Ok(records)
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
        let hashes = self.due_hashes(now, limit, include_ready);
        let records = hashes
            .iter()
            .map(|hash| {
                self.terminal_record(hash)
                    .expect("due cohort member has a terminal record")
            })
            .collect();
        let desired = hashes.into_iter().map(|hash| (hash, None)).collect();
        let plan = self.plan_cohort(desired, self.next_version, self.next_arrival)?;
        self.apply_cohort(plan);
        Ok(records)
    }

    pub(crate) fn work_is_ready(&self, lane: WorkLane, capability: WorkCapability) -> bool {
        match lane {
            WorkLane::Commit => !self.ready.is_empty(),
            _ => self.queues[lane.index()].peek(capability).is_some(),
        }
    }
}
