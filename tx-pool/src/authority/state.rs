//! Single-writer in-memory transaction-pool authority state.
//!
//! `AuthorityState` owns every live owner, derived index, live owner accounting,
//! global eviction lower bounds, pending ranks and the paired chain snapshot.
//! Every mutation runs in pure memory under one short write guard and returns
//! displaced payloads in `Retired` for destruction after lock release.

use super::{
    budget::{Account, AccountUsage, Amount, Limits},
    eviction::EvictionRank,
    membership::Aggregate,
    model::{
        Entry, Error, OutPointKey, Phase, Premises, Status, hash_key, matches_weak, proposal_key,
    },
    queue::{Queues, WorkStage},
    ranks::{Priority, RankedPriorities},
};
use ckb_network::PeerIndex;
use ckb_snapshot::Snapshot;
use ckb_types::{core::BlockView, packed::ProposalShortId, prelude::*};
use std::{
    collections::{BTreeSet, HashMap, HashSet, VecDeque, hash_map::Entry as MapEntry},
    hash::Hash,
    sync::Arc,
    time::Instant,
};

mod descendants;
mod waiters;

use waiters::for_each_derived_wake_key;
pub(super) use waiters::{WAKE_PAGE, WaitRow};

pub(super) const COMMITTED_HASH_CACHE_CAPACITY: usize = 100_000;
pub(super) type CommittedHashes = lru::LruCache<[u8; ProposalShortId::TOTAL_SIZE], [u8; 32]>;

/// Fast zero-dependency hasher for Blake2b-256 digest keys (`[u8; 32]`,
/// 10-byte proposal prefixes, and `OutPointKey`). Taking an 8-byte word
/// directly avoids SipHash overhead without exposing hash-collision DoS.
#[derive(Clone, Copy, Default)]
pub(super) struct HashKeyBuildHasher;

pub(super) struct HashKeyHasher(u64);

impl std::hash::BuildHasher for HashKeyBuildHasher {
    type Hasher = HashKeyHasher;

    #[inline]
    fn build_hasher(&self) -> Self::Hasher {
        HashKeyHasher(0)
    }
}

impl std::hash::Hasher for HashKeyHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }

    #[inline]
    fn write_u64(&mut self, value: u64) {
        self.0 ^= value;
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        if let Some(prefix) = bytes.first_chunk::<8>() {
            self.0 ^= u64::from_le_bytes(*prefix);
        } else {
            let mut buf = [0u8; 8];
            if let Some(dst) = buf.get_mut(..bytes.len()) {
                dst.copy_from_slice(bytes);
            }
            self.0 ^= u64::from_le_bytes(buf);
        }
    }
}

pub(super) type FastHashMap<K, V> = HashMap<K, V, HashKeyBuildHasher>;
pub(super) type FastHashSet<K> = HashSet<K, HashKeyBuildHasher>;

/// Compact reader/child set: zero heap allocation for the common singleton
/// case and O(1) insert/remove for heavily shared system cell-deps.
#[derive(Debug)]
pub(super) enum ReaderSet {
    One([u8; 32]),
    Many(FastHashSet<[u8; 32]>),
}

impl ReaderSet {
    fn insert(&mut self, hash: [u8; 32]) {
        match self {
            Self::One(existing) if *existing != hash => {
                let mut set = FastHashSet::with_capacity_and_hasher(4, HashKeyBuildHasher);
                set.insert(*existing);
                set.insert(hash);
                *self = Self::Many(set);
            }
            Self::One(_) => {}
            Self::Many(set) => {
                set.insert(hash);
            }
        }
    }

