use super::*;

struct ActivePlan {
    total: usize,
    owner_updates: [Option<(WorkOwner, usize)>; 2],
}

#[derive(Clone, Copy)]
enum WaiterEdgeDelta {
    Detach,
    Attach,
}

impl WaiterEdgeDelta {
    fn apply(self, count: usize) -> Result<usize, PrePoolError> {
        match self {
            Self::Detach => count
                .checked_sub(1)
                .ok_or(PrePoolError::ProjectionInconsistent(
                    "waiter projection omits a cohort primary edge",
                )),
            Self::Attach => count.checked_add(1).ok_or(PrePoolError::CounterExhausted),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum ReplacementMode {
    Ordinary,
    Checkout(WorkCapability),
}

struct EntryReplacementPlan {
    usage: UsagePlan,
    active: ActivePlan,
    queue_lengths: WorkLaneSlots<usize>,
    checkout: Option<(WorkLane, WorkKey, u128)>,
    next_revision: EntryRevision,
    next_arrival: Arrival,
}

/// Proof-carrying terminal cohort. Effects are built from `records` and the
/// journal closure can only consume the already-validated, total mutation.
pub(crate) struct PreparedTerminalCohort<'authority> {
    prepared: PreparedKernelMutation<'authority>,
    records: Vec<TerminalRecord>,
}

impl PreparedTerminalCohort<'_> {
    pub(crate) fn records(&self) -> &[TerminalRecord] {
        &self.records
    }

    pub(crate) fn apply(self) -> Vec<TerminalRecord> {
        self.prepared.apply();
        self.records
    }
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
    queue_lengths: WorkLaneSlots<usize>,
    affected_owners: BTreeSet<WorkOwner>,
    next_revision: EntryRevision,
    next_arrival: Arrival,
}

impl CohortPlan {
    fn apply_waiter_edges(
        counts: &mut BTreeMap<DependencyKey, usize>,
        observed: &ObservedDependencies,
        delta: WaiterEdgeDelta,
    ) -> Result<(), PrePoolError> {
        if observed.len() <= counts.len() {
            for key in observed.keys() {
                if let Some(count) = counts.get_mut(key) {
                    *count = delta.apply(*count)?;
                }
            }
        } else {
            for (key, count) in counts {
                if observed.contains_key(key) {
                    *count = delta.apply(*count)?;
                }
            }
        }
        Ok(())
    }

    /// Apply this cohort's waiter-edge delta to the requested dependency
    /// counts in one pass over the changed primaries.
    ///
    /// Computing one key at a time would rescan the complete cohort for every
    /// changed dependency. A hostile multi-parent fan-out could therefore
    /// turn one bounded transition into the Cartesian product of dependency
    /// keys and cohort members. Keeping the reduction on the immutable plan
    /// preserves the same proof boundary without adding a resident index.
    pub(super) fn apply_waiter_count_delta(
        &self,
        counts: &mut BTreeMap<DependencyKey, usize>,
    ) -> Result<(), PrePoolError> {
        for change in &self.changes {
            if let Some(EntryState::Wait(wait)) = change.old.as_ref().map(|entry| &entry.state) {
                Self::apply_waiter_edges(counts, &wait.observed, WaiterEdgeDelta::Detach)?;
            }
            if let Some(EntryState::Wait(wait)) = change.next.as_ref().map(|entry| &entry.state) {
                Self::apply_waiter_edges(counts, &wait.observed, WaiterEdgeDelta::Attach)?;
            }
        }
        Ok(())
    }

    /// Bind conflict history created by this cohort to the post-Apply
    /// dependency level. An unchanged historical waiter must observe the
    /// level change and retry, but a victim retained by the same accepted
    /// mutation must not interpret that mutation as a later release and
    /// immediately resurrect itself.
    ///
    /// `Wait(Missing)` is deliberately excluded: definitive parent loss uses
    /// the same level change to schedule bounded re-resolution of consumers
    /// invalidated by this cohort.
    fn bind_conflict_observation_cut(&mut self, changes: &DependencyChangePlan) {
        for change in &mut self.changes {
            if let Some(next) = change.next.take() {
                change.next = Some(next.bind_conflict_observation_cut(changes));
            }
        }
    }
}

/// One final primary action per hash. Stored-entry keys are always derived
/// from the checked entry itself; callers cannot pair a payload with a
/// different map key or submit duplicate final actions in a vector.
#[derive(Clone, Default)]
pub(super) struct MutationSet(BTreeMap<Byte32, Option<StoredEntry>>);

impl MutationSet {
    pub(super) fn set_entry(&mut self, entry: StoredEntry) {
        self.0.insert(entry.hash().clone(), Some(entry));
    }

    pub(super) fn try_add_entry(&mut self, entry: StoredEntry) -> Result<(), PrePoolError> {
        let hash = entry.hash().clone();
        if self.0.contains_key(&hash) {
            return Err(PrePoolError::DuplicateHash(hash));
        }
        self.0.insert(hash, Some(entry));
        Ok(())
    }

    pub(super) fn set_remove(&mut self, hash: Byte32) {
        self.0.insert(hash, None);
    }

    pub(super) fn take_entry(&mut self, hash: &Byte32) -> Option<StoredEntry> {
        self.0.remove(hash).flatten()
    }

