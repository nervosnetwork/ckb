//! Apply one prepared decision. Recoverable checks, the protected mutation and
//! lock release stay in one path; retired payloads outlive every authority guard.
use super::{
    ACCEPTED_ROLES, CHILD, DEP, Edit, Guard, INPUT, LockFootprint, Peer, Plan, Relation,
    RelationMember, SHARDS, Shard, Store, WAIT, Wake, WakePage, acquire, compact_dependency,
    compact_relation, proposal_key,
};
use crate::authority::{
    budget::OwnerDelta,
    model::{DependencyKey, Entry, Error, FullReason, Phase, RelationKey, Status},
    notice::{self, Batch},
    queue::Queues,
};
use crate::util::compact_packed;
use ckb_network::PeerIndex;
use ckb_snapshot::Snapshot;
use ckb_types::packed::Byte32;
use ckb_util::parking_lot::Mutex;
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Bound::{Excluded, Unbounded},
    sync::{Arc, atomic::Ordering},
    time::Instant,
};

// Unique owner hashes in order, each paired with its old and new membership.
type MemberChanges<T> = Vec<(Byte32, (T, T))>;

/// Keep recoverable Result/Option propagation out of the post-preflight tail.
/// This unit-return boundary does not claim panic or allocation-failure recovery.
fn commit_infallibly(commit: impl FnOnce()) {
    commit();
}