    /// Remove `hash` and return `true` if the set is now empty.
    fn remove(&mut self, hash: &[u8; 32]) -> bool {
        match self {
            Self::One(existing) => existing == hash,
            Self::Many(set) => {
                set.remove(hash);
                set.is_empty()
            }
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        matches!(self, Self::Many(set) if set.is_empty())
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &[u8; 32]> + '_ {
        let (one, many) = match self {
            Self::One(hash) => (Some(hash), None),
            Self::Many(set) => (None, Some(set.iter())),
        };
        one.into_iter().chain(many.into_iter().flatten())
    }
}

fn insert_reader<K: Eq + Hash>(map: &mut FastHashMap<K, ReaderSet>, key: K, hash: [u8; 32]) {
    map.entry(key)
        .and_modify(|readers| readers.insert(hash))
        .or_insert(ReaderSet::One(hash));
}

fn clear_if_owner<K: Eq + Hash>(map: &mut FastHashMap<K, [u8; 32]>, key: K, hash: [u8; 32]) {
    if let MapEntry::Occupied(entry) = map.entry(key)
        && *entry.get() == hash
    {
        entry.remove();
    }
}

fn remove_reader<K: Eq + Hash>(map: &mut FastHashMap<K, ReaderSet>, key: K, hash: &[u8; 32]) {
    if let MapEntry::Occupied(mut entry) = map.entry(key)
        && entry.get_mut().remove(hash)
    {
        entry.remove();
    }
}

/// Displaced payloads kept alive until the authority write lock is released.
#[derive(Default)]
pub(super) struct Retired {
    pub(super) owners: Vec<Arc<Entry>>,
    pub(super) state: Option<Box<AuthorityState>>,
    pub(super) queues: Option<Queues>,
    pub(super) snapshot: Option<Arc<Snapshot>>,
}

/// Summary of post-lock notifications required after a committed mutation.
#[derive(Default)]
pub(super) struct CommitOutcome {
    pub(super) template_changed: bool,
    pub(super) maintenance_ready: bool,
    pub(super) work_ready: bool,
    pub(super) retired: Retired,
}

#[derive(Clone, Copy)]
struct PreservedIndexKeys {
    deadline: bool,
    peer: bool,
    timestamp: bool,
}

/// One validated owner transition passed into `AuthorityState::apply_edits`.
pub(super) struct OwnerEdit {
    pub(super) hash: [u8; 32],
    pub(super) before: Option<Arc<Entry>>,
    pub(super) after: Option<Arc<Entry>>,
    /// Precomputed ancestor aggregate when admitting a new accepted owner.
    pub(super) ancestors: Option<Aggregate>,
}

impl OwnerEdit {
    pub(super) fn new(
        hash: [u8; 32],
        before: Option<Arc<Entry>>,
        after: Option<Arc<Entry>>,
    ) -> Self {
        Self {
            hash,
            before,
            after,
            ancestors: None,
        }
    }

    pub(super) fn remove(hash: [u8; 32], before: Arc<Entry>) -> Self {
        Self::new(hash, Some(before), None)
    }

    pub(super) fn insert(hash: [u8; 32], after: Arc<Entry>) -> Self {
        Self::new(hash, None, Some(after))
    }