    fn into_iter(self) -> impl Iterator<Item = (Byte32, Option<StoredEntry>)> {
        self.0.into_iter()
    }
}

struct EntryChange {
    hash: Byte32,
    old: Option<StoredEntry>,
    next: Option<StoredEntry>,
}

/// One exclusive, proof-carrying kernel transaction. The mutable borrow makes
/// it impossible to mutate the authority between Plan and Apply; private
/// fields make a partially validated delta impossible to construct outside
/// this module.
pub(crate) struct PreparedKernelMutation<'authority> {
    authority: &'authority mut PrePoolKernel,
    cohort: CohortPlan,
    dependency_changes: DependencyChangePlan,
}

impl PreparedKernelMutation<'_> {
    pub(crate) fn apply(self) {
        self.authority.apply_cohort(self.cohort);
        self.authority
            .apply_dependency_change_plan(self.dependency_changes);
    }
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

    fn validate_entry_intrinsic(
        &self,
        hash: &Byte32,
        entry: &StoredEntry,
    ) -> Result<(), PrePoolError> {
        if entry.hash() != hash {
            return Err(PrePoolError::primary_key_mismatch(
                hash.clone(),
                entry.hash().clone(),
            ));
        }
        Ok(())
    }

    pub(super) fn validate_entry_shape(
        &self,
        hash: &Byte32,
        entry: &StoredEntry,
    ) -> Result<(), PrePoolError> {
        self.validate_entry_intrinsic(hash, entry)?;
        for parent in entry.parent_hashes() {
            let existing = self
                .by_parent
                .get(&parent)
                .map_or(0, |children| children.len());
            let already = self.entries.get(hash).is_some_and(|old| {
                old.dependencies
                    .iter()
                    .any(|key| key.parent_hash() == parent)
            });
            if !already && existing >= self.limits.max_dependents_per_parent {
                return Err(PrePoolError::ParentFanoutLimitExceeded(parent));
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

    fn validate_entry_projection(
        &self,
        hash: &Byte32,
        entry: &StoredEntry,
    ) -> Result<(), PrePoolError> {
        self.validate_entry_intrinsic(hash, entry)?;
        if self.by_short_id.get(&entry.short_id()) != Some(hash) {
            return Err(PrePoolError::ProjectionInconsistent(
                "short-id projection omits its primary",
            ));
        }
        if let Some(peer) = entry.raw.ingress_peer()
            && !self
                .by_ingress_peer
                .get(&peer)
                .is_some_and(|hashes| hashes.contains(hash))
        {
            return Err(PrePoolError::ProjectionInconsistent(
                "ingress-peer projection omits its primary",
            ));
        }
        for parent in entry.parent_hashes() {
            if !self
                .by_parent
                .get(&parent)
                .is_some_and(|children| children.contains(hash))
            {
                return Err(PrePoolError::ProjectionInconsistent(
                    "parent projection omits its primary edge",
                ));
            }
        }
        if let Some(deadline) = Self::deadline_key(hash, entry)
            && !self.deadlines.contains(&deadline)
        {
            return Err(PrePoolError::ProjectionInconsistent(
                "deadline projection omits its primary",
            ));
        }
        if let Some((lane, key)) = entry.queued_work(hash, self.limits.verify_fee_rate_ordering)
            && !self.queues.get(lane).contains(&key)
        {
            return Err(PrePoolError::ProjectionInconsistent(
                "work queue omits its primary key",
            ));
        }
        if let EntryState::Wait(wait) = &entry.state {
            let edge = WaitEdge {
                hash: hash.clone(),
                revision: entry.revision,
            };
            for key in wait.observed.keys() {
                if !self
                    .waiters
                    .get(key)
                    .is_some_and(|edges| edges.contains(&edge))
                {
                    return Err(PrePoolError::ProjectionInconsistent(
                        "waiter projection omits its primary edge",
                    ));
                }
            }
        }
        if let EntryState::Ready { payload, inputs } = &entry.state {
            let rank = entry.ready_key_for(hash, payload);
            if !self.ready.contains(&rank) {
                return Err(PrePoolError::ProjectionInconsistent(
                    "Ready projection omits its primary rank",
                ));
            }
            for input in inputs {
                if !self
                    .ready_by_input
                    .get(input)
                    .is_some_and(|candidates| candidates.contains(&rank))
                {
                    return Err(PrePoolError::ProjectionInconsistent(
                        "Ready input projection omits its primary rank",
                    ));
                }
            }
        }
        Ok(())
    }

    pub(super) fn apply_queue_transition(
        &self,
        lengths: &mut WorkLaneSlots<usize>,
        old: Option<&StoredEntry>,
        new: Option<&StoredEntry>,
    ) -> Result<(), PrePoolError> {
        if let Some(old) = old
            && let Some((lane, _)) =
                old.queued_work(old.hash(), self.limits.verify_fee_rate_ordering)
        {
            *lengths.get_mut(lane) =
                lengths
                    .get(lane)
                    .checked_sub(1)
                    .ok_or(PrePoolError::ProjectionInconsistent(
                        "queue length omits a primary work key",
                    ))?;
        }
        if let Some(new) = new
            && let Some((lane, _)) =
                new.queued_work(new.hash(), self.limits.verify_fee_rate_ordering)
        {
            *lengths.get_mut(lane) = lengths
                .get(lane)
                .checked_add(1)
                .ok_or(PrePoolError::CounterExhausted)?;
        }
        Ok(())
    }

    /// Compile a bounded cohort against exact current primary ownership.
    /// `desired` contains one final primary per changed hash (`None` removes
    /// it). Clocks are supplied by the caller's local planning cursors and are
    /// installed only after a successful total Apply.
    pub(super) fn compile_cohort(
        &self,
        desired: MutationSet,
        next_revision: EntryRevision,
        next_arrival: Arrival,
    ) -> Result<CohortPlan, PrePoolError> {
        let mut changes = Vec::with_capacity(desired.0.len());
        for (hash, next) in desired.into_iter() {
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
            if let Some(old) = &change.old {
                self.validate_entry_projection(&change.hash, old)?;
            }
            let Some(next) = &change.next else {
                continue;
            };
            if let Some(existing_hash) =
                final_short_ids.insert(next.short_id(), change.hash.clone())
                && existing_hash != change.hash
            {
                return Err(PrePoolError::ShortIdCollision(
                    next.short_id(),
                    existing_hash,
                ));
            }
            if let Some(existing_hash) = self.by_short_id.get(&next.short_id())
                && existing_hash != &change.hash
                && !changed_hashes.contains(existing_hash)
            {
                return Err(PrePoolError::ShortIdCollision(
                    next.short_id(),
                    existing_hash.clone(),
                ));
            }
        }

        let mut parent_counts = HashMap::<Byte32, usize>::new();
        let mut input_counts = HashMap::<OutPoint, usize>::new();
        for change in &changes {
            if let Some(old) = &change.old {
                for parent in old.parent_hashes() {
                    let count = parent_counts
                        .entry(parent.clone())
                        .or_insert_with(|| self.by_parent.get(&parent).map_or(0, BTreeSet::len));
                    *count = count
                        .checked_sub(1)
                        .ok_or(PrePoolError::ProjectionInconsistent(
                            "parent projection omits a primary edge",
                        ))?;
                }
                if let EntryState::Ready { inputs, .. } = &old.state {
                    for input in inputs {
                        let count = input_counts.entry(input.clone()).or_insert_with(|| {
                            self.ready_by_input.get(input).map_or(0, BTreeSet::len)
                        });
                        *count =
                            count
                                .checked_sub(1)
                                .ok_or(PrePoolError::ProjectionInconsistent(
                                    "ready-input projection omits a primary edge",
                                ))?;
                    }
                }
            }
        }
        for change in &changes {
            let Some(next) = &change.next else {
                continue;
            };
            for parent in next.parent_hashes() {
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
        let mut queue_lengths = self.queues.map(FairQueue::len);
        for change in &changes {
            self.apply_queue_transition(
                &mut queue_lengths,
                change.old.as_ref(),
                change.next.as_ref(),
            )?;
            if let Some(old) = &change.old {
                let charge = Residency::new(1, old.charge_bytes());
                total_usage =
                    total_usage
                        .checked_sub(charge)
                        .ok_or(PrePoolError::ProjectionInconsistent(
                            "total usage omits a cohort primary",
                        ))?;
                if let Some(peer) = old.source.peer() {
                    remote_usage = remote_usage.checked_sub(charge).ok_or(
                        PrePoolError::ProjectionInconsistent("remote usage omits a cohort primary"),
                    )?;
                    let usage = peer_updates
                        .entry(peer)
                        .or_insert_with(|| self.peer_usage.get(&peer).copied().unwrap_or_default());
                    *usage =
                        usage
                            .checked_sub(charge)
                            .ok_or(PrePoolError::ProjectionInconsistent(
                                "peer usage omits a cohort primary",
                            ))?;
                }
                if Self::is_conflict(old) {
                    conflict_usage = conflict_usage.checked_sub(charge).ok_or(
                        PrePoolError::ProjectionInconsistent(
                            "conflict usage omits a cohort primary",
                        ),
                    )?;
                }
                if let Some(owner) = Self::active_owner(old.source, &old.state) {
                    active_work =
                        active_work
                            .checked_sub(1)
                            .ok_or(PrePoolError::ProjectionInconsistent(
                                "active-work usage omits an active primary",
                            ))?;
                    let active = owner_updates.entry(owner).or_insert_with(|| {
                        self.active_by_owner
                            .get(&owner)
                            .copied()
                            .unwrap_or_default()
                    });
                    *active = active
                        .checked_sub(1)
                        .ok_or(PrePoolError::ProjectionInconsistent(
                            "active-owner usage omits an active primary",
                        ))?;
                }
                affected_owners.insert(old.source.into());
            }
            if let Some(next) = &change.next {
                let charge = Residency::new(1, next.charge_bytes());
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
            queue_lengths,
            affected_owners,
            next_revision,
            next_arrival,
        })
    }

    /// Compile every fallible primary, projection, budget, clock and
    /// dependency-publication predicate, then retain the authority's unique
    /// mutable borrow until the resulting transaction is consumed or dropped.
    pub(super) fn prepare_cohort(
        &mut self,
        desired: MutationSet,
        next_revision: EntryRevision,
        next_arrival: Arrival,
        dependency_changes: impl IntoIterator<Item = DependencyKey>,
    ) -> Result<PreparedKernelMutation<'_>, PrePoolError> {
        let cohort = self.compile_cohort(desired, next_revision, next_arrival)?;
        self.seal_cohort(cohort, dependency_changes)
    }

    /// Seal a selected read-only delta only after all fallback policy has
    /// finished. Callers may compile alternative policies, but only this
    /// method creates the exclusive capability required to Apply one.
    pub(super) fn seal_cohort(
        &mut self,
        mut cohort: CohortPlan,
        dependency_changes: impl IntoIterator<Item = DependencyKey>,
    ) -> Result<PreparedKernelMutation<'_>, PrePoolError> {
        let dependency_changes =
            self.plan_dependency_changes_for_cohort(dependency_changes, &cohort)?;
        cohort.bind_conflict_observation_cut(&dependency_changes);
        Ok(PreparedKernelMutation {
            authority: self,
            cohort,
            dependency_changes,
        })
    }

    /// Total half of [`Self::prepare_cohort`]. Any failure here is an internal
    /// invariant violation; no transaction-shaped input is interpreted after
    /// the first projection mutation.
    fn apply_cohort(
        &mut self,
        CohortPlan {
            changes,
            total_usage,
            remote_usage,
            conflict_usage,
            peer_updates,
            active_work,
            owner_updates,
            queue_lengths,
            affected_owners,
            next_revision,
            next_arrival,
        }: CohortPlan,
    ) {
        self.total_usage = total_usage;
        self.remote_usage = remote_usage;
        self.conflict_usage = conflict_usage;
        self.active_work = active_work;
        for (peer, usage) in peer_updates {
            if usage == Residency::default() {
                self.peer_usage.remove(&peer);
            } else {
                self.peer_usage.insert(peer, usage);
            }
        }
        for (owner, active) in owner_updates {
            if active == 0 {
                self.active_by_owner.remove(&owner);
            } else {
                self.active_by_owner.insert(owner, active);
            }
        }

        for change in &changes {
            if let Some(old) = &change.old {
                self.detach_indexes(&change.hash, old);
                self.entries.remove(&change.hash);
            }
        }
        for change in changes {
            if let Some(next) = change.next {
                self.attach_indexes(&change.hash, &next);
                self.entries.insert(change.hash, next);
            }
        }
        for (lane, len) in queue_lengths.into_entries() {
            self.queues.get_mut(lane).set_len(len);
        }
        self.next_revision = next_revision;
        self.next_arrival = next_arrival;
        for owner in affected_owners {
            self.refresh_owner_runnable(owner);
        }
    }

    pub(super) fn deadline_key(hash: &Byte32, entry: &Entry) -> Option<DeadlineKey> {
        entry.expires_at.map(|expires_at| DeadlineKey {
            expires_at,
            hash: hash.clone(),
            revision: entry.revision,
        })
    }

    pub(super) fn attach_indexes(&mut self, hash: &Byte32, entry: &Entry) {
        self.attach_common_indexes(hash, entry);
        if let Some((lane, key)) = entry.queued_work(hash, self.limits.verify_fee_rate_ordering) {
            self.queues.get_mut(lane).apply_insert(key);
            self.refresh_owner_runnable(entry.source.into());
        }
        if let EntryState::Wait(wait) = &entry.state {
            let edge = WaitEdge {
                hash: hash.clone(),
                revision: entry.revision,
            };
            for key in wait.observed.keys() {
                self.waiters
                    .entry(key.clone())
                    .or_default()
                    .insert(edge.clone());
            }
        }
        if let EntryState::Ready { payload, inputs } = &entry.state {
            let rank = entry.ready_key_for(hash, payload);
            self.ready.insert(rank.clone());
            for input in inputs {
                self.ready_by_input
                    .entry(input.clone())
                    .or_default()
                    .insert(rank.clone());
            }
        }
    }

    fn attach_common_indexes(&mut self, hash: &Byte32, entry: &Entry) {
        self.by_short_id.insert(entry.short_id(), hash.clone());
        if let Some(peer) = entry.raw.ingress_peer() {
            self.by_ingress_peer
                .entry(peer)
                .or_default()
                .insert(hash.clone());
        }
        for parent in entry.parent_hashes() {
            self.by_parent
                .entry(parent)
                .or_default()
                .insert(hash.clone());
        }
        if let Some(deadline) = Self::deadline_key(hash, entry) {
            self.deadlines.insert(deadline);
        }
    }

    pub(super) fn detach_indexes(&mut self, hash: &Byte32, entry: &Entry) {
        self.detach_indexes_with_checkout(hash, entry, None);
    }

    fn detach_indexes_with_checkout(
        &mut self,
        hash: &Byte32,
        entry: &Entry,
        checkout: Option<(WorkLane, WorkKey, u128)>,
    ) {
        self.detach_common_indexes(hash, entry);
        if let Some((lane, key)) = entry.queued_work(hash, self.limits.verify_fee_rate_ordering) {
            match checkout {
                Some((checkout_lane, checkout_key, next_turn))
                    if checkout_lane == lane && checkout_key == key =>
                {
                    self.queues.get_mut(lane).apply_checkout(&key, next_turn);
                }
                _ => self.queues.get_mut(lane).apply_remove(&key),
            }
        }
        if let EntryState::Wait(wait) = &entry.state {
            let edge = WaitEdge {
                hash: hash.clone(),
                revision: entry.revision,
            };
            for key in wait.observed.keys() {
                let empty = self.waiters.get_mut(key).is_none_or(|edges| {
                    edges.remove(&edge);
                    edges.is_empty()
                });
                if empty {
                    self.waiters.remove(key);
                    if !self.dirty.contains_key(key) {
                        self.availability_epoch.remove(key);
                    }
                }
            }
        }
        if let EntryState::Ready { payload, inputs } = &entry.state {
            let rank = entry.ready_key_for(hash, payload);
            self.ready.remove(&rank);
            for input in inputs {
                let empty = self.ready_by_input.get_mut(input).is_none_or(|candidates| {
                    candidates.remove(&rank);
                    candidates.is_empty()
                });
                if empty {
                    self.ready_by_input.remove(input);
                }
            }
        }
    }

    fn detach_common_indexes(&mut self, hash: &Byte32, entry: &Entry) {
        if self.by_short_id.get(&entry.short_id()) == Some(hash) {
            self.by_short_id.remove(&entry.short_id());
        }
        if let Some(peer) = entry.raw.ingress_peer() {
            let empty = self.by_ingress_peer.get_mut(&peer).is_none_or(|hashes| {
                hashes.remove(hash);
                hashes.is_empty()
            });
            if empty {
                self.by_ingress_peer.remove(&peer);
            }
        }
        for parent in entry.parent_hashes() {
            let empty = self.by_parent.get_mut(&parent).is_none_or(|children| {
                children.remove(hash);
                children.is_empty()
            });
            if empty {
                self.by_parent.remove(&parent);
            }
        }
        if let Some(deadline) = Self::deadline_key(hash, entry) {
            self.deadlines.remove(&deadline);
        }
    }

    fn plan_entry_replacement(
        &self,
        hash: &Byte32,
        next: &StoredEntry,
        mode: ReplacementMode,
        next_revision: EntryRevision,
        next_arrival: Arrival,
    ) -> Result<EntryReplacementPlan, PrePoolError> {
        self.validate_entry_shape(hash, next)?;
        let old = self
            .entries
            .get(hash)
            .ok_or_else(|| PrePoolError::Missing(hash.clone()))?;
        self.validate_entry_projection(hash, old)?;
        if old.short_id() != next.short_id()
            && let Some(existing_hash) = self.by_short_id.get(&next.short_id())
        {
            return Err(PrePoolError::ShortIdCollision(
                next.short_id(),
                existing_hash.clone(),
            ));
        }
        let usage = self.plan_usage_delta(Some(old), Some(next))?;
        let active = self.plan_active_transition(
            Self::active_owner(old.source, &old.state),
            Self::active_owner(next.source, &next.state),
        )?;
        let mut queue_lengths = self.queues.map(FairQueue::len);
        self.apply_queue_transition(&mut queue_lengths, Some(old), Some(next))?;
        let checkout = match mode {
            ReplacementMode::Ordinary => None,
            ReplacementMode::Checkout(capability) => {
                let (lane, key) = old
                    .queued_work(hash, self.limits.verify_fee_rate_ordering)
                    .ok_or(PrePoolError::ProjectionInconsistent(
                        "queued checkout source has no work key",
                    ))?;
                let next_turn = self.queues.get(lane).plan_checkout(&key, capability)?;
                Some((lane, key, next_turn))
            }
        };
        Ok(EntryReplacementPlan {
            usage,
            active,
            queue_lengths,
            checkout,
            next_revision,
            next_arrival,
        })
    }

    pub(super) fn replace_entry(
        &mut self,
        hash: &Byte32,
        next: Entry,
        next_revision: EntryRevision,
        next_arrival: Arrival,
        mode: ReplacementMode,
    ) -> Result<(), PrePoolError> {
        let next = StoredEntry::prepare(next, self.limits)?;
        let plan = self.plan_entry_replacement(hash, &next, mode, next_revision, next_arrival)?;
        // Planning holds the exclusive kernel borrow and completes every
        // fallible predicate above. Move the validated primary into Apply
        // instead of deep-cloning its dependency and observation sets into
        // the plan merely to detach them once.
        let old = self
            .entries
            .remove(hash)
            .ok_or(PrePoolError::ProjectionInconsistent(
                "prepared replacement lost its primary",
            ))?;
        let old_source = old.source;
        self.detach_indexes_with_checkout(hash, &old, plan.checkout);
        self.apply_usage_plan(plan.usage);
        self.attach_indexes(hash, &next);
        let next_source = next.source;
        self.entries.insert(hash.clone(), next);
        for (lane, len) in plan.queue_lengths.into_entries() {
            self.queues.get_mut(lane).set_len(len);
        }
        self.next_revision = plan.next_revision;
        self.next_arrival = plan.next_arrival;
        self.apply_active_plan(plan.active);
        self.refresh_owner_runnable(old_source.into());
        self.refresh_owner_runnable(next_source.into());
        Ok(())
    }

    pub(super) fn remove_entry_without_dependency_change(
        &mut self,
        hash: &Byte32,
    ) -> Result<TerminalRecord, PrePoolError> {
        let (active_plan, usage_plan, queue_lengths) = {
            let entry = self
                .entries
                .get(hash)
                .ok_or_else(|| PrePoolError::Missing(hash.clone()))?;
            self.validate_entry_projection(hash, entry)?;
            let old_active = Self::active_owner(entry.source, &entry.state);
            let mut queue_lengths = self.queues.map(FairQueue::len);
            self.apply_queue_transition(&mut queue_lengths, Some(entry), None)?;
            (
                self.plan_active_transition(old_active, None)?,
                self.plan_usage_delta(Some(entry), None)?,
                queue_lengths,
            )
        };
        let entry = self
            .entries
            .remove(hash)
            .ok_or(PrePoolError::ProjectionInconsistent(
                "prepared removal lost its primary",
            ))?;
        self.detach_indexes(hash, &entry);
        self.apply_usage_plan(usage_plan);
        self.apply_active_plan(active_plan);
        for (lane, len) in queue_lengths.into_entries() {
            self.queues.get_mut(lane).set_len(len);
        }
        self.refresh_owner_runnable(entry.source.into());
        let entry = entry.into_draft();
        Ok(TerminalRecord {
            hash: hash.clone(),
            raw: entry.raw,
            source: entry.source,
        })
    }

    /// Remove a definitive-unavailable cohort and invalidate every exact
    /// consumer in the same Plan/Apply. The level change is then queued for
    /// bounded maintenance, which re-resolves remote consumers (and therefore
    /// republishes their missing-parent request) while trusted consumers reach
    /// their ordinary terminal policy instead of remaining parked forever.
    fn prepare_unavailable_entries(
        &mut self,
        hashes: &[Byte32],
    ) -> Result<Option<PreparedTerminalCohort<'_>>, PrePoolError> {
        let mut present = hashes
            .iter()
            .filter(|hash| self.entries.contains_key(*hash))
            .cloned()
            .collect::<Vec<_>>();
        present.sort_unstable();
        present.dedup();
        if present.is_empty() {
            return Ok(None);
        }

        let parents = present.iter().cloned().collect::<HashSet<_>>();
        let changed_keys = self.dependency_keys_for_parents(&parents);
        let records = present
            .iter()
            .map(|hash| {
                self.terminal_record(hash).ok_or_else(|| {
                    PrePoolError::ProjectionInconsistent(
                        "present unavailable cohort member lost its primary",
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut revision_cursor = self.next_revision;
        let mut desired = MutationSet::default();
        for (_, entry) in self.unavailable_replacements(&parents, &mut revision_cursor)? {
            desired.set_entry(entry);
        }
        for hash in present {
            desired.set_remove(hash);
        }
        let prepared =
            self.prepare_cohort(desired, revision_cursor, self.next_arrival, changed_keys)?;
        Ok(Some(PreparedTerminalCohort { prepared, records }))
    }

    pub(super) fn remove_unavailable_entries(
        &mut self,
        hashes: &[Byte32],
    ) -> Result<Vec<TerminalRecord>, PrePoolError> {
        Ok(self
            .prepare_unavailable_entries(hashes)?
            .map_or_else(Vec::new, PreparedTerminalCohort::apply))
    }

    pub(super) fn remove_unavailable_entry(
        &mut self,
        hash: &Byte32,
    ) -> Result<TerminalRecord, PrePoolError> {
        if !self.entries.contains_key(hash) {
            return Err(PrePoolError::Missing(hash.clone()));
        }
        // Definitive rejection without a dependent is the common hostile
        // input path. Preserve the original allocation-free O(1) removal;
        // only a real reverse-edge fan-out pays for cohort planning.
        if self.by_parent.get(hash).is_none_or(BTreeSet::is_empty) {
            return self.remove_entry_without_dependency_change(hash);
        }
        let mut records = self.remove_unavailable_entries(std::slice::from_ref(hash))?;
        records
            .pop()
            .ok_or_else(|| PrePoolError::Missing(hash.clone()))
    }

    pub(super) fn validate_location(
        &self,
        hash: &Byte32,
        revision: EntryRevision,
        expected: PrePoolLocation,
    ) -> Result<&StoredEntry, PrePoolError> {
        let entry = self
            .entries
            .get(hash)
            .ok_or_else(|| PrePoolError::Missing(hash.clone()))?;
        if entry.revision != revision {
            return Err(PrePoolError::revision_mismatch(
                hash.clone(),
                revision,
                entry.revision,
            ));
        }
        let actual = entry.state.location();
        if actual != expected {
            return Err(PrePoolError::location_mismatch(
                hash.clone(),
                expected,
                actual,
            ));
        }
        Ok(entry)
    }

    pub(super) fn validate_resolve_lease(
        &self,
        lease: &ResolveLease,
    ) -> Result<&StoredEntry, PrePoolError> {
        self.validate_location(&lease.hash, lease.revision, PrePoolLocation::ResolveLeased)
    }

    pub(super) fn validate_verify_lease(
        &self,
        lease: &VerifyLease,
    ) -> Result<&StoredEntry, PrePoolError> {
        self.validate_location(&lease.hash, lease.revision, PrePoolLocation::VerifyLeased)
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
            .ok_or(PrePoolError::ProjectionInconsistent(
                "active-work usage does not match primary ownership",
            ))?;
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
                .ok_or(PrePoolError::ProjectionInconsistent(
                    "active-owner usage does not match primary ownership",
                ))?;
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
            self.queues.get_mut(lane).set_runnable(owner, runnable);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn admit(
        &mut self,
        raw: PipelineRawTx,
        lane: ResolveLane,
        source: PrePoolSource,
        expires_at: Option<u64>,
        dependencies: BTreeSet<DependencyKey>,
    ) -> Result<(), PrePoolError> {
        let hash = crate::util::compact_packed(&raw.tx.hash());
        let short_id = crate::util::compact_packed(&raw.tx.proposal_short_id());
        if self.entries.contains_key(&hash) {
            return Err(PrePoolError::DuplicateHash(hash));
        }
        if let Some(existing_hash) = self.by_short_id.get(&short_id) {
            return Err(PrePoolError::ShortIdCollision(
                short_id,
                existing_hash.clone(),
            ));
        }
        let dependencies = dependencies
            .into_iter()
            .map(DependencyKey::into_compact)
            .collect::<BTreeSet<_>>();
        let mut revision_cursor = self.next_revision;
        let revision = EntryRevision::take(&mut revision_cursor)?;
        let mut arrival_cursor = self.next_arrival;
        let arrival = Arrival::take(&mut arrival_cursor)?;
        let payload_charge_bytes = raw.charge_bytes();
        let entry = Entry {
            raw: Arc::new(raw),
            source,
            state: EntryState::ResolveQueued { lane },
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
        Ok(())
    }

    pub(crate) fn promote_source(&mut self, hash: &Byte32) -> Result<(), PrePoolError> {
        let mut next = self
            .entries
            .get(hash)
            .cloned()
            .ok_or_else(|| PrePoolError::Missing(hash.clone()))?
            .into_draft();
        if next.source == PrePoolSource::Proposal {
            return Ok(());
        }
        next.source = PrePoolSource::Proposal;
        next.expires_at = None;
        let mut revision_cursor = self.next_revision;
        if let EntryState::Wait(wait) = &next.state
            && wait.reason == WaitReason::Missing
        {
            next.revision = EntryRevision::take(&mut revision_cursor)?;
            next.state = EntryState::ResolveQueued {
                lane: ResolveLane::Ordered,
            };
        }
        if let EntryState::Ready { .. } = &mut next.state {
            next.revision = EntryRevision::take(&mut revision_cursor)?;
        }
        self.replace_entry(
            hash,
            next,
            revision_cursor,
            self.next_arrival,
            ReplacementMode::Ordinary,
        )?;
        Ok(())
    }

    pub(crate) fn replace_raw_payload(
        &mut self,
        hash: &Byte32,
        raw: PipelineRawTx,
        raw_bytes: usize,
        lane: ResolveLane,
    ) -> Result<(), PrePoolError> {
        let mut next = self
            .entries
            .get(hash)
            .cloned()
            .ok_or_else(|| PrePoolError::Missing(hash.clone()))?
            .into_draft();
        next.raw = Arc::new(raw);
        next.source = PrePoolSource::Proposal;
        next.expires_at = None;
        let mut revision_cursor = self.next_revision;
        next.revision = EntryRevision::take(&mut revision_cursor)?;
        next.payload_charge_bytes = raw_bytes;
        next.state = EntryState::ResolveQueued { lane };
        self.replace_entry(
            hash,
            next,
            revision_cursor,
            self.next_arrival,
            ReplacementMode::Ordinary,
        )?;
        Ok(())
    }

    pub(crate) fn checkout_resolve(
        &mut self,
        lane: ResolveLane,
    ) -> Result<Option<ResolveLease>, PrePoolError> {
        let work_lane = Self::lane_for_resolve(lane);
        let Some(key) = self
            .queues
            .get(work_lane)
            .peek(WorkCapability::Any)
            .cloned()
        else {
            return Ok(None);
        };
        let mut next = self
            .validate_location(&key.hash, key.revision, PrePoolLocation::ResolveQueued)?
            .clone()
            .into_draft();
        let mut revision_cursor = self.next_revision;
        let revision = EntryRevision::take(&mut revision_cursor)?;
        next.revision = revision;
        next.state = EntryState::ResolveLeased;
        let payload = Arc::clone(&next.raw);
        self.replace_entry(
            &key.hash,
            next,
            revision_cursor,
            self.next_arrival,
            ReplacementMode::Checkout(WorkCapability::Any),
        )?;
        Ok(Some(ResolveLease {
            hash: key.hash,
            lane,
            revision,
            payload,
        }))
    }

    pub(crate) fn terminalize_resolve(
        &mut self,
        lease: &ResolveLease,
    ) -> Result<TerminalRecord, PrePoolError> {
        self.validate_resolve_lease(lease)?;
        self.remove_unavailable_entry(&lease.hash)
    }

    pub(crate) fn terminalize_verify(
        &mut self,
        lease: &VerifyLease,
    ) -> Result<TerminalRecord, PrePoolError> {
        self.validate_verify_lease(lease)?;
        self.remove_unavailable_entry(&lease.hash)
    }

    pub(crate) fn complete_resolve(
        &mut self,
        lease: &ResolveLease,
        resolved: ResolvedTx,
        charge_bytes: usize,
        schedule: VerifySchedule,
        discovered_dependencies: BTreeSet<DependencyKey>,
    ) -> Result<(), PrePoolError> {
        let mut next = self.validate_resolve_lease(lease)?.clone().into_draft();
        next.dependencies.extend(
            discovered_dependencies
                .into_iter()
                .map(DependencyKey::into_compact),
        );
        let mut revision_cursor = self.next_revision;
        next.revision = EntryRevision::take(&mut revision_cursor)?;
        next.payload_charge_bytes = charge_bytes;
        next.state = EntryState::VerifyQueued {
            payload: Arc::new(resolved),
            schedule,
        };
        self.replace_entry(
            &lease.hash,
            next,
            revision_cursor,
            self.next_arrival,
            ReplacementMode::Ordinary,
        )?;
        Ok(())
    }

    pub(crate) fn complete_resolve_and_checkout(
        &mut self,
        lease: &ResolveLease,
        resolved: ResolvedTx,
        charge_bytes: usize,
        schedule: VerifySchedule,
        discovered_dependencies: BTreeSet<DependencyKey>,
    ) -> Result<AppliedContinuation<ResolveLease>, PrePoolError> {
        self.complete_resolve(
            lease,
            resolved,
            charge_bytes,
            schedule,
            discovered_dependencies,
        )?;
        Ok(AppliedContinuation::from_checkout(
            self.checkout_resolve(lease.lane),
        ))
    }

    pub(crate) fn complete_resolve_without_checkout(
        &mut self,
        lease: &ResolveLease,
        resolved: ResolvedTx,
        charge_bytes: usize,
        schedule: VerifySchedule,
        discovered_dependencies: BTreeSet<DependencyKey>,
    ) -> Result<AppliedContinuation<ResolveLease>, PrePoolError> {
        self.complete_resolve(
            lease,
            resolved,
            charge_bytes,
            schedule,
            discovered_dependencies,
        )?;
        Ok(AppliedContinuation::yielded())
    }

    pub(crate) fn checkout_verify(
        &mut self,
        capability: WorkCapability,
    ) -> Result<Option<VerifyLease>, PrePoolError> {
        let lane = WorkLane::Verify;
        let Some(key) = self.queues.get(lane).peek(capability).cloned() else {
            return Ok(None);
        };
        let mut next = self
            .validate_location(&key.hash, key.revision, PrePoolLocation::VerifyQueued)?
            .clone()
            .into_draft();
        let payload = match &next.state {
            EntryState::VerifyQueued { payload, .. } => Arc::clone(payload),
            _ => {
                return Err(PrePoolError::ProjectionInconsistent(
                    "VerifyQueued location contains a non-VerifyQueued state",
                ));
            }
        };
        let mut revision_cursor = self.next_revision;
        let revision = EntryRevision::take(&mut revision_cursor)?;
        next.revision = revision;
        next.state = EntryState::VerifyLeased {
            payload: Arc::clone(&payload),
        };
        self.replace_entry(
            &key.hash,
            next,
            revision_cursor,
            self.next_arrival,
            ReplacementMode::Checkout(capability),
        )?;
        Ok(Some(VerifyLease {
            hash: key.hash,
            revision,
            capability,
            payload,
        }))
    }

    pub(crate) fn complete_verify(
        &mut self,
        lease: &VerifyLease,
        mut verified: PipelineVerifiedTx,
        charge_bytes: usize,
    ) -> Result<(), PrePoolError> {
        let mut next = self.validate_verify_lease(lease)?.clone().into_draft();
        let candidate_hash = crate::util::compact_packed(&verified.candidate.tx.hash());
        if candidate_hash != lease.hash {
            return Err(PrePoolError::primary_key_mismatch(
                lease.hash.clone(),
                candidate_hash,
            ));
        }
        let inputs = verified
            .candidate
            .tx
            .input_pts_iter()
            .map(|input| crate::util::compact_packed(&input))
            .collect::<BTreeSet<_>>();
        if inputs.is_empty() || verified.candidate.tx_size == 0 {
            return Err(PrePoolError::ZeroTransactionSize(lease.hash.clone()));
        }
        // Promotion may race with verification, but Ready is published only
        // by this transition. Bind the payload to the source owned by this
        // exact revision instead of relying on a separate, immediately stale
        // read in the worker.
        verified.candidate.source = next.raw.authoritative_source(next.source);
        let inputs = ReadyInputs::new(inputs, self.limits.max_inputs_per_ready)?;
        let mut revision_cursor = self.next_revision;
        let revision = EntryRevision::take(&mut revision_cursor)?;
        next.revision = revision;
        next.payload_charge_bytes = charge_bytes;
        next.state = EntryState::Ready {
            payload: Arc::new(verified),
            inputs,
        };
        self.replace_entry(
            &lease.hash,
            next,
            revision_cursor,
            self.next_arrival,
            ReplacementMode::Ordinary,
        )?;
        Ok(())
    }

    pub(crate) fn complete_verify_and_checkout(
        &mut self,
        lease: &VerifyLease,
        verified: PipelineVerifiedTx,
        charge_bytes: usize,
    ) -> Result<AppliedContinuation<VerifyLease>, PrePoolError> {
        self.complete_verify(lease, verified, charge_bytes)?;
        Ok(AppliedContinuation::from_checkout(
            self.checkout_verify(lease.capability),
        ))
    }

    pub(crate) fn complete_verify_without_checkout(
        &mut self,
        lease: &VerifyLease,
        verified: PipelineVerifiedTx,
        charge_bytes: usize,
    ) -> Result<AppliedContinuation<VerifyLease>, PrePoolError> {
        self.complete_verify(lease, verified, charge_bytes)?;
        Ok(AppliedContinuation::yielded())
    }

    pub(crate) fn force_terminalize(
        &mut self,
        hash: &Byte32,
    ) -> Result<Option<TerminalRecord>, PrePoolError> {
        if !self.entries.contains_key(hash) {
            return Ok(None);
        }
        self.remove_unavailable_entry(hash).map(Some)
    }

    pub(crate) fn plan_peer_revocation(
        &mut self,
        peer: PeerIndex,
        candidates: &[Byte32],
    ) -> Result<Option<PreparedTerminalCohort<'_>>, PrePoolError> {
        let hashes = candidates
            .iter()
            .filter_map(|hash| {
                let record = self.terminal_record(hash)?;
                (record.raw.ingress_peer() == Some(peer)).then_some(record.hash)
            })
            .collect::<Vec<_>>();
        self.prepare_unavailable_entries(&hashes)
    }

    fn due_hashes(&self, now: u64, limit: usize) -> Vec<Byte32> {
        self.deadlines
            .iter()
            .take_while(|deadline| deadline.expires_at <= now)
            .filter(|deadline| {
                self.entries
                    .get(&deadline.hash)
                    .is_some_and(|entry| entry.revision == deadline.revision)
            })
            .take(limit)
            .map(|deadline| deadline.hash.clone())
            .collect()
    }

    pub(crate) fn due_terminal_records(&self, now: u64, limit: usize) -> Vec<TerminalRecord> {
        self.due_hashes(now, limit)
            .iter()
            .filter_map(|hash| self.terminal_record(hash))
            .collect()
    }

    pub(crate) fn plan_expiry(
        &mut self,
        now: u64,
        limit: usize,
    ) -> Result<Option<PreparedTerminalCohort<'_>>, PrePoolError> {
        let hashes = self.due_hashes(now, limit);
        self.prepare_unavailable_entries(&hashes)
    }

    pub(crate) fn work_is_ready(&self, lane: WorkLane, capability: WorkCapability) -> bool {
        match lane {
            WorkLane::Commit => !self.ready.is_empty(),
            _ => self.queues.get(lane).peek(capability).is_some(),
        }
    }
}
