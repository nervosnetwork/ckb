//! Apply one prepared decision. Recoverable checks, the protected mutation and
//! lock release stay in one path; retired payloads outlive every authority guard.
use super::plan::LifecycleWrite;
use super::{
    COMMITTED_HASH_CACHE_CAPACITY, CommitLocks, Edit, Guard, Peer, Plan, Relation, RelationMember,
    Roles, SHARDS, Shard, Store, View, Wake, WakePage, acquire, compact_dependency,
    compact_relation, proposal_key,
};
use crate::authority::{
    budget::OwnerDelta,
    model::{DependencyKey, Entry, Error, FullReason, Phase, RelationKey, Status},
    notice::{self, Batch},
    queue::{Queues, WorkStage},
};
use crate::util::compact_packed;
use ckb_network::PeerIndex;
use ckb_snapshot::Snapshot;
use ckb_types::packed::{Byte32, ProposalShortId};
use ckb_util::parking_lot::Mutex;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, atomic::Ordering},
    time::Instant,
};

#[derive(Clone, Copy, Default)]
struct MemberChange<T> {
    before: T,
    after: T,
}

// Unique owner hashes in order, each paired with its membership transition.
type MemberChanges<T> = Vec<(Byte32, MemberChange<T>)>;

/// Payload displaced by a committed cut. Keep it alive until all authority
/// guards are released; dropping an owner can run caller-owned destructors.
#[derive(Default)]
struct Retired {
    owners: Vec<Arc<Entry>>,
    shards: Vec<Shard>,
    relations: Vec<BTreeMap<RelationKey, Arc<Mutex<Relation>>>>,
    peers: Option<BTreeMap<PeerIndex, Arc<Mutex<Peer>>>>,
    dirty: Option<BTreeSet<DependencyKey>>,
    queues: Option<Queues>,
    snapshot: Option<Arc<Snapshot>>,
    _committed: Option<lru::LruCache<ProposalShortId, Byte32>>,
}

/// A Plan and its reserved notices remain paired through the one possible
/// synchronous history retry. A failed application drops the reservation first.
struct Application<'a> {
    store: &'a Store,
    notice: Option<notice::Reservation>,
    plan: Plan,
}

/// Derive each owner transition's routed edits, projection changes and wake keys
/// together. Original read observations add their guards at the Apply boundary.
struct OwnerChanges<'a> {
    owner_edits: Vec<(usize, &'a Byte32, &'a Edit)>,
    locks: CommitLocks,
    relation_changes: BTreeMap<RelationKey, MemberChanges<Roles>>,
    peer_changes: BTreeMap<PeerIndex, MemberChanges<bool>>,
}
impl<'a> OwnerChanges<'a> {
    fn derive(
        store: &Store,
        edits: &'a BTreeMap<Byte32, Edit>,
        wake: &mut BTreeSet<DependencyKey>,
        replace_generation: bool,
    ) -> Self {
        let mut locks = CommitLocks::default();
        let mut owner_edits = Vec::with_capacity(edits.len());
        // Plan hashes are unique and ordered; the last per-key change can merge
        // repeated roles of the current owner without another lookup tree.
        let mut relation_changes: BTreeMap<RelationKey, MemberChanges<Roles>> = BTreeMap::new();
        let mut peer_changes: BTreeMap<PeerIndex, MemberChanges<bool>> = BTreeMap::new();
        for (hash, edit) in edits {
            let index = store.owner_shard(hash);
            locks.owners.insert(index, true);
            owner_edits.push((index, hash, edit));
            if replace_generation {
                continue;
            }
            visit_role_changes(edit, |key, change| {
                if let Some(changes) = relation_changes.get_mut(&key) {
                    // Ordered owners keep duplicate roles in the final item.
                    if let Some((_, roles)) = changes.last_mut().filter(|(last, _)| last == hash) {
                        roles.before |= change.before;
                        roles.after |= change.after;
                    } else {
                        changes.push((hash.clone(), change));
                    }
                } else {
                    relation_changes.insert(compact_relation(&key), vec![(hash.clone(), change)]);
                }
                locks.dependencies.insert(
                    store.route(&key),
                    (change.before | change.after).intersects(Roles::SPENDER),
                );
            });
            let old_peer = edit.before.as_deref().and_then(Entry::preaccepted_peer);
            let new_peer = edit.after.as_deref().and_then(Entry::preaccepted_peer);
            if edit.after.is_none()
                && let Some(before) = edit.before.as_ref().filter(|entry| entry.preaccepted())
            {
                wake.extend(
                    before
                        .transaction
                        .output_pts_iter()
                        .map(|point| DependencyKey::Cell(compact_packed(&point))),
                );
            }
            for peer in old_peer.into_iter().chain(new_peer) {
                locks.peers.insert(store.route(&peer), false);
                if old_peer != new_peer {
                    peer_changes
                        .entry(peer)
                        .or_insert_with(|| Vec::with_capacity(1))
                        .push((
                            hash.clone(),
                            MemberChange {
                                before: old_peer == Some(peer),
                                after: new_peer == Some(peer),
                            },
                        ));
                }
            }
            for entry in edit
                .before
                .iter()
                .chain(&edit.after)
                .filter(|entry| entry.accepted().is_some())
            {
                for point in entry
                    .transaction
                    .output_pts_iter()
                    .chain(entry.transaction.input_pts_iter())
                {
                    wake.insert(DependencyKey::Cell(compact_packed(&point)));
                }
            }
        }
        // Route edits once before locking; both guarded passes retain the original
        // shard-major, full-hash order using this bounded borrowed scratch.
        owner_edits.sort_unstable_by_key(|(index, hash, _)| (*index, *hash));
        Self {
            owner_edits,
            locks,
            relation_changes,
            peer_changes,
        }
    }