    pub(super) fn affects_accepted(&self) -> bool {
        self.before.as_ref().is_some_and(|e| e.accepted().is_some())
            || self.after.as_ref().is_some_and(|e| e.accepted().is_some())
    }
}

/// Stack-first batch of owner edits so common single-owner commits allocate
/// zero `Vec<OwnerEdit>` heap memory.
pub(super) enum EditBatch {
    One(Option<OwnerEdit>),
    Many(Vec<OwnerEdit>),
}

impl EditBatch {
    pub(super) fn as_slice(&self) -> &[OwnerEdit] {
        match self {
            Self::One(Some(edit)) => std::slice::from_ref(edit),
            Self::One(None) => &[],
            Self::Many(edits) => edits.as_slice(),
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }
}

pub(super) struct AuthorityState {
    pub(super) snapshot: Arc<Snapshot>,
    pub(super) view: u64,
    /// Monotonic counter bumped on every owner, lifecycle or generation mutation
    /// so optimistic two-phase reconciliation detects intervening writes in O(1).
    pub(super) mutation_epoch: u64,
    /// Monotonic counter bumped on any mutation other than a pure accepted-leaf
    /// insertion so forward-tip reconciliation can safely validate disjoint leaf
    /// admissions without freezing writers behind `ReorgGate`.
    pub(super) structural_epoch: u64,
    pub(super) recent_leaf_inserts: VecDeque<[u8; 32]>,
    pub(super) owners: FastHashMap<[u8; 32], Arc<Entry>>,
    pub(super) proposals: FastHashMap<[u8; ProposalShortId::TOTAL_SIZE], [u8; 32]>,
    pub(super) spenders: FastHashMap<OutPointKey, [u8; 32]>,
    pub(super) cell_deps: FastHashMap<OutPointKey, ReaderSet>,
    pub(super) children: FastHashMap<[u8; 32], ReaderSet>,
    /// Live descendant closure aggregates for accepted owners with at least one
    /// accepted child (`count > 1`). Leaves are omitted (`Aggregate::one`).
    pub(super) descendants: FastHashMap<[u8; 32], Aggregate>,
    pub(super) waiters: FastHashMap<OutPointKey, WaitRow>,
    pub(super) dirty_wakes: BTreeSet<OutPointKey>,
    /// Monotonic high-water mark across retired `WaitRow` lifetimes so a newly
    /// recreated row never aliases an in-flight wake page's pass number.
    pub(super) wake_epoch: u64,
    pub(super) waiting_owners: BTreeSet<[u8; 32]>,
    pub(super) preaccepted_owners: FastHashSet<[u8; 32]>,
    pub(super) peer_members: HashMap<PeerIndex, BTreeSet<[u8; 32]>>,
    pub(super) bans: HashMap<PeerIndex, Instant>,
    pub(super) deadlines: BTreeSet<(Instant, [u8; 32])>,
    pub(super) accepted_times: BTreeSet<(u64, [u8; 32])>,
    pub(super) usage: AccountUsage,
    pub(super) gap_count: usize,
    pub(super) proposed_count: usize,
    pub(super) accepted_revision: u64,
    pub(super) accepted_removal_revision: u64,
    pub(super) eviction_bounds: BTreeSet<EvictionRank>,
    pub(super) pending_ranks: RankedPriorities,
    pub(super) rank_generation: u64,
    pub(super) committed: CommittedHashes,
}

impl AuthorityState {
    pub(super) fn new(snapshot: Arc<Snapshot>) -> Self {
        Self {
            snapshot,
            view: 0,
            mutation_epoch: 0,
            structural_epoch: 0,
            recent_leaf_inserts: VecDeque::new(),
            owners: FastHashMap::default(),
            proposals: FastHashMap::default(),
            spenders: FastHashMap::default(),
            cell_deps: FastHashMap::default(),
            children: FastHashMap::default(),
            descendants: FastHashMap::default(),
            waiters: FastHashMap::default(),
            dirty_wakes: BTreeSet::new(),
            wake_epoch: 0,
            waiting_owners: BTreeSet::new(),
            preaccepted_owners: FastHashSet::default(),
            peer_members: HashMap::new(),
            bans: HashMap::new(),
            deadlines: BTreeSet::new(),
            accepted_times: BTreeSet::new(),
            usage: AccountUsage::default(),
            gap_count: 0,
            proposed_count: 0,
            accepted_revision: 0,
            accepted_removal_revision: 0,
            eviction_bounds: BTreeSet::new(),
            pending_ranks: RankedPriorities::invalid(),
            rank_generation: 0,
            committed: CommittedHashes::new(COMMITTED_HASH_CACHE_CAPACITY),
        }
    }

    pub(super) fn owner(&self, hash: &[u8; 32]) -> Option<&Arc<Entry>> {
        self.owners.get(hash)
    }

    pub(super) fn proposal_owner(
        &self,
        id: &[u8; ProposalShortId::TOTAL_SIZE],
    ) -> Option<&Arc<Entry>> {
        self.owners.get(self.proposals.get(id)?)
    }

    pub(super) fn is_peer_banned(&self, peer: PeerIndex, now: Instant) -> bool {
        self.bans.get(&peer).is_some_and(|until| *until > now)
    }

    pub(super) fn validate_premises(&self, view: u64, premises: &Premises) -> Result<(), Error> {
        if self.view != view {
            return Err(Error::Stale);
        }
        for (hash, expected) in &premises.owners {
            if !matches_weak(expected, self.owners.get(hash)) {
                return Err(Error::Stale);
            }
        }
        for (point, expected) in &premises.spenders {
            if self.spenders.get(point).copied() != *expected {
                return Err(Error::Stale);
            }
        }
        Ok(())
    }

    /// Collect all accepted input and cell-dep readers of `point` into `out` in O(k).
    /// `ReaderSet` already deduplicates `cell_deps`, so at most the sole `spender`
    /// can overlap with it.
    pub(super) fn accepted_readers(&self, point: &OutPointKey, out: &mut Vec<[u8; 32]>) {
        let spender = self.spenders.get(point).copied();
        if let Some(spender) = spender {
            out.push(spender);
        }
        if let Some(deps) = self.cell_deps.get(point) {
            for reader in deps.iter() {
                if spender != Some(*reader) {
                    out.push(*reader);
                }
            }
        }
    }