fn visit_roles(entry: &Entry, mut add: impl FnMut(RelationKey, u8)) {
    match &entry.phase {
        Phase::Accepted(accepted) => {
            for point in entry.transaction.input_pts_iter() {
                add(RelationKey::Dependency(DependencyKey::Cell(point)), INPUT);
            }
            for point in accepted.dependencies() {
                add(RelationKey::Dependency(DependencyKey::Cell(point)), DEP);
            }
            for hash in &accepted.parents {
                add(RelationKey::Children(hash.clone()), CHILD);
            }
        }
        Phase::Waiting(keys) | Phase::Replaced { triggers: keys, .. } => {
            for key in keys {
                add(RelationKey::Dependency(key.clone()), WAIT);
            }
        }
        Phase::Resolve | Phase::Verify(_) => {}
    }
}
// A roleless side cannot cancel a visited role. Stream that common transition
// directly; two role-bearing sides still need a bounded owner-local merge.
fn visit_role_changes(edit: &Edit, mut add: impl FnMut(RelationKey, u8, u8)) {
    let has_roles = |entry: &&Entry| !matches!(entry.phase, Phase::Resolve | Phase::Verify(_));
    let before = edit.before.as_deref().filter(has_roles);
    let after = edit.after.as_deref().filter(has_roles);
    match (before, after) {
        (None, Some(after)) => visit_roles(after, |key, role| add(key, 0, role)),
        (Some(before), None) => visit_roles(before, |key, role| add(key, role, 0)),
        (Some(before), Some(after)) => {
            let mut roles: BTreeMap<RelationKey, (u8, u8)> = BTreeMap::new();
            visit_roles(before, |key, role| roles.entry(key).or_default().0 |= role);
            visit_roles(after, |key, role| roles.entry(key).or_default().1 |= role);
            for (key, (old, new)) in roles {
                if old != new {
                    add(key, old, new);
                }
            }
        }
        (None, None) => {}
    }
}
fn peer(entry: &Entry) -> Option<PeerIndex> {
    entry
        .preaccepted()
        .then(|| entry.source.residency_peer())
        .flatten()
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
            orphan,
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
            *orphan -= usize::from(matches!(before.phase, Phase::Waiting(_)));
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
            if matches!(before.phase, Phase::Resolve | Phase::Verify(_)) {
                queues.remove(before);
            }
        }
        if let Some(after) = &edit.after {
            *orphan += usize::from(matches!(after.phase, Phase::Waiting(_)));
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
            if matches!(after.phase, Phase::Resolve | Phase::Verify(_)) {
                queues.insert(after);
            }
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

impl Store {
    pub(in crate::authority) fn apply(&self, mut plan: Plan) -> Result<Option<Arc<Batch>>, Error> {
        let mut notice = None;
        self.apply_plan(&mut plan, &mut notice)
    }

    /// Optional replacement history may lose capacity to another admission.
    /// Keep this plan's exact notices for one synchronous retry; no reservation
    /// or failed plan escapes to the service's asynchronous capacity wait.
    pub(in crate::authority) fn apply_admission(
        &self,
        mut plan: Plan,
        retain_history: &mut bool,
    ) -> Result<Option<Arc<Batch>>, Error> {
        let mut notice = None;
        match self.apply_plan(&mut plan, &mut notice) {
            Err(Error::Full(FullReason::Pipeline | FullReason::History)) if *retain_history => {
                *retain_history = false;
                if self.is_stopped() {
                    // Return to the caller's existing close/requeue handling.
                    return Err(Error::Stale);
                }
                for edit in plan.edits.values_mut() {
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
                self.apply_plan(&mut plan, &mut notice)
            }
            result => result,
        }
    }

    /// The private caller keeps a notice paired with its original plan and
    /// never changes the effects or class between attempts. No VM, provider,
    /// await, endpoint or payload destruction under the authority cut; all
    /// expected failures precede the first owner/index mutation.
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "Revision deltas are prechecked; exact validated owner edits preserve bounded phase counts."
    )]
    fn apply_plan(
        &self,
        plan: &mut Plan,
        notice: &mut Option<notice::Reservation>,
    ) -> Result<Option<Arc<Batch>>, Error> {
        #[cfg(feature = "profiling")]
        let _span = tracing::trace_span!(target: "ckb_tx_pool_profile", "tx_pool.authority.apply")
            .entered();
        if self.is_faulted() {
            return Err(Error::Fault("closed generation"));
        }
        let owner_delta = OwnerDelta::new(
            plan.edits
                .values()
                .filter_map(|edit| edit.before.as_deref()),
            plan.edits.values().filter_map(|edit| edit.after.as_deref()),
        )?;
        if notice.is_none() && !plan.dry_run {
            *notice = self
                .outbox
                .reserve(std::mem::take(&mut plan.effects), plan.class)?;
        }
        let mut owner_footprint = LockFootprint::default();
        let mut dependency_footprint = LockFootprint::default();
        let mut peer_footprint = LockFootprint::default();
        let mut owner_edits = Vec::with_capacity(plan.edits.len());
        // Plan hashes are unique and ordered; the last per-key change can merge
        // repeated roles of the current owner without another lookup tree.
        let mut delta: BTreeMap<RelationKey, MemberChanges<u8>> = BTreeMap::new();
        let mut peer_delta: BTreeMap<PeerIndex, MemberChanges<bool>> = BTreeMap::new();
        for (hash, edit) in &plan.edits {
            let index = self.owner_shard(hash);
            owner_footprint.insert(index, true);
            owner_edits.push((index, hash, edit));
            if plan.clear_all {
                continue;
            }
            visit_role_changes(edit, |key, old, new| {
                if let Some(changes) = delta.get_mut(&key) {
                    // Ordered owners keep duplicate roles in the final item.
                    if let Some((_, roles)) = changes.last_mut().filter(|(last, _)| last == hash) {
                        roles.0 |= old;
                        roles.1 |= new;
                    } else {
                        changes.push((hash.clone(), (old, new)));
                    }
                } else {
                    delta.insert(compact_relation(&key), vec![(hash.clone(), (old, new))]);
                }
                dependency_footprint.insert(self.route(&key), (old | new) & INPUT != 0);
            });
            let old_peer = edit.before.as_deref().and_then(peer);
            let new_peer = edit.after.as_deref().and_then(peer);
            if edit.after.is_none()
                && let Some(before) = edit.before.as_ref().filter(|entry| entry.preaccepted())
            {
                plan.wake.extend(
                    before
                        .transaction
                        .output_pts_iter()
                        .map(|point| DependencyKey::Cell(compact_packed(&point))),
                );
            }
            for peer in old_peer.into_iter().chain(new_peer) {
                peer_footprint.insert(self.route(&peer), false);
                if old_peer != new_peer {
                    peer_delta
                        .entry(peer)
                        .or_insert_with(|| Vec::with_capacity(1))
                        .push((
                            hash.clone(),
                            (old_peer == Some(peer), new_peer == Some(peer)),
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
                    plan.wake
                        .insert(DependencyKey::Cell(compact_packed(&point)));
                }
            }
        }
        // Route edits once before locking; both guarded passes retain the original
        // shard-major, full-hash order using this bounded borrowed scratch.
        owner_edits.sort_unstable_by_key(|(index, hash, _)| (*index, *hash));
        if plan.clear_all || plan.snapshot.is_some() {
            // The lifecycle writer already excludes ordinary commits. Refresh
            // proposal counts for unchanged owners in the same snapshot cut.
            for index in 0..SHARDS {
                owner_footprint.insert(index, true);
            }
        }
        for hash in plan.reads.owners.keys() {
            owner_footprint.insert(self.owner_shard(hash), false);
        }
        if plan.reads.all.is_some() || plan.reads.accepted.is_some() {
            for index in 0..SHARDS {
                owner_footprint.insert(index, false);
            }
        }
        for point in plan.reads.spenders.keys() {
            dependency_footprint.insert(
                self.route(&RelationKey::Dependency(DependencyKey::Cell(point.clone()))),
                false,
            );
        }
        for key in plan.reads.relations.keys() {
            dependency_footprint.insert(self.route(key), true);
        }
        for peer in plan.reads.peers.keys() {
            peer_footprint.insert(self.route(peer), true);
        }
        if let Some((peer, _)) = plan.peer_access {
            peer_footprint.insert(self.route(&peer), false);
        }
        if let Some((peer, _)) = plan.ban {
            peer_footprint.insert(self.route(&peer), true);
        }
        for key in &plan.wake {
            dependency_footprint.insert(self.route(&RelationKey::Dependency(key.clone())), true);
        }
        if let Some(page) = &plan.wake_advance {
            dependency_footprint
                .insert(self.route(&RelationKey::Dependency(page.key.clone())), true);
        }
        let lifecycle_write = plan.snapshot.is_some();
        let work_changed = lifecycle_write
            || plan.edits.values().any(|edit| {
                edit.after
                    .as_ref()
                    .is_some_and(|entry| matches!(entry.phase, Phase::Resolve | Phase::Verify(_)))
            });
        let template_changed = lifecycle_write || plan.edits.values().any(Edit::affects_accepted);
        if !plan.committed.is_empty() && !lifecycle_write {
            return Err(Error::Fault("committed hash source"));
        }
        #[cfg(test)]
        let observer = self.commit_observer.lock().clone();
        #[cfg(test)]
        if let Some(observer) = &observer {
            observer(plan, false);
        }
        #[cfg(feature = "profiling")]
        let acquire_span =
            tracing::trace_span!(target: "ckb_tx_pool_profile", "tx_pool.authority.acquire")
                .entered();
        let mut view = if lifecycle_write {
            Guard::Write(self.view.write())
        } else {
            Guard::Read(self.view.read())
        };
        let peer_guards = acquire(&self.peer_gates, &peer_footprint);
        let dependency_guards = acquire(&self.dependency_gates, &dependency_footprint);
        let mut owners = acquire(&self.shards, &owner_footprint);
        #[cfg(feature = "profiling")]
        drop(acquire_span);
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
        if plan.clear_all
            && (!lifecycle_write
                || plan.reads.all.is_none()
                || plan.edits.values().any(|edit| edit.after.is_some())
                || plan.edits.len()
                    != owners
                        .iter()
                        .map(|(_, guard)| guard.get().owners.len())
                        .sum::<usize>())
        {
            return Err(Error::Fault("incomplete generation replacement"));
        }
        self.validate_reads(&plan.reads, &owners)?;
        #[cfg(test)]
        if let Some(observer) = &observer {
            observer(plan, true);
        }
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
                && let Some(peer) = peer(entry)
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
        let mut remaining_edits = owner_edits.as_slice();
        for (index, guard) in &owners {
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
        // Projection contradictions are discovered before mutation. Point
        // readers may update different members under compatible dependency gates.
        for (key, changes) in &delta {
            let row = self.relation(key);
            let row = row.as_ref().map(|row| row.lock());
            let mut spender = row.as_ref().and_then(|row| row.spender.clone());
            for (hash, (old, _)) in changes {
                if row.as_ref().map_or(0, |row| row.roles(hash)) != *old {
                    return Err(Error::Fault("relation projection"));
                }
                if old & INPUT != 0 && spender.as_ref() == Some(hash) {
                    spender = None;
                }
            }
            for (hash, (_, new)) in changes {
                if new & INPUT != 0 {
                    if spender.as_ref().is_some_and(|old| old != hash) {
                        return Err(Error::Fault("multiple accepted spenders"));
                    }
                    spender = Some(hash.clone());
                }
            }
        }
        for key in &plan.wake {
            if let Some(row) = self.relation(&RelationKey::Dependency(key.clone())) {
                row.lock()
                    .next_pass
                    .checked_add(1)
                    .ok_or(Error::Fault("wake pass"))?;
            }
        }
        for (peer, changes) in &peer_delta {
            let row = self.peers.lock().get(peer).cloned();
            let row = row.as_ref().map(|row| row.lock());
            for (hash, (old, _)) in changes {
                if row.as_ref().is_some_and(|row| row.members.contains(hash)) != *old {
                    return Err(Error::Fault("peer projection"));
                }
            }
        }
        // Scalar capacity is reserved only after every owner observation is
        // validated. This reservation commits or drops before these guards open.
        // A sparse admission may lose room to a disjoint commit; replan its fee
        // policy against the now-settled population instead of rejecting it.
        let budget = owner_delta
            .reserve(&self.budget)
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
        let mut batch = None;
        commit_infallibly(|| {
            // All recoverable checks are complete. Collection and version-marker
            // allocation use Rust's abort-on-OOM platform contract.
            let mut retired = Vec::with_capacity(plan.edits.len());
            let mut retired_shards = Vec::new();
            let mut retired_relations = Vec::new();
            let mut retired_peers = None;
            let mut retired_dirty = None;
            let mut retired_queues = None;
            let mut retired_snapshot = None;
            let mut retired_committed = None;
            if plan.clear_all {
                retired_committed = Some(std::mem::replace(
                    &mut *self.committed.lock(),
                    lru::LruCache::new(100_000),
                ));
                retired_shards.reserve_exact(SHARDS);
                retired_relations.reserve_exact(SHARDS);
                for (_, guard) in &mut owners {
                    if let Some(shard) = guard.get_mut() {
                        retired_shards.push(std::mem::take(shard));
                    }
                }
                for relations in &self.relations {
                    retired_relations.push(std::mem::take(&mut *relations.lock()));
                }
                retired_peers = Some(std::mem::take(&mut *self.peers.lock()));
                retired_dirty = Some(std::mem::take(&mut *self.dirty.lock()));
                retired_queues = Some(self.queues.take());
            } else {
                let mut remaining_edits = owner_edits.as_slice();
                for (index, guard) in &mut owners {
                    let Some(shard) = guard.get_mut() else {
                        continue;
                    };
                    let (edits, remaining) = remaining_edits.split_at(
                        remaining_edits.partition_point(|(edit_index, _, _)| edit_index == index),
                    );
                    remaining_edits = remaining;
                    for (_, hash, edit) in edits.iter().copied() {
                        shard.apply_edit(
                            hash,
                            edit,
                            &view.get().snapshot,
                            &self.queues,
                            &mut retired,
                        );
                    }
                }
            }
            for (key, changes) in delta {
                self.apply_relation(key, changes, &plan.edits, &plan.wake);
            }
            for (peer, changes) in peer_delta {
                let mut peers = self.peers.lock();
                let row = peers
                    .entry(peer)
                    .or_insert_with(|| Arc::new(Mutex::new(Peer::default())));
                let mut row = row.lock();
                for (hash, (_, new)) in changes {
                    if new {
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
            if let Some((peer, deadline)) = plan.ban {
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
            for key in &plan.wake {
                self.start_wake(key);
            }
            if let Some(page) = &plan.wake_advance {
                self.advance_wake(page);
            }
            if let Some(snapshot) = &plan.snapshot {
                for (_, guard) in &mut owners {
                    if let Some(shard) = guard.get_mut() {
                        shard.refresh_proposed(snapshot);
                    }
                }
            }
            if let Some(view) = view.get_mut() {
                if let Some(snapshot) = plan.snapshot.take() {
                    retired_snapshot = Some(std::mem::replace(&mut view.snapshot, snapshot));
                }
                view.revision += 1;
            }
            if !plan.committed.is_empty() {
                let mut cache = self.committed.lock();
                for (proposal, hash) in std::mem::take(&mut plan.committed) {
                    cache.put(proposal, hash);
                }
            }
            batch = notice.take().map(notice::Reservation::append);
            let capacity_returned = budget.commit();
            drop(owners);
            drop(dependency_guards);
            drop(peer_guards);
            drop(view);
            if let Some(batch) = &batch {
                batch.activate(&self.outbox);
            }
            if capacity_returned {
                self.budget.changed.notify_waiters();
            }
            if template_changed {
                self.template_changed.notify_waiters();
            }
            // Queue-only transitions wake workers below. Maintenance needs
            // dependency, capacity or lifecycle progress instead.
            if lifecycle_write || !plan.wake.is_empty() || capacity_returned {
                self.changed.notify_waiters();
            }
            if work_changed {
                self.work.notify_waiters();
            }
            drop((
                retired,
                retired_shards,
                retired_relations,
                retired_peers,
                retired_dirty,
                retired_queues,
                retired_snapshot,
                retired_committed,
            ));
        });
        if self.is_faulted() {
            return Err(Error::Fault("committed projection"));
        }
        Ok(batch)
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "Keyed routing modulo SHARDS indexes fixed arrays of exactly SHARDS buckets."
    )]
    fn apply_relation(
        &self,
        key: RelationKey,
        changes: MemberChanges<u8>,
        edits: &BTreeMap<Byte32, Edit>,
        wake: &BTreeSet<DependencyKey>,
    ) {
        let mut collection = self.relations[self.route(&key)].lock();
        let row = collection
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(Relation::default())));
        let mut row = row.lock();
        for (hash, (old, _)) in &changes {
            if old & INPUT != 0 && row.spender.as_ref() == Some(hash) {
                row.spender = None;
            }
        }
        // Use the final spender, independent of the order of owner hashes.
        // A newly blocked history cannot recover during its creation event.
        let spent = row.spender.is_some() || changes.iter().any(|(_, (_, new))| new & INPUT != 0);
        let accepted_changed = changes
            .iter()
            .any(|(_, (old, new))| old & ACCEPTED_ROLES != new & ACCEPTED_ROLES);
        for (hash, (_, new)) in changes {
            let other_roles = new & !INPUT;
            if other_roles == 0 {
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
                        self.faulted.store(true, Ordering::Release);
                        row.next_pass
                    }
                };
                row.members.insert(
                    hash.clone(),
                    RelationMember {
                        roles: other_roles,
                        wait_after_pass,
                    },
                );
            }
            if new & INPUT != 0 {
                row.spender = Some(hash);
            }
        }
        if accepted_changed {
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
            .filter(|member| member.roles & WAIT != 0);
        let Some(first) = waiters.next() else {
            return;
        };
        if let Some(pass) = row.next_pass.checked_add(1) {
            let eligible =
                pass > first.wait_after_pass || waiters.any(|member| pass > member.wait_after_pass);
            // Consume even a deferred event so the next event is eligible.
            row.next_pass = pass;
            if !eligible {
                return;
            }
            row.wake = Some(Wake { pass, after: None });
        } else {
            self.faulted.store(true, Ordering::Release);
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
            .members
            .range((page.after.as_ref().map_or(Unbounded, Excluded), Unbounded))
            .any(|(_, flags)| flags.roles & WAIT != 0 && page.pass > flags.wait_after_pass);
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