    /// Check every derived index and counter against the guarded owner cut.
    /// Apply may reserve scalar capacity only after this complete preflight.
    fn validate_projections(
        &self,
        store: &Store,
        plan: &Plan,
        owners: &[(usize, Guard<'_, Shard>)],
    ) -> Result<(), Error> {
        let mut remaining_edits = self.owner_edits.as_slice();
        for (index, guard) in owners {
            // Every edited shard has a write guard; read guards have no edits.
            let Guard::Write(shard) = guard else {
                continue;
            };
            let (edits, remaining) = remaining_edits.split_at(
                remaining_edits.partition_point(|(edit_index, _, _)| edit_index == index),
            );
            remaining_edits = remaining;
            let changes = edits.len() as u64;
            let accepted_changes = edits
                .iter()
                .filter(|(_, _, edit)| edit.affects_accepted())
                .count() as u64;
            shard
                .revision
                .checked_add(changes)
                .ok_or(Error::Fault("owner revision"))?;
            shard
                .accepted_revision
                .checked_add(accepted_changes)
                .ok_or(Error::Fault("accepted revision"))?;
            // Full-hash order keeps equal proposal prefixes adjacent and in
            // this same shard. Compare prospective owners, skipping removals.
            let mut previous_proposal = None;
            for (_, hash, edit) in edits.iter().copied() {
                if edit.after.is_some() {
                    let proposal = proposal_key(hash);
                    if let Some(existing) = shard.proposals.get(proposal)
                        && existing != hash
                        && plan
                            .edits
                            .get(existing)
                            .is_none_or(|edit| edit.after.is_some())
                    {
                        return Err(Error::Full("proposal short-ID collision".into()));
                    }
                    if previous_proposal == Some(proposal) {
                        return Err(Error::Full("proposal short-ID collision".into()));
                    }
                    previous_proposal = Some(proposal);
                }
            }
        }
        debug_assert!(
            remaining_edits.is_empty(),
            "every edited shard has a write guard"
        );
        // Point readers may update different members under compatible
        // dependency gates; check the affected rows before any mutation.
        for (key, changes) in &self.relation_changes {
            let row = store.relation(key);
            let row = row.as_ref().map(|row| row.lock());
            let mut spender = row.as_ref().and_then(|row| row.spender.clone());
            for (hash, change) in changes {
                if row.as_ref().map_or(Roles::NONE, |row| row.roles(hash)) != change.before {
                    return Err(Error::Fault("relation projection"));
                }
                if change.before.intersects(Roles::SPENDER) && spender.as_ref() == Some(hash) {
                    spender = None;
                }
            }
            for (hash, change) in changes {
                if change.after.intersects(Roles::SPENDER) {
                    if spender.as_ref().is_some_and(|old| old != hash) {
                        return Err(Error::Fault("multiple accepted spenders"));
                    }
                    spender = Some(hash.clone());
                }
            }
        }
        for key in &plan.wake {
            if let Some(row) = store.relation(&RelationKey::Dependency(key.clone())) {
                row.lock()
                    .next_pass
                    .checked_add(1)
                    .ok_or(Error::Fault("wake pass"))?;
            }
        }
        for (peer, changes) in &self.peer_changes {
            let row = store.peers.lock().get(peer).cloned();
            let row = row.as_ref().map(|row| row.lock());
            for (hash, change) in changes {
                if row.as_ref().is_some_and(|row| row.members.contains(hash)) != change.before {
                    return Err(Error::Fault("peer projection"));
                }
            }
        }
        Ok(())
    }

    /// Install every projection derived from these validated owner changes.
    /// Displaced owners remain alive until Apply releases all authority guards.
    fn apply(
        self,
        store: &Store,
        plan: &Plan,
        owners: &mut [(usize, Guard<'_, Shard>)],
        snapshot: &Snapshot,
    ) -> Retired {
        let Self {
            owner_edits,
            relation_changes,
            peer_changes,
            ..
        } = self;
        let retired = if plan
            .lifecycle
            .as_ref()
            .is_some_and(|write| write.replace_generation)
        {
            store.retire_generation(owners)
        } else {
            let mut retired = Retired {
                owners: Vec::with_capacity(plan.edits.len()),
                ..Retired::default()
            };
            store.apply_owner_changes(owners, &owner_edits, snapshot, &mut retired.owners);
            retired
        };
        for (key, changes) in relation_changes {
            store.apply_relation(key, changes, &plan.edits, &plan.wake);
        }
        store.apply_peer_changes(peer_changes);
        retired
    }
}

/// Keep recoverable Result/Option propagation out of the post-preflight tail.
/// This unit-return boundary does not claim panic or allocation-failure recovery.
fn commit_infallibly(commit: impl FnOnce()) {
    commit();
}

fn visit_roles(entry: &Entry, mut add: impl FnMut(RelationKey, Roles)) {
    match &entry.phase {
        Phase::Accepted(accepted) => {
            for point in entry.transaction.input_pts_iter() {
                add(
                    RelationKey::Dependency(DependencyKey::Cell(point)),
                    Roles::SPENDER,
                );
            }
            for point in accepted.dependencies() {
                add(
                    RelationKey::Dependency(DependencyKey::Cell(point)),
                    Roles::DEPENDENCY,
                );
            }
            for hash in &accepted.parents {
                add(RelationKey::Children(hash.clone()), Roles::CHILD);
            }
        }
        Phase::Waiting(keys) | Phase::Replaced { triggers: keys, .. } => {
            for key in keys {
                add(RelationKey::Dependency(key.clone()), Roles::WAITING);
            }
        }
        Phase::Resolve | Phase::Verify(_) => {}
    }
}
// A roleless side cannot cancel a visited role. Stream that common transition
// directly; two role-bearing sides still need a bounded owner-local merge.
fn visit_role_changes(edit: &Edit, mut add: impl FnMut(RelationKey, MemberChange<Roles>)) {
    let has_roles = |entry: &&Entry| !matches!(entry.phase, Phase::Resolve | Phase::Verify(_));
    let before = edit.before.as_deref().filter(has_roles);
    let after = edit.after.as_deref().filter(has_roles);
    match (before, after) {
        (None, Some(after)) => visit_roles(after, |key, role| {
            add(
                key,
                MemberChange {
                    before: Roles::NONE,
                    after: role,
                },
            );
        }),
        (Some(before), None) => visit_roles(before, |key, role| {
            add(
                key,
                MemberChange {
                    before: role,
                    after: Roles::NONE,
                },
            );
        }),
        (Some(before), Some(after)) => {
            let mut roles: BTreeMap<RelationKey, MemberChange<Roles>> = BTreeMap::new();
            visit_roles(before, |key, role| {
                roles.entry(key).or_default().before |= role
            });
            visit_roles(after, |key, role| {
                roles.entry(key).or_default().after |= role
            });
            for (key, change) in roles {
                if change.before != change.after {
                    add(key, change);
                }
            }
        }
        (None, None) => {}
    }
}
fn deadline(entry: &Entry) -> Option<Instant> {
    entry
        .preaccepted()
        .then(|| entry.source.deadline())
        .flatten()
}

impl Shard {
    /// Apply one preflighted owner edit and all of its local projections.
    /// Retired owners stay with Apply until its guards have been released.
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "Exact owner validation bounds phase counts; Apply prechecks both revision increments."
    )]
    fn apply_edit(
        &mut self,
        hash: &Byte32,
        edit: &Edit,
        snapshot: &Snapshot,
        queues: &Queues,
        retired: &mut Vec<Arc<Entry>>,
    ) {
        // Adding a Shard field requires reviewing this complete owner transition.
        let Self {
            owners,
            proposals,
            deadlines,
            accepted_times,
            waiting,
            proposed,
            revision,
            accepted_revision,
        } = self;
        let proposal = proposal_key(hash);
        let before_deadline = edit.before.as_deref().and_then(deadline);
        let after_deadline = edit.after.as_deref().and_then(deadline);
        if let Some(before) = &edit.before {
            // Exact owner validation and the population bound
            // make these derived phase-count updates infallible.
            *waiting -= usize::from(matches!(before.phase, Phase::Waiting(_)));
            *proposed -= usize::from(
                before
                    .accepted()
                    .is_some_and(|value| value.status(snapshot) == Status::Proposed),
            );
            // A prior edit in this same plan may already have
            // transferred this proposal ID to its new owner.
            if edit.after.is_none() && proposals.get(proposal) == Some(hash) {
                proposals.remove(proposal);
            }
            if before_deadline != after_deadline
                && let Some(deadline) = before_deadline
            {
                deadlines.remove(&(deadline, hash.clone()));
            }
            if let Some(accepted) = before.accepted() {
                accepted_times.remove(&(accepted.timestamp, hash.clone()));
            }
            queues.remove(before);
        }
        if let Some(after) = &edit.after {
            *waiting += usize::from(matches!(after.phase, Phase::Waiting(_)));
            *proposed += usize::from(
                after
                    .accepted()
                    .is_some_and(|value| value.status(snapshot) == Status::Proposed),
            );
            // An owner update keeps its hash and proposal mapping.
            if edit.before.is_none() {
                proposals.insert(*proposal, hash.clone());
            }
            if before_deadline != after_deadline
                && let Some(deadline) = after_deadline
            {
                deadlines.insert((deadline, hash.clone()));
            }
            if let Some(accepted) = after.accepted() {
                accepted_times.insert((accepted.timestamp, hash.clone()));
            }
            retired.extend(owners.insert(hash.clone(), Arc::clone(after)));
            queues.insert(after);
        } else {
            retired.extend(owners.remove(hash));
        }
        *revision += 1;
        if edit.affects_accepted() {
            *accepted_revision += 1;
        }
    }

    fn refresh_proposed(&mut self, snapshot: &Snapshot) {
        self.proposed = self
            .owners
            .values()
            .filter(|entry| {
                entry
                    .accepted()
                    .is_some_and(|value| value.status(snapshot) == Status::Proposed)
            })
            .count();
    }
}

impl LifecycleWrite {
    /// Refresh all proposed counts against the successor view, then install
    /// the view and its ordered committed-hash records as one lifecycle write.
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "Apply checks view revision overflow before the first mutation."
    )]
    fn apply(
        &self,
        store: &Store,
        view: &mut Guard<'_, View>,
        owners: &mut [(usize, Guard<'_, Shard>)],
        retired: &mut Retired,
        committed: Vec<(ProposalShortId, Byte32)>,
    ) {
        for (_, guard) in owners {
            if let Some(shard) = guard.get_mut() {
                shard.refresh_proposed(&self.snapshot);
            }
        }
        if let Some(view) = view.get_mut() {
            retired.snapshot = Some(std::mem::replace(
                &mut view.snapshot,
                Arc::clone(&self.snapshot),
            ));
            view.revision += 1;
        }
        if !committed.is_empty() {
            let mut cache = store.committed.lock();
            for (proposal, hash) in committed {
                cache.put(proposal, hash);
            }
        }
    }
}

impl<'a> Application<'a> {
    fn new(store: &'a Store, plan: Plan) -> Self {
        Self {
            store,
            notice: None,
            plan,
        }
    }

    /// Optional replacement history may lose capacity to another admission.
    /// Only those history edits change between attempts: effect payloads and
    /// class remain paired with the original notice reservation. No failed
    /// plan or reservation escapes to the service's asynchronous capacity wait.
    fn admit(&mut self, retain_history: &mut bool) -> Result<Option<Arc<Batch>>, Error> {
        match self.attempt() {
            Err(Error::Full(FullReason::Pipeline | FullReason::History)) if *retain_history => {
                *retain_history = false;
                if self.store.is_stopped() {
                    // Return to the caller's existing close/requeue handling.
                    return Err(Error::Stale);
                }
                for edit in self.plan.edits.values_mut() {
                    if edit
                        .before
                        .as_ref()
                        .is_some_and(|old| old.accepted().is_some())
                        && edit
                            .after
                            .as_ref()
                            .is_some_and(|new| matches!(new.phase, Phase::Replaced { .. }))
                    {
                        edit.after = None;
                    }
                }
                self.attempt()
            }
            result => result,
        }
    }

    /// Validate one attempt and commit it under the complete authority cut.
    /// On refusal, the same Plan and notice reservation remain available for
    /// the one synchronous admission-history retry.
    fn attempt(&mut self) -> Result<Option<Arc<Batch>>, Error> {
        let store = self.store;
        let plan = &mut self.plan;
        let notice = &mut self.notice;
        #[cfg(feature = "profiling")]
        let _span = tracing::trace_span!(target: "ckb_tx_pool_profile", "tx_pool.authority.apply")
            .entered();
        if store.is_faulted() {
            return Err(Error::Fault("closed generation"));
        }
        let owner_delta = OwnerDelta::new(
            plan.edits
                .values()
                .filter_map(|edit| edit.before.as_deref()),
            plan.edits.values().filter_map(|edit| edit.after.as_deref()),
        )?;
        if notice.is_none() && !plan.dry_run {
            *notice = store
                .outbox
                .reserve(std::mem::take(&mut plan.effects), plan.class)?;
        }
        let replace_generation = plan
            .lifecycle
            .as_ref()
            .is_some_and(|write| write.replace_generation);
        let mut changes =
            OwnerChanges::derive(store, &plan.edits, &mut plan.wake, replace_generation);
        changes.locks.complete_for_plan(store, plan);

        #[cfg(test)]
        let observer = store.commit_observer.lock().clone();
        #[cfg(test)]
        if let Some(observer) = &observer {
            observer(plan, false);
        }
        #[cfg(feature = "profiling")]
        let acquire_span =
            tracing::trace_span!(target: "ckb_tx_pool_profile", "tx_pool.authority.acquire")
                .entered();
        let mut view = if plan.lifecycle.is_some() {
            Guard::Write(store.view.write())
        } else {
            Guard::Read(store.view.read())
        };
        let peer_guards = acquire(&store.peer_gates, &changes.locks.peers);
        let dependency_guards = acquire(&store.dependency_gates, &changes.locks.dependencies);
        let mut owners = acquire(&store.shards, &changes.locks.owners);
        #[cfg(feature = "profiling")]
        drop(acquire_span);
        store.validate_original_cut(plan, &view, &owners)?;
        #[cfg(test)]
        if let Some(observer) = &observer {
            observer(plan, true);
        }
        let now = store.validate_proposed_cut(plan, &changes, &owners)?;
        // A disjoint commit may consume scalar capacity after planning. Sparse
        // accepted pressure requires replanning its fee policy.
        let budget = owner_delta
            .reserve(&store.budget)
            .map_err(|error| match error {
                Error::Full(FullReason::Accepted)
                    if plan.reads.all.is_none() && plan.reads.accepted.is_none() =>
                {
                    Error::Stale
                }
                error => error,
            })?;
        if plan.dry_run {
            return Ok(None);
        }
        // These fresh packed values move into the cache. Cloning them here
        // would allocate under the lifecycle and committed-cache locks.
        let committed = plan
            .lifecycle
            .as_mut()
            .map(|write| std::mem::take(&mut write.committed))
            .unwrap_or_default();
        let mut batch = None;
        commit_infallibly(|| {
            // All recoverable checks precede the first mutation. Collection and
            // marker allocation use Rust's abort-on-OOM platform contract.
            let mut retired = changes.apply(store, plan, &mut owners, &view.get().snapshot);
            if let Some((peer, deadline)) = plan.ban {
                store.apply_ban(peer, deadline, now);
            }
            for key in &plan.wake {
                store.start_wake(key);
            }
            if let Some(page) = &plan.wake_advance {
                store.advance_wake(page);
            }
            if let Some(lifecycle) = &plan.lifecycle {
                lifecycle.apply(store, &mut view, &mut owners, &mut retired, committed);
            }
            batch = notice.take().map(notice::Reservation::append);
            let capacity_returned = budget.commit();
            drop(owners);
            drop(dependency_guards);
            drop(peer_guards);
            drop(view);
            store.publish_commit(plan, &batch, capacity_returned, retired);
        });
        if store.is_faulted() {
            return Err(Error::Fault("committed projection"));
        }
        Ok(batch)
    }
}

impl Store {
    pub(in crate::authority) fn apply(&self, plan: Plan) -> Result<Option<Arc<Batch>>, Error> {
        Application::new(self, plan).attempt()
    }

    pub(in crate::authority) fn apply_admission(
        &self,
        plan: Plan,
        retain_history: &mut bool,
    ) -> Result<Option<Arc<Batch>>, Error> {
        Application::new(self, plan).admit(retain_history)
    }

    /// Make a committed cut visible only after its authority guards open, then
    /// release displaced payloads whose destructors may call back into Store.
    fn publish_commit(
        &self,
        plan: &Plan,
        batch: &Option<Arc<Batch>>,
        capacity_returned: bool,
        retired: Retired,
    ) {
        if let Some(batch) = batch {
            batch.activate(&self.outbox);
        }
        if capacity_returned {
            self.budget.changed.notify_waiters();
        }
        if plan.lifecycle.is_some() || plan.edits.values().any(Edit::affects_accepted) {
            self.template_changed.notify_waiters();
        }
        // Queue-only transitions wake workers below. Maintenance needs
        // dependency, capacity or lifecycle progress instead.
        if plan.lifecycle.is_some() || !plan.wake.is_empty() || capacity_returned {
            self.changed.notify_waiters();
        }
        if plan.lifecycle.is_some()
            || plan.edits.values().any(|edit| {
                edit.after
                    .as_ref()
                    .is_some_and(|entry| WorkStage::for_phase(&entry.phase).is_some())
            })
        {
            self.work.notify_waiters();
        }
        drop(retired);
    }

    /// Validate the captured view and original reads before examining any new
    /// policy projection. The test observer marks this exact boundary.
    fn validate_original_cut(
        &self,
        plan: &Plan,
        view: &Guard<'_, View>,
        owners: &[(usize, Guard<'_, Shard>)],
    ) -> Result<(), Error> {
        let lifecycle_write = plan.lifecycle.is_some();
        if view.get().revision != plan.view {
            return Err(Error::Stale);
        }
        if lifecycle_write && view.get().revision == u64::MAX {
            return Err(Error::Fault("view counter"));
        }
        if self.chain_pending.load(Ordering::Acquire) && !lifecycle_write && !plan.edits.is_empty()
        {
            return Err(Error::Full(FullReason::ChainTransition));
        }
        if plan
            .lifecycle
            .as_ref()
            .is_some_and(|write| write.replace_generation)
            && (plan.reads.all.is_none()
                || plan.edits.values().any(|edit| edit.after.is_some())
                || plan.edits.len()
                    != owners
                        .iter()
                        .map(|(_, guard)| guard.get().owners.len())
                        .sum::<usize>())
        {
            return Err(Error::Fault("incomplete generation replacement"));
        }
        self.validate_reads(&plan.reads, owners)
    }

    /// Check policy fences and every derived row against the same guarded cut.
    /// The returned instant is also used by the committed ban-fence update.
    fn validate_proposed_cut(
        &self,
        plan: &Plan,
        changes: &OwnerChanges<'_>,
        owners: &[(usize, Guard<'_, Shard>)],
    ) -> Result<Instant, Error> {
        let now = Instant::now();
        if let Some((peer, expected)) = plan.peer_access
            && self
                .bans
                .lock()
                .get(&peer)
                .is_some_and(|until| *until > now)
                != expected
        {
            return Err(Error::Stale);
        }
        for edit in plan.edits.values() {
            if let Some(entry) = edit.after.as_deref()
                && let Some(peer) = entry.preaccepted_peer()
                && self
                    .bans
                    .lock()
                    .get(&peer)
                    .is_some_and(|deadline| *deadline > now)
            {
                return Err(Error::Full("peer is banned".into()));
            }
        }
        if let Some(page) = &plan.wake_advance {
            let row = self
                .relation(&RelationKey::Dependency(page.key.clone()))
                .ok_or(Error::Stale)?;
            if !page.row.ptr_eq(&Arc::downgrade(&row))
                || row
                    .lock()
                    .wake
                    .as_ref()
                    .is_none_or(|wake| wake.pass != page.pass)
            {
                return Err(Error::Stale);
            }
        }
        changes.validate_projections(self, plan, owners)?;
        Ok(now)
    }

    /// Replace every projection while the lifecycle writer owns all shards.
    /// Return the displaced generation for destruction after guard release.
    fn retire_generation(&self, owners: &mut [(usize, Guard<'_, Shard>)]) -> Retired {
        let mut retired = Retired {
            _committed: Some(std::mem::replace(
                &mut *self.committed.lock(),
                lru::LruCache::new(COMMITTED_HASH_CACHE_CAPACITY),
            )),
            ..Retired::default()
        };
        retired.shards.reserve_exact(SHARDS);
        retired.relations.reserve_exact(SHARDS);
        for (_, guard) in owners {
            if let Some(shard) = guard.get_mut() {
                retired.shards.push(std::mem::take(shard));
            }
        }
        for relations in &self.relations {
            retired
                .relations
                .push(std::mem::take(&mut *relations.lock()));
        }
        retired.peers = Some(std::mem::take(&mut *self.peers.lock()));
        retired.dirty = Some(std::mem::take(&mut *self.dirty.lock()));
        retired.queues = Some(self.queues.take());
        retired
    }

    /// Apply all owner-local indexes in shard order using the validated edits.
    fn apply_owner_changes(
        &self,
        owners: &mut [(usize, Guard<'_, Shard>)],
        edits: &[(usize, &Byte32, &Edit)],
        snapshot: &Snapshot,
        retired: &mut Vec<Arc<Entry>>,
    ) {
        let mut remaining_edits = edits;
        for (index, guard) in owners {
            let Some(shard) = guard.get_mut() else {
                continue;
            };
            let (edits, remaining) = remaining_edits.split_at(
                remaining_edits.partition_point(|(edit_index, _, _)| edit_index == index),
            );
            remaining_edits = remaining;
            for (_, hash, edit) in edits.iter().copied() {
                shard.apply_edit(hash, edit, snapshot, &self.queues, retired);
            }
        }
        debug_assert!(
            remaining_edits.is_empty(),
            "preflight covered every owner edit"
        );
    }

    /// Maintain peer membership and its observation marker as one projection.
    fn apply_peer_changes(&self, changes: BTreeMap<PeerIndex, MemberChanges<bool>>) {
        for (peer, changes) in changes {
            let mut peers = self.peers.lock();
            let row = peers
                .entry(peer)
                .or_insert_with(|| Arc::new(Mutex::new(Peer::default())));
            let mut row = row.lock();
            for (hash, change) in changes {
                if change.after {
                    row.members.insert(hash);
                } else {
                    row.members.remove(&hash);
                }
            }
            row.version = Arc::new(());
            let empty = row.members.is_empty();
            drop(row);
            if empty {
                peers.remove(&peer);
            }
        }
    }

    /// Extend the bounded peer-ban fence after owner and peer projections settle.
    fn apply_ban(&self, peer: PeerIndex, deadline: Instant, now: Instant) {
        let mut bans = self.bans.lock();
        bans.retain(|_, deadline| *deadline > now);
        if bans.len() >= crate::constants::PEER_BAN_FENCE_CAPACITY
            && !bans.contains_key(&peer)
            && let Some(oldest) = bans
                .iter()
                .min_by_key(|(_, deadline)| **deadline)
                .map(|(peer, _)| *peer)
        {
            bans.remove(&oldest);
        }
        bans.entry(peer)
            .and_modify(|current| *current = (*current).max(deadline))
            .or_insert(deadline);
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "Keyed routing modulo SHARDS indexes fixed arrays of exactly SHARDS buckets."
    )]
    fn apply_relation(
        &self,
        key: RelationKey,
        changes: MemberChanges<Roles>,
        edits: &BTreeMap<Byte32, Edit>,
        wake: &BTreeSet<DependencyKey>,
    ) {
        let mut collection = self.relations[self.route(&key)].lock();
        let row = collection
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(Relation::default())));
        let mut row = row.lock();
        for (hash, change) in &changes {
            if change.before.intersects(Roles::SPENDER) && row.spender.as_ref() == Some(hash) {
                row.spender = None;
            }
        }
        // Use the final spender, independent of the order of owner hashes.
        // A newly blocked history cannot recover during its creation event.
        let spent = row.spender.is_some()
            || changes
                .iter()
                .any(|(_, change)| change.after.intersects(Roles::SPENDER));
        let accepted_membership_changed = changes.iter().any(|(_, change)| {
            change.before.intersection(Roles::ACCEPTED)
                != change.after.intersection(Roles::ACCEPTED)
        });
        for (hash, change) in changes {
            let member_roles = change.after.intersection(Roles::MEMBERS);
            if member_roles.is_empty() {
                row.members.remove(&hash);
            } else {
                let deferred = matches!(&key, RelationKey::Dependency(key) if wake.contains(key))
                    && edits
                        .get(&hash)
                        .and_then(|edit| edit.after.as_ref())
                        .is_some_and(|entry| {
                            matches!(
                                entry.phase,
                                Phase::Replaced {
                                    require_all,
                                    ..
                                } if !require_all || spent
                            )
                        });
                let wait_after_pass = match row.next_pass.checked_add(u64::from(deferred)) {
                    Some(pass) => pass,
                    None => {
                        self.fault();
                        row.next_pass
                    }
                };
                row.members.insert(
                    hash.clone(),
                    RelationMember {
                        roles: member_roles,
                        wait_after_pass,
                    },
                );
            }
            if change.after.intersects(Roles::SPENDER) {
                row.spender = Some(hash);
            }
        }
        if accepted_membership_changed {
            row.accepted_version = Arc::new(());
        }
        if row.members.is_empty() {
            // Release the empty BTreeMap root leaf for a spender-only row.
            row.members.clear();
            if row.spender.is_none()
                && row.wake.take().is_some()
                && let RelationKey::Dependency(key) = &key
            {
                self.dirty.lock().remove(key);
            }
        }
        let empty = row.is_empty();
        drop(row);
        if empty {
            collection.remove(&key);
        }
    }
    #[expect(
        clippy::indexing_slicing,
        reason = "Keyed routing modulo SHARDS indexes fixed arrays of exactly SHARDS buckets."
    )]
    pub(super) fn start_wake(&self, key: &DependencyKey) {
        let relation_key = RelationKey::Dependency(key.clone());
        let collection = self.relations[self.route(&relation_key)].lock();
        let Some(row) = collection.get(&relation_key) else {
            return;
        };
        let mut row = row.lock();
        let was_queued = row.wake.is_some();
        let mut waiters = row
            .members
            .values()
            .filter(|member| member.roles.intersects(Roles::WAITING));
        let Some(first) = waiters.next() else {
            return;
        };
        if let Some(pass) = row.next_pass.checked_add(1) {
            let eligible = first.can_wake(pass) || waiters.any(|member| member.can_wake(pass));
            // Consume even a deferred event so the next event is eligible.
            row.next_pass = pass;
            if !eligible {
                return;
            }
            row.wake = Some(Wake { pass, after: None });
        } else {
            self.fault();
            return;
        }
        // Wake and dirty membership change under this same row lock. A newer
        // pass keeps the existing key and does not need another dirty lookup.
        if !was_queued {
            self.dirty.lock().insert(compact_dependency(key));
        }
    }
    #[expect(
        clippy::indexing_slicing,
        reason = "Keyed routing modulo SHARDS indexes fixed arrays of exactly SHARDS buckets."
    )]
    fn advance_wake(&self, page: &WakePage) {
        let key = RelationKey::Dependency(page.key.clone());
        let mut collection = self.relations[self.route(&key)].lock();
        let Some(row) = collection.get(&key) else {
            // Preflight validated the row; this commit retired its last member.
            return;
        };
        let mut row = row.lock();
        // This same commit may start a newer pass; an old cursor cannot erase it.
        if row.wake.as_ref().is_none_or(|wake| wake.pass != page.pass) {
            return;
        }
        let more = row
            .wake_candidates(page.pass, page.after.as_ref())
            .next()
            .is_some();
        if more {
            row.wake = Some(Wake {
                pass: page.pass,
                after: page.after.clone(),
            });
        } else {
            row.wake = None;
            self.dirty.lock().remove(&page.key);
        }
        let empty = row.is_empty();
        drop(row);
        if empty {
            collection.remove(&key);
        }
    }
}