    /// Check all recoverable preflight conditions BEFORE reserving outbox
    /// notices or mutating live state.
    pub(super) fn preflight_edits(
        &self,
        edits: &[OwnerEdit],
        limits: &Limits,
        now: Instant,
        wake_keys: &[OutPointKey],
    ) -> Result<(), Error> {
        // Single-edit and pure-removal commits allocate zero scratch maps.
        let multi = edits.len() > 1;
        let has_after = edits.iter().any(|edit| edit.after.is_some());
        let track_multi = multi && has_after;
        let mut accepted_edits = 0u64;
        let mut removed_accepted = 0u64;
        let mut removing_hashes = FastHashSet::default();
        let mut releasing_accepted = FastHashSet::default();
        for edit in edits {
            if edit.affects_accepted() {
                accepted_edits = accepted_edits.saturating_add(1);
            }
            let before_accepted = edit.before.as_ref().is_some_and(|e| e.accepted().is_some());
            if before_accepted {
                removed_accepted = removed_accepted.saturating_add(1);
            }
            if track_multi {
                if edit.after.is_none() {
                    removing_hashes.insert(edit.hash);
                }
                if before_accepted {
                    releasing_accepted.insert(edit.hash);
                }
            }
        }
        self.accepted_revision
            .checked_add(accepted_edits)
            .ok_or(Error::Fault("accepted revision"))?;
        self.accepted_removal_revision
            .checked_add(removed_accepted)
            .ok_or(Error::Fault("accepted removal revision"))?;
        if has_after {
            let mut new_proposals = FastHashMap::default();
            let mut new_spenders = FastHashMap::default();
            for edit in edits {
                let Some(after) = &edit.after else {
                    continue;
                };
                let proposal = proposal_key(&edit.hash);
                if self
                    .proposals
                    .get(&proposal)
                    .is_some_and(|e| *e != edit.hash && (!multi || !removing_hashes.contains(e)))
                    || (multi
                        && new_proposals
                            .insert(proposal, edit.hash)
                            .is_some_and(|prev| prev != edit.hash))
                {
                    return Err(Error::Full("proposal short-ID collision".into()));
                }
                if after
                    .preaccepted_peer()
                    .is_some_and(|peer| self.is_peer_banned(peer, now))
                {
                    return Err(Error::Full("peer is banned".into()));
                }
                if let Some(accepted) = after.accepted() {
                    for cell in &accepted.transaction.resolved_inputs {
                        let key = OutPointKey::from_out_point(&cell.out_point);
                        if self.spenders.get(&key).is_some_and(|e| {
                            *e != edit.hash && (!multi || !releasing_accepted.contains(e))
                        }) || (multi
                            && new_spenders
                                .insert(key, edit.hash)
                                .is_some_and(|prev| prev != edit.hash))
                        {
                            return Err(Error::Fault("multiple accepted spenders"));
                        }
                    }
                }
            }
        }
        if let [edit] = edits {
            self.usage
                .check_single(edit.before.as_deref(), edit.after.as_deref(), limits)?;
        } else {
            self.usage.check_delta(
                edits.iter().filter_map(|edit| edit.before.as_deref()),
                edits.iter().filter_map(|edit| edit.after.as_deref()),
                limits,
            )?;
        }

        if !self.waiters.is_empty() {
            let check_key = |key: &OutPointKey| -> Result<(), Error> {
                if let Some(row) = self.waiters.get(key) {
                    row.next_pass
                        .checked_add(1)
                        .ok_or(Error::Fault("wake pass"))?;
                }
                Ok(())
            };
            for key in wake_keys {
                check_key(key)?;
            }
            for_each_derived_wake_key(edits, |key| check_key(&key))?;
            for edit in edits {
                if let Some(after) = &edit.after
                    && let Phase::Replaced(triggers) = &after.phase
                {
                    for key in triggers.keys() {
                        check_key(&OutPointKey::from_out_point(key))?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Apply a preflighted batch of owner edits infallibly in three order-independent passes:
    /// 1. Detach all `before` rows and install `after` owners + accepted graph indexes.
    /// 2. Register new `Waiting`/`Replaced` rows against the final `spenders` table.
    /// 3. Fire dependency wakes when waiters exist.
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "Preflight validated revision bounds and exact owner membership bounds phase counts."
    )]
    pub(super) fn apply_edits(
        &mut self,
        edits: EditBatch,
        mut extra_wake_keys: Vec<OutPointKey>,
        queues: &Queues,
        outcome: &mut CommitOutcome,
    ) -> Result<(), Error> {
        let edit_slice = edits.as_slice();
        let before_fixed = self.usage.fixed_snapshot();
        let mut first_peer: Option<(PeerIndex, Amount)> = None;
        let mut extra_peers: Vec<(PeerIndex, Amount)> = Vec::new();
        for peer in edit_slice
            .iter()
            .filter_map(|e| e.before.as_deref().and_then(Entry::preaccepted_peer))
        {
            match first_peer {
                None => first_peer = Some((peer, self.usage.get(Account::Peer(peer)))),
                Some((first, _))
                    if first != peer && !extra_peers.iter().any(|(p, _)| *p == peer) =>
                {
                    extra_peers.push((peer, self.usage.get(Account::Peer(peer))));
                }
                _ => {}
            }
        }

        let mut had_waiters = !self.waiters.is_empty();
        let creating_waiters = edit_slice.iter().any(|edit| {
            edit.after
                .as_ref()
                .is_some_and(|e| matches!(e.phase, Phase::Waiting(_) | Phase::Replaced(_)))
        });
        if creating_waiters {
            for_each_derived_wake_key(edit_slice, |key| {
                extra_wake_keys.push(key);
                Ok(())
            })?;
        } else if had_waiters {
            let waiters = &self.waiters;
            for_each_derived_wake_key(edit_slice, |key| {
                if waiters.contains_key(&key) {
                    extra_wake_keys.push(key);
                }
                Ok(())
            })?;
        }

        let descendants_update = self.prepare_descendants_update(edit_slice)?;
        let pure_leaf_insert = if extra_wake_keys.is_empty() && !had_waiters {
            match edit_slice {
                [edit]
                    if edit.before.is_none()
                        && edit
                            .after
                            .as_ref()
                            .and_then(|e| e.accepted())
                            .is_some_and(|v| v.parents.is_empty()) =>
                {
                    Some(edit.hash)
                }
                _ => None,
            }
        } else {
            None
        };

        let mut new_wait_entries: Vec<([u8; 32], Arc<Entry>)> = Vec::new();
        let mut rank_dirty = false;
        let mut single_affected: Option<[u8; 32]> = None;
        let mut extra_affected: Vec<[u8; 32]> = Vec::new();

        let (one, many) = match edits {
            EditBatch::One(edit) => (edit, Vec::new().into_iter()),
            EditBatch::Many(edits) => (None, edits.into_iter()),
        };
        for OwnerEdit {
            hash,
            before,
            after,
            ancestors,
        } in one.into_iter().chain(many)
        {
            if let (Some(old), Some(new)) = (&before, &after)
                && Arc::ptr_eq(old, new)
            {
                continue;
            }
            let before_accepted = before.as_ref().is_some_and(|e| e.accepted().is_some());
            let after_accepted = after.as_ref().is_some_and(|e| e.accepted().is_some());
            let affects_accepted = before_accepted || after_accepted;
            if affects_accepted {
                outcome.template_changed = true;
                if self
                    .pending_ranks
                    .is_current(self.rank_generation, usize::MAX)
                {
                    self.pending_ranks.update(
                        self.rank_generation,
                        &hash,
                        before.as_deref(),
                        after.as_deref(),
                        &self.snapshot,
                        ancestors,
                    );
                }
                if before_accepted && after_accepted {
                    rank_dirty = true;
                } else if !rank_dirty {
                    if single_affected.is_none() {
                        single_affected = Some(hash);
                    } else {
                        extra_affected.push(hash);
                    }
                }
            }
            if after
                .as_ref()
                .is_some_and(|e| WorkStage::for_phase(&e.phase).is_some())
            {
                outcome.work_ready = true;
            }

            let preserved = PreservedIndexKeys {
                deadline: before.as_deref().and_then(Entry::preaccepted_deadline)
                    == after.as_deref().and_then(Entry::preaccepted_deadline),
                peer: before.as_deref().and_then(Entry::preaccepted_peer)
                    == after.as_deref().and_then(Entry::preaccepted_peer),
                timestamp: before
                    .as_deref()
                    .and_then(Entry::accepted)
                    .map(|a| a.timestamp)
                    == after
                        .as_deref()
                        .and_then(Entry::accepted)
                        .map(|a| a.timestamp),
            };
            self.usage.apply_edit(before.as_deref(), after.as_deref())?;

            if let Some(old) = &before {
                self.detach_owner(hash, old, after.is_none(), preserved, queues, outcome);
            }
            if let Some(new) = after {
                self.attach_owner(
                    hash,
                    new,
                    before.is_none(),
                    preserved,
                    queues,
                    &mut new_wait_entries,
                    outcome,
                );
            }
            if affects_accepted {
                self.accepted_revision += 1;
            }
        }

        if let Some(update) = descendants_update {
            self.finish_descendants_update(update)?;
        }

        if rank_dirty
            || single_affected
                .into_iter()
                .chain(extra_affected)
                .any(|hash| self.children.contains_key(&hash))
        {
            self.rank_generation = self.rank_generation.saturating_add(1);
            self.pending_ranks.invalidate();
        }

        if !new_wait_entries.is_empty() {
            let needs_wake_set = !extra_wake_keys.is_empty()
                && new_wait_entries
                    .iter()
                    .any(|(_, entry)| matches!(entry.phase, Phase::Replaced(_)));
            let wake_set: FastHashSet<OutPointKey> = if needs_wake_set {
                extra_wake_keys.iter().copied().collect()
            } else {
                FastHashSet::default()
            };
            for (hash, entry) in new_wait_entries {
                match &entry.phase {
                    Phase::Waiting(keys) => {
                        for key in keys {
                            self.add_waiter(OutPointKey::from_out_point(key), hash, false)?;
                            had_waiters = true;
                        }
                    }
                    Phase::Replaced(triggers) => {
                        for key in triggers.keys() {
                            let point_key = OutPointKey::from_out_point(key);
                            let spent = self.spenders.contains_key(&point_key);
                            let deferred = wake_set.contains(&point_key)
                                && triggers.defer_creation_event(spent);
                            self.add_waiter(point_key, hash, deferred)?;
                            had_waiters = true;
                        }
                    }
                    _ => {}
                }
            }
        }

        if had_waiters && !extra_wake_keys.is_empty() && !self.waiters.is_empty() {
            let mut seen_wakes = FastHashSet::default();
            for key in extra_wake_keys {
                if self.waiters.contains_key(&key)
                    && seen_wakes.insert(key)
                    && self.start_wake(&key)?
                {
                    outcome.maintenance_ready = true;
                }
            }
        }

        if self
            .usage
            .has_returned_capacity_since(before_fixed, first_peer.into_iter().chain(extra_peers))
        {
            outcome.maintenance_ready = true;
        }
        const MAX_RECENT_LEAF_INSERTS: usize = 256;
        self.mutation_epoch = self.mutation_epoch.wrapping_add(1);
        if let Some(hash) = pure_leaf_insert {
            if self.recent_leaf_inserts.len() == MAX_RECENT_LEAF_INSERTS {
                self.recent_leaf_inserts.pop_front();
            }
            self.recent_leaf_inserts.push_back(hash);
        } else {
            self.structural_epoch = self.structural_epoch.wrapping_add(1);
            self.recent_leaf_inserts.clear();
        }
        Ok(())
    }

    #[expect(
        clippy::arithmetic_side_effects,
        reason = "Preflight validated revision bounds and exact owner membership bounds phase counts."
    )]
    fn detach_owner(
        &mut self,
        hash: [u8; 32],
        old: &Arc<Entry>,
        removing: bool,
        preserved: PreservedIndexKeys,
        queues: &Queues,
        outcome: &mut CommitOutcome,
    ) {
        if WorkStage::for_phase(&old.phase).is_some() {
            queues.remove(old);
        }
        if old.preaccepted() {
            self.preaccepted_owners.remove(&hash);
        }
        if matches!(old.phase, Phase::Waiting(_)) {
            self.waiting_owners.remove(&hash);
        }
        if !preserved.deadline
            && let Some(deadline) = old.preaccepted_deadline()
        {
            self.deadlines.remove(&(deadline, hash));
        }
        if !preserved.peer
            && let Some(peer) = old.preaccepted_peer()
            && let Some(members) = self.peer_members.get_mut(&peer)
        {
            members.remove(&hash);
            if members.is_empty() {
                self.peer_members.remove(&peer);
            }
        }
        match &old.phase {
            Phase::Accepted(accepted) => {
                let status = accepted.status(&self.snapshot);
                self.gap_count -= usize::from(status == Status::Gap);
                self.proposed_count -= usize::from(status == Status::Proposed);
                if !preserved.timestamp {
                    self.accepted_times.remove(&(accepted.timestamp, hash));
                }
                if let Some(bound) = EvictionRank::lower_bound(old) {
                    self.eviction_bounds.remove(&bound);
                }
                for cell in &accepted.transaction.resolved_inputs {
                    clear_if_owner(
                        &mut self.spenders,
                        OutPointKey::from_out_point(&cell.out_point),
                        hash,
                    );
                }
                for point in accepted.dependencies() {
                    remove_reader(
                        &mut self.cell_deps,
                        OutPointKey::from_out_point(point),
                        &hash,
                    );
                }
                for parent in &accepted.parents {
                    remove_reader(&mut self.children, hash_key(parent), &hash);
                }
                self.accepted_removal_revision += 1;
            }
            Phase::Waiting(keys) => self.remove_waiter_keys(&hash, keys),
            Phase::Replaced(triggers) => self.remove_waiter_keys(&hash, triggers.keys()),
            Phase::Resolve | Phase::Verify(_) => {}
        }
        if removing {
            clear_if_owner(&mut self.proposals, proposal_key(&hash), hash);
            if let Some(retired) = self.owners.remove(&hash) {
                outcome.retired.owners.push(retired);
            }
        }
    }

    #[expect(
        clippy::arithmetic_side_effects,
        reason = "Exact owner membership bounds phase counts."
    )]
    #[allow(clippy::too_many_arguments)]
    fn attach_owner(
        &mut self,
        hash: [u8; 32],
        new: Arc<Entry>,
        is_new: bool,
        preserved: PreservedIndexKeys,
        queues: &Queues,
        new_wait_entries: &mut Vec<([u8; 32], Arc<Entry>)>,
        outcome: &mut CommitOutcome,
    ) {
        if new.preaccepted() {
            self.preaccepted_owners.insert(hash);
        }
        if matches!(new.phase, Phase::Waiting(_)) {
            self.waiting_owners.insert(hash);
        }
        if !preserved.deadline
            && let Some(deadline) = new.preaccepted_deadline()
        {
            self.deadlines.insert((deadline, hash));
        }
        if !preserved.peer
            && let Some(peer) = new.preaccepted_peer()
        {
            self.peer_members.entry(peer).or_default().insert(hash);
        }
        match &new.phase {
            Phase::Accepted(accepted) => {
                let status = accepted.status(&self.snapshot);
                self.gap_count += usize::from(status == Status::Gap);
                self.proposed_count += usize::from(status == Status::Proposed);
                if !preserved.timestamp {
                    self.accepted_times.insert((accepted.timestamp, hash));
                }
                if let Some(bound) = EvictionRank::lower_bound(&new) {
                    self.eviction_bounds.insert(bound);
                }
                for cell in &accepted.transaction.resolved_inputs {
                    self.spenders
                        .insert(OutPointKey::from_out_point(&cell.out_point), hash);
                }
                for point in accepted.dependencies() {
                    insert_reader(
                        &mut self.cell_deps,
                        OutPointKey::from_out_point(point),
                        hash,
                    );
                }
                for parent in &accepted.parents {
                    insert_reader(&mut self.children, hash_key(parent), hash);
                }
            }
            Phase::Waiting(_) | Phase::Replaced(_) => {
                new_wait_entries.push((hash, Arc::clone(&new)));
            }
            Phase::Resolve | Phase::Verify(_) => {}
        }
        if is_new {
            self.proposals.insert(proposal_key(&hash), hash);
        }
        if WorkStage::for_phase(&new.phase).is_some() {
            queues.insert(&new);
        }
        if let Some(retired) = self.owners.insert(hash, new) {
            outcome.retired.owners.push(retired);
        }
    }

    pub(super) fn apply_ban(&mut self, peer: PeerIndex, deadline: Instant, now: Instant) {
        self.bans.retain(|_, until| *until > now);
        if self.bans.len() >= crate::constants::PEER_BAN_FENCE_CAPACITY
            && !self.bans.contains_key(&peer)
            && let Some(oldest) = self
                .bans
                .iter()
                .min_by_key(|(_, until)| **until)
                .map(|(peer, _)| *peer)
        {
            self.bans.remove(&oldest);
        }
        self.bans
            .entry(peer)
            .and_modify(|current| *current = (*current).max(deadline))
            .or_insert(deadline);
        self.mutation_epoch = self.mutation_epoch.wrapping_add(1);
        self.structural_epoch = self.structural_epoch.wrapping_add(1);
        self.recent_leaf_inserts.clear();
    }

    /// Advance proposal-band counts and pending ranks, install the new snapshot,
    /// and cache committed hashes as one lifecycle cut.
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "Each changed accepted owner is counted once in its previous band and view overflow is prechecked."
    )]
    pub(super) fn apply_lifecycle(
        &mut self,
        next_snapshot: Arc<Snapshot>,
        proposals: Option<&[(ProposalShortId, Status, Status)]>,
        attached_blocks: &[Arc<BlockView>],
        outcome: &mut CommitOutcome,
    ) {
        if let Some(changes) = proposals {
            for (id, before, after) in changes {
                let Some(owner) = self.proposal_owner(&id.unpack()) else {
                    continue;
                };
                let Some(value) = owner.accepted() else {
                    continue;
                };
                let before = value.status_with(|| *before);
                let after = value.status_with(|| *after);
                if before == after {
                    continue;
                }
                let hash = owner.hash_bytes();
                self.gap_count = self.gap_count - usize::from(before == Status::Gap)
                    + usize::from(after == Status::Gap);
                self.proposed_count = self.proposed_count - usize::from(before == Status::Proposed)
                    + usize::from(after == Status::Proposed);
                if self
                    .pending_ranks
                    .is_current(self.rank_generation, usize::MAX)
                {
                    if before == Status::Proposed {
                        self.pending_ranks.invalidate();
                    } else if after == Status::Proposed {
                        self.pending_ranks.remove(&hash);
                    }
                }
            }
        } else {
            self.gap_count = 0;
            self.proposed_count = 0;
            for accepted in self.owners.values().filter_map(|entry| entry.accepted()) {
                let status = accepted.status(&next_snapshot);
                self.gap_count += usize::from(status == Status::Gap);
                self.proposed_count += usize::from(status == Status::Proposed);
            }
            self.pending_ranks.invalidate();
        }
        outcome.retired.snapshot = Some(std::mem::replace(&mut self.snapshot, next_snapshot));
        self.view += 1;
        self.mutation_epoch = self.mutation_epoch.wrapping_add(1);
        self.structural_epoch = self.structural_epoch.wrapping_add(1);
        self.recent_leaf_inserts.clear();
        for block in attached_blocks {
            for tx_hash in block.tx_hashes().iter().skip(1) {
                let hash: [u8; 32] = tx_hash.unpack();
                self.committed.put(proposal_key(&hash), hash);
            }
        }
        outcome.template_changed = true;
        outcome.maintenance_ready = true;
        outcome.work_ready = true;
    }

    /// Replace the entire generation on `ClearScope::All` while advancing the
    /// lifecycle view. Displaced state returns in `Retired` for off-lock drop.
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "View overflow is prechecked before replacement."
    )]
    pub(super) fn replace_generation(
        &mut self,
        next_snapshot: Arc<Snapshot>,
        queues: &Queues,
        outcome: &mut CommitOutcome,
    ) {
        let mut fresh = Self::new(next_snapshot);
        fresh.view = self.view + 1;
        fresh.mutation_epoch = self.mutation_epoch.wrapping_add(1);
        fresh.structural_epoch = self.structural_epoch.wrapping_add(1);
        fresh.rank_generation = self.rank_generation.saturating_add(1);
        fresh.pending_ranks = RankedPriorities::invalid();
        fresh.bans = std::mem::take(&mut self.bans);
        outcome.retired.state = Some(Box::new(std::mem::replace(self, fresh)));
        outcome.retired.queues = Some(queues.take());
        outcome.template_changed = true;
        outcome.maintenance_ready = true;
        outcome.work_ready = true;
    }

    pub(super) fn is_leaf_accepted(&self, hash: &[u8; 32]) -> Option<Option<Arc<Entry>>> {
        let Some(root) = self.owners.get(hash).filter(|e| e.accepted().is_some()) else {
            return Some(None);
        };
        if self.children.get(hash).is_some_and(|c| !c.is_empty()) {
            return None;
        }
        Some(Some(Arc::clone(root)))
    }

    pub(super) fn last_updated(&self) -> u64 {
        self.accepted_times
            .last()
            .map_or(0, |(timestamp, _)| *timestamp)
    }

    pub(super) fn priority_for(
        &self,
        owner: &Entry,
        ancestors: Aggregate,
    ) -> Result<Priority, Error> {
        let value = owner.accepted().ok_or(Error::Stale)?;
        Ok(Priority {
            score: super::query::score(value, ancestors)?,
            arrival: owner.arrival,
            hash: owner.hash_bytes(),
        })
    }
}
