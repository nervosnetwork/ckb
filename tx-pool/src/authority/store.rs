//! The only live owner/projection writer. Policy prepares a flat Plan; Apply
//! prepares charges, locks, validates observations, reserves capacity, mutates
//! and appends notices.
use super::{
    budget::{Amount, Budget, Limits, OwnerDelta},
    jobs::Job,
    model::{DependencyKey, Entry, Error, FullReason, Phase, RelationKey, Status},
    notice::{self, Batch, Class, Effect, Outbox},
    queue::{Queues, WorkStage},
};
use crate::util::compact_packed;
use ckb_app_config::TxPoolConfig;
use ckb_network::PeerIndex;
use ckb_snapshot::Snapshot;
use ckb_types::{
    packed::{Byte32, OutPoint, ProposalShortId},
    prelude::*,
};
use ckb_util::parking_lot::{Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::{
    collections::{BTreeMap, BTreeSet, hash_map::RandomState},
    hash::{BuildHasher, Hash, Hasher},
    ops::Bound::{Excluded, Unbounded},
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Instant,
};
use tokio::sync::Notify;

#[cfg(test)]
#[path = "tests/concurrency.rs"]
mod tests;

#[cfg(test)]
type CommitObserver = Arc<dyn Fn(&Plan, bool) + Send + Sync>;

pub(super) const SHARDS: usize = 256;
pub(super) const INPUT: u8 = 1;
pub(super) const DEP: u8 = 2;
pub(super) const WAIT: u8 = 4;
pub(super) const CHILD: u8 = 8;
const ACCEPTED_ROLES: u8 = INPUT | DEP | CHILD;
const WAKE_PAGE: usize = 32;

// Unique owner hashes in order, each paired with its old and new membership.
type MemberChanges<T> = Vec<(Byte32, (T, T))>;

/// Every decision read is kept, including negative owner/spender observations.
/// Successful resolution keeps producer identities and point spender facts;
/// membership separately observes complete reader and descendant relations.
#[derive(Clone, Debug, Default)]
pub(super) struct ReadSet {
    owners: BTreeMap<Byte32, Option<Weak<Entry>>>,
    spenders: BTreeMap<OutPoint, Option<Byte32>>,
    relations: BTreeMap<RelationKey, Option<Weak<()>>>,
    peers: BTreeMap<PeerIndex, Option<Weak<()>>>,
    // Keep full-capture vectors out of ordinary sparse transaction read sets.
    all: Option<Box<[u64; SHARDS]>>,
    accepted: Option<Box<[u64; SHARDS]>>,
}
fn same_weak<T>(a: &Option<Weak<T>>, b: &Option<Weak<T>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.ptr_eq(b),
        _ => false,
    }
}
impl ReadSet {
    pub(super) fn observed_owner(&self, hash: &Byte32) -> Option<&Option<Weak<Entry>>> {
        self.owners.get(hash)
    }
    pub(super) fn owner(&mut self, hash: &Byte32, entry: Option<&Arc<Entry>>) -> Result<(), Error> {
        let observed = entry.map(Arc::downgrade);
        if let Some(old) = self.owners.get(hash) {
            if !same_weak(old, &observed) {
                return Err(Error::Stale);
            }
        } else {
            self.owners.insert(compact_packed(hash), observed);
        }
        Ok(())
    }
    pub(super) fn merge(&mut self, other: &Self) -> Result<(), Error> {
        for (hash, entry) in &other.owners {
            if let Some(old) = self.owners.get(hash) {
                if !same_weak(old, entry) {
                    return Err(Error::Stale);
                }
            } else {
                self.owners.insert(hash.clone(), entry.clone());
            }
        }
        for (point, spender) in &other.spenders {
            if let Some(old) = self.spenders.get(point) {
                if old != spender {
                    return Err(Error::Stale);
                }
            } else {
                self.spenders.insert(point.clone(), spender.clone());
            }
        }
        for (key, read) in &other.relations {
            if let Some(old) = self.relations.get(key) {
                if !same_weak(old, read) {
                    return Err(Error::Stale);
                }
            } else {
                self.relations.insert(key.clone(), read.clone());
            }
        }
        for (key, read) in &other.peers {
            if let Some(old) = self.peers.get(key) {
                if !same_weak(old, read) {
                    return Err(Error::Stale);
                }
            } else {
                self.peers.insert(*key, read.clone());
            }
        }
        for (own, incoming) in [
            (&mut self.all, &other.all),
            (&mut self.accepted, &other.accepted),
        ] {
            if let Some(incoming) = incoming {
                if own.as_ref().is_some_and(|old| old != incoming) {
                    return Err(Error::Stale);
                }
                if own.is_none() {
                    *own = Some(incoming.clone());
                }
            }
        }
        Ok(())
    }
    pub(super) fn into_verification_reads(self) -> Self {
        Self {
            owners: self.owners,
            spenders: self.spenders,
            ..Self::default()
        }
    }
    pub(super) fn spent(&self) -> impl Iterator<Item = (&OutPoint, &Byte32)> {
        self.spenders
            .iter()
            .filter_map(|(point, spender)| spender.as_ref().map(|hash| (point, hash)))
    }
}

#[derive(Clone)]
pub(super) struct Edit {
    pub(super) before: Option<Arc<Entry>>,
    pub(super) after: Option<Arc<Entry>>,
}
/// A lifecycle revision is paired with its snapshot. No caller can increment it.
#[derive(Clone)]
pub(super) struct Plan {
    pub(super) view: u64,
    pub(super) reads: ReadSet,
    pub(super) edits: BTreeMap<Byte32, Edit>,
    pub(super) effects: Vec<Effect>,
    pub(super) class: Class,
    pub(super) snapshot: Option<Arc<Snapshot>>,
    pub(super) invalidate_view: bool,
    pub(super) dry_run: bool,
    pub(super) clear_all: bool,
    pub(super) committed: Vec<(ProposalShortId, Byte32)>,
    pub(super) ban: Option<(PeerIndex, Instant)>,
    pub(super) peer_access: Option<(PeerIndex, bool)>,
    pub(super) wake: BTreeSet<DependencyKey>,
    wake_advance: Option<WakePage>,
}
impl Plan {
    pub(super) fn new(view: u64, class: Class) -> Self {
        Self {
            view,
            reads: ReadSet::default(),
            edits: BTreeMap::new(),
            effects: Vec::new(),
            class,
            snapshot: None,
            invalidate_view: false,
            dry_run: false,
            clear_all: false,
            committed: Vec::new(),
            ban: None,
            peer_access: None,
            wake: BTreeSet::new(),
            wake_advance: None,
        }
    }
    pub(super) fn edit(
        &mut self,
        before: Option<Arc<Entry>>,
        after: Option<Arc<Entry>>,
    ) -> Result<(), Error> {
        let hash = before
            .as_ref()
            .or(after.as_ref())
            .ok_or(Error::Fault("empty owner edit"))?
            .hash();
        if after.as_ref().is_some_and(|entry| entry.hash() != hash) {
            return Err(Error::Fault("owner hash change"));
        }
        if self.edits.contains_key(&hash) {
            return Err(Error::Fault("duplicate owner edit"));
        }
        self.reads.owner(&hash, before.as_ref())?;
        // An identical immutable owner is only a read. Re-inserting its queue
        // projection could duplicate work that a worker has already selected.
        if before
            .as_ref()
            .zip(after.as_ref())
            .is_some_and(|(before, after)| Arc::ptr_eq(before, after))
        {
            return Ok(());
        }
        self.edits
            .insert(compact_packed(&hash), Edit { before, after });
        Ok(())
    }
    pub(super) fn advance(&mut self, page: WakePage) {
        self.wake_advance = Some(page);
    }
}

struct View {
    snapshot: Arc<Snapshot>,
    revision: u64,
}
#[derive(Default)]
struct Shard {
    owners: BTreeMap<Byte32, Arc<Entry>>,
    proposals: BTreeMap<[u8; ProposalShortId::TOTAL_SIZE], Byte32>,
    deadlines: BTreeSet<(Instant, Byte32)>,
    accepted_times: BTreeSet<(u64, Byte32)>,
    // Derived only by owner edits; proposed is refreshed with a new snapshot.
    orphan: usize,
    proposed: usize,
    revision: u64,
    accepted_revision: u64,
}
pub(super) struct Summary {
    pub(super) snapshot: Arc<Snapshot>,
    pub(super) accepted: Amount,
    pub(super) orphan: usize,
    pub(super) proposed: usize,
    pub(super) queued: usize,
    pub(super) last_updated: u64,
}

#[derive(Clone, Debug)]
struct Wake {
    pass: u64,
    after: Option<Byte32>,
}
#[derive(Debug)]
struct RelationMember {
    roles: u8,
    // A freshly registered current absence waits for a later availability
    // event. A policy-only history cannot wake itself on its own removal.
    wait_after_pass: u64,
}
#[derive(Debug, Default)]
struct Relation {
    // INPUT is held only by spender; this map stores DEP, WAIT and CHILD.
    members: BTreeMap<Byte32, RelationMember>,
    spender: Option<Byte32>,
    // Weak observations keep each retired marker allocation unique until the
    // observer is gone. Shared member updates need no finite counter reserve.
    accepted_version: Arc<()>,
    wake: Option<Wake>,
    next_pass: u64,
}
impl Relation {
    fn roles(&self, hash: &Byte32) -> u8 {
        self.members.get(hash).map_or(0, |member| member.roles)
            | if self.spender.as_ref() == Some(hash) {
                INPUT
            } else {
                0
            }
    }
    fn is_empty(&self) -> bool {
        self.spender.is_none() && self.members.is_empty() && self.wake.is_none()
    }
}
#[derive(Debug, Default)]
struct Peer {
    members: BTreeSet<Byte32>,
    version: Arc<()>,
}

#[derive(Clone)]
pub(super) struct WakePage {
    pub(super) key: DependencyKey,
    pub(super) hashes: Vec<Byte32>,
    row: Weak<Mutex<Relation>>,
    pass: u64,
    after: Option<Byte32>,
}

/// Nested acquisition order: view -> peer gates -> dependency gates -> owner
/// shards -> short row/queue mutexes. Each gate/shard family is sorted by index.
/// Relation and peer collections precede their row mutexes. Bookkeeping mutexes
/// (budget, outbox, bans, committed hashes, dirty keys) must not acquire an
/// earlier layer while held. Queue selection releases its lane before owners.
/// New acquisition paths must fit this order; no authority guard spans await.
pub(super) struct Store {
    #[cfg(test)]
    pub(super) commit_observer: Mutex<Option<CommitObserver>>,
    #[cfg(test)]
    pub(super) admission_attempts: AtomicU64,
    view: RwLock<View>,
    shards: [RwLock<Shard>; SHARDS],
    relations: [Mutex<BTreeMap<RelationKey, Arc<Mutex<Relation>>>>; SHARDS],
    dependency_gates: [RwLock<()>; SHARDS],
    peer_gates: [RwLock<()>; SHARDS],
    peers: Mutex<BTreeMap<PeerIndex, Arc<Mutex<Peer>>>>,
    bans: Mutex<BTreeMap<PeerIndex, Instant>>,
    committed: Mutex<lru::LruCache<ProposalShortId, Byte32>>,
    dirty: Mutex<BTreeSet<DependencyKey>>,
    routing: RandomState,
    arrival: AtomicU64,
    stopped: AtomicBool,
    chain_pending: AtomicBool,
    faulted: Arc<AtomicBool>,
    pub(super) budget: Arc<Budget>,
    pub(super) outbox: Arc<Outbox>,
    queues: Queues,
    pub(super) changed: Notify,
    pub(super) template_changed: Notify,
    pub(super) work: Notify,
}

/// The sole reliable chain consumer temporarily prevents new owner commits
/// while preparing its bounded pure reconciliation. In-flight cuts finish;
/// ordinary work waits outside guards and resumes when this pause is dropped.
/// Normal reconciliation holds the pause through publication of the new view.
pub(super) struct ChainPause<'a>(&'a Store);
impl Drop for ChainPause<'_> {
    fn drop(&mut self) {
        self.0.chain_pending.store(false, Ordering::Release);
        self.0.work.notify_waiters();
        self.0.changed.notify_waiters();
    }
}
enum Guard<'a, T> {
    Read(RwLockReadGuard<'a, T>),
    Write(RwLockWriteGuard<'a, T>),
}
impl<T> Guard<'_, T> {
    fn get(&self) -> &T {
        match self {
            Self::Read(g) => g,
            Self::Write(g) => g,
        }
    }
    fn get_mut(&mut self) -> Option<&mut T> {
        match self {
            Self::Read(_) => None,
            Self::Write(g) => Some(g),
        }
    }
}
/// Keep recoverable Result/Option propagation out of the post-preflight tail.
/// This unit-return boundary does not claim panic or allocation-failure recovery.
fn commit_infallibly(commit: impl FnOnce()) {
    commit();
}

/// Temporary lock footprint: present bits select shards; write bits only upgrade.
#[derive(Default)]
struct LockFootprint {
    present: [u64; SHARDS.div_ceil(64)],
    write: [u64; SHARDS.div_ceil(64)],
}
impl LockFootprint {
    #[expect(
        clippy::indexing_slicing,
        reason = "LockFootprint indices come only from keyed routing modulo SHARDS or the complete 0..SHARDS range."
    )]
    fn insert(&mut self, index: usize, write: bool) {
        let bit = 1_u64 << (index % 64);
        self.present[index / 64] |= bit;
        if write {
            self.write[index / 64] |= bit;
        }
    }
    fn len(&self) -> usize {
        self.present
            .iter()
            .map(|word| word.count_ones() as usize)
            .sum()
    }
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "Only a nonzero word is decremented; its bit is below64 and the word offset is bounded by the fixed shard array."
    )]
    fn iter(&self) -> impl Iterator<Item = (usize, bool)> + '_ {
        self.present
            .iter()
            .zip(&self.write)
            .enumerate()
            .flat_map(|(word, (&present, &write))| {
                let mut remaining = present;
                std::iter::from_fn(move || {
                    if remaining == 0 {
                        return None;
                    }
                    let bit = remaining.trailing_zeros() as usize;
                    remaining &= remaining - 1;
                    Some((word * 64 + bit, write & (1_u64 << bit) != 0))
                })
            })
    }
}

#[expect(
    clippy::indexing_slicing,
    reason = "LockFootprint indices come only from keyed routing modulo SHARDS or the complete 0..SHARDS range."
)]
fn acquire<'a, T>(
    locks: &'a [RwLock<T>; SHARDS],
    footprint: &LockFootprint,
) -> Vec<(usize, Guard<'a, T>)> {
    let mut guards = Vec::with_capacity(footprint.len());
    for (index, write) in footprint.iter() {
        guards.push((
            index,
            if write {
                Guard::Write(locks[index].write())
            } else {
                Guard::Read(locks[index].read())
            },
        ));
    }
    guards
}
#[expect(
    clippy::expect_used,
    reason = "A valid Byte32 has 32 bytes, so its 10-byte proposal prefix always exists."
)]
fn proposal_key(hash: &Byte32) -> &[u8; ProposalShortId::TOTAL_SIZE] {
    hash.as_slice()
        .first_chunk()
        .expect("a transaction hash contains a proposal ID")
}
fn compact_dependency(key: &DependencyKey) -> DependencyKey {
    match key {
        DependencyKey::Cell(point) => DependencyKey::Cell(compact_packed(point)),
        DependencyKey::Header(hash) => DependencyKey::Header(compact_packed(hash)),
    }
}
fn compact_relation(key: &RelationKey) -> RelationKey {
    match key {
        RelationKey::Dependency(key) => RelationKey::Dependency(compact_dependency(key)),
        RelationKey::Children(hash) => RelationKey::Children(compact_packed(hash)),
    }
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

impl Store {
    pub(super) fn new(snapshot: Arc<Snapshot>, config: &TxPoolConfig) -> Result<Arc<Self>, Error> {
        let limits = Limits::new(config, snapshot.consensus())?;
        Self::with_limits(snapshot, config, limits)
    }
    pub(super) fn with_limits(
        snapshot: Arc<Snapshot>,
        config: &TxPoolConfig,
        limits: Limits,
    ) -> Result<Arc<Self>, Error> {
        let faulted = Arc::new(AtomicBool::new(false));
        let outbox = Outbox::new(&limits, Arc::clone(&faulted))?;
        Ok(Arc::new(Self {
            #[cfg(test)]
            commit_observer: Mutex::new(None),
            #[cfg(test)]
            admission_attempts: AtomicU64::new(0),
            view: RwLock::new(View {
                snapshot,
                revision: 0,
            }),
            shards: std::array::from_fn(|_| RwLock::new(Shard::default())),
            relations: std::array::from_fn(|_| Mutex::new(BTreeMap::new())),
            dependency_gates: std::array::from_fn(|_| RwLock::new(())),
            peer_gates: std::array::from_fn(|_| RwLock::new(())),
            peers: Mutex::new(BTreeMap::new()),
            bans: Mutex::new(BTreeMap::new()),
            committed: Mutex::new(lru::LruCache::new(100_000)),
            dirty: Mutex::new(BTreeSet::new()),
            routing: RandomState::new(),
            arrival: AtomicU64::new(0),
            stopped: AtomicBool::new(false),
            chain_pending: AtomicBool::new(false),
            faulted,
            budget: Budget::new(limits),
            outbox,
            queues: Queues::new(config.verify_ordering, config.max_tx_verify_cycles),
            changed: Notify::new(),
            template_changed: Notify::new(),
            work: Notify::new(),
        }))
    }
    fn route<T: Hash + ?Sized>(&self, key: &T) -> usize {
        (self.routing.hash_one(key) as usize) % SHARDS
    }
    fn owner_shard(&self, hash: &Byte32) -> usize {
        // Packed ProposalShortId hashes raw bytes, without a slice-length prefix.
        let mut hasher = self.routing.build_hasher();
        hasher.write(proposal_key(hash));
        (hasher.finish() as usize) % SHARDS
    }
    pub(super) fn begin_chain(&self) -> Result<ChainPause<'_>, Error> {
        self.chain_pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| Error::Full(FullReason::ChainTransition))?;
        Ok(ChainPause(self))
    }
    pub(super) fn snapshot(&self) -> (u64, Arc<Snapshot>) {
        let view = self.view.read();
        (view.revision, Arc::clone(&view.snapshot))
    }
    pub(super) fn next_arrival(&self) -> Result<u64, Error> {
        self.arrival
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| {
                old.checked_add(1)
            })
            .map_err(|_| {
                self.fault();
                Error::Fault("arrival counter")
            })
    }
    pub(super) fn fault(&self) {
        if !self.faulted.swap(true, Ordering::AcqRel) {
            crate::metrics::record_failure(crate::metrics::FailureBoundary::TypedFault);
        }
        self.changed.notify_waiters();
        self.template_changed.notify_waiters();
        self.work.notify_waiters();
        self.outbox.failed.notify_waiters();
        self.outbox.changed.notify_waiters();
    }
    pub(super) fn is_faulted(&self) -> bool {
        self.faulted.load(Ordering::Acquire) || self.budget.faulted()
    }
    pub(super) fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.work.notify_waiters();
        self.changed.notify_waiters();
        self.template_changed.notify_waiters();
    }
    pub(super) fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
    #[expect(
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects,
        reason = "The cursor is checked below SHARDS; counters advance only within the bounded scan."
    )]
    pub(super) fn next_missing(
        &self,
        cursor: &mut (u64, usize, Option<Byte32>),
        maximum: usize,
    ) -> (Option<crate::service::TxVerificationResult>, bool) {
        use std::ops::Bound::{Excluded, Unbounded};
        let view = self.view.read();
        if cursor.0 != view.revision {
            *cursor = (view.revision, 0, None);
        }
        let mut scanned = 0;
        while cursor.1 < SHARDS {
            let shard = self.shards[cursor.1].read();
            let lower = cursor.2.as_ref().map_or(Unbounded, Excluded);
            for (hash, entry) in shard.owners.range((lower, Unbounded)) {
                cursor.2 = Some(hash.clone());
                scanned += 1;
                if let (Phase::Waiting(keys), Some(peer)) =
                    (&entry.phase, entry.source.residency_peer())
                {
                    let parents = keys
                        .iter()
                        .filter_map(|key| match key {
                            DependencyKey::Cell(point) => Some(compact_packed(&point.tx_hash())),
                            _ => None,
                        })
                        .collect();
                    return (
                        Some(crate::service::TxVerificationResult::UnknownParents {
                            peer,
                            parents,
                        }),
                        false,
                    );
                }
                if scanned == maximum {
                    return (None, false);
                }
            }
            cursor.1 += 1;
            cursor.2 = None;
        }
        (None, true)
    }

    pub(super) fn peer_banned(&self, peer: PeerIndex) -> bool {
        self.bans
            .lock()
            .get(&peer)
            .is_some_and(|until| *until > Instant::now())
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "Keyed routing modulo SHARDS indexes fixed arrays of exactly SHARDS buckets."
    )]
    pub(super) fn point(&self, hash: &Byte32) -> (Arc<Snapshot>, Option<Arc<Entry>>) {
        let view = self.view.read();
        let shard = self.shards[self.owner_shard(hash)].read();
        (Arc::clone(&view.snapshot), shard.owners.get(hash).cloned())
    }

    pub(super) fn points(&self, hashes: &[Byte32]) -> Vec<Arc<Entry>> {
        let mut footprint = LockFootprint::default();
        for hash in hashes {
            footprint.insert(self.owner_shard(hash), false);
        }
        let _view = self.view.read();
        let owners = acquire(&self.shards, &footprint);
        hashes
            .iter()
            .filter_map(|hash| {
                owners
                    .iter()
                    .find(|(index, _)| *index == self.owner_shard(hash))
                    .and_then(|(_, guard)| guard.get().owners.get(hash))
                    .cloned()
            })
            .collect()
    }

    #[expect(
        clippy::type_complexity,
        reason = "One coherent lookup returns live owners and committed hashes with their chain snapshot."
    )]
    pub(super) fn compact_lookup(
        &self,
        ids: &[ProposalShortId],
    ) -> (
        Arc<Snapshot>,
        Vec<(ProposalShortId, Arc<Entry>)>,
        Vec<(ProposalShortId, Byte32)>,
    ) {
        let mut footprint = LockFootprint::default();
        for id in ids {
            footprint.insert(self.route(id), false);
        }
        let view = self.view.read();
        let owners = acquire(&self.shards, &footprint);
        let mut live = Vec::with_capacity(ids.len());
        let mut committed = Vec::with_capacity(ids.len());
        let cache = self.committed.lock();
        for id in ids {
            let shard = owners
                .iter()
                .find(|(index, _)| *index == self.route(id))
                .map(|(_, guard)| guard.get());
            if let Some(entry) = shard.and_then(|shard| {
                shard
                    .proposals
                    .get(id.as_slice())
                    .and_then(|hash| shard.owners.get(hash))
            }) {
                live.push((id.clone(), Arc::clone(entry)));
            } else if let Some(hash) = cache.peek(id) {
                committed.push((id.clone(), hash.clone()));
            }
        }
        (Arc::clone(&view.snapshot), live, committed)
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "Keyed routing modulo SHARDS indexes fixed arrays of exactly SHARDS buckets."
    )]
    pub(super) fn live_cell(
        &self,
        point: &OutPoint,
    ) -> (Arc<Snapshot>, Option<ckb_types::core::cell::CellStatus>) {
        use ckb_types::{
            bytes::Bytes,
            core::cell::{CellMetaBuilder, CellStatus},
        };
        let view = self.view.read();
        let key = RelationKey::Dependency(DependencyKey::Cell(point.clone()));
        let _dependency = self.dependency_gates[self.route(&key)].read();
        let shard = self.shards[self.owner_shard(&point.tx_hash())].read();
        let snapshot = Arc::clone(&view.snapshot);
        if self
            .relation(&key)
            .is_some_and(|row| row.lock().spender.is_some())
        {
            return (snapshot, Some(CellStatus::Unknown));
        }
        let Some(owner) = shard
            .owners
            .get(&point.tx_hash())
            .filter(|entry| entry.accepted().is_some())
        else {
            return (snapshot, None);
        };
        let index: u32 = point.index().unpack();
        let Some((output, data)) = owner.transaction.output_with_data(index as usize) else {
            return (snapshot, None);
        };
        let output =
            ckb_types::packed::CellOutput::new_unchecked(Bytes::copy_from_slice(output.as_slice()));
        let data = Bytes::copy_from_slice(&data);
        (
            snapshot,
            Some(CellStatus::Live(
                CellMetaBuilder::from_cell_output(output, data)
                    .out_point(compact_packed(point))
                    .build(),
            )),
        )
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "Keyed routing modulo SHARDS indexes fixed arrays of exactly SHARDS buckets."
    )]
    pub(super) fn get(
        &self,
        hash: &Byte32,
        reads: &mut ReadSet,
    ) -> Result<Option<Arc<Entry>>, Error> {
        let _view = self.view.read();
        let entry = self.shards[self.owner_shard(hash)]
            .read()
            .owners
            .get(hash)
            .cloned();
        reads.owner(hash, entry.as_ref())?;
        Ok(entry)
    }
    /// Copy one bounded pool cell while its producer still owns its complete
    /// payload charge. No provider or foreign code is called under this read.
    #[expect(
        clippy::indexing_slicing,
        reason = "Keyed routing modulo SHARDS indexes fixed arrays of exactly SHARDS buckets."
    )]
    pub(super) fn pool_cell(
        &self,
        point: &OutPoint,
        byte_limit: usize,
        reads: &mut ReadSet,
    ) -> Result<Option<ckb_types::core::cell::CellMeta>, Error> {
        use ckb_types::{bytes::Bytes, core::cell::CellMetaBuilder};
        let _view = self.view.read();
        let shard = self.shards[self.owner_shard(&point.tx_hash())].read();
        let owner = shard.owners.get(&point.tx_hash());
        reads.owner(&point.tx_hash(), owner)?;
        let Some(owner) = owner.filter(|entry| entry.accepted().is_some()) else {
            return Ok(None);
        };
        let index: u32 = point.index().unpack();
        let Some((output, data)) = owner.transaction.output_with_data(index as usize) else {
            return Ok(None);
        };
        let bytes = output
            .total_size()
            .checked_add(data.len())
            .and_then(|b| b.checked_add(256))
            .ok_or(Error::Full("pool cell arithmetic".into()))?;
        if bytes > byte_limit {
            return Err(Error::Full("pool cell materialization".into()));
        }
        let output =
            ckb_types::packed::CellOutput::new_unchecked(Bytes::copy_from_slice(output.as_slice()));
        let data = Bytes::copy_from_slice(&data);
        Ok(Some(
            CellMetaBuilder::from_cell_output(output, data)
                .out_point(compact_packed(point))
                .build(),
        ))
    }
    #[expect(
        clippy::indexing_slicing,
        reason = "Keyed routing modulo SHARDS indexes fixed arrays of exactly SHARDS buckets."
    )]
    fn relation(&self, key: &RelationKey) -> Option<Arc<Mutex<Relation>>> {
        self.relations[self.route(key)].lock().get(key).cloned()
    }
    pub(super) fn spender(
        &self,
        point: &OutPoint,
        reads: &mut ReadSet,
    ) -> Result<Option<Byte32>, Error> {
        let key = RelationKey::Dependency(DependencyKey::Cell(point.clone()));
        let row = self.relation(&key);
        let spender = row.as_ref().and_then(|row| row.lock().spender.clone());
        if let Some(old) = reads.spenders.get(point) {
            if old != &spender {
                return Err(Error::Stale);
            }
        } else {
            reads
                .spenders
                .insert(compact_packed(point), spender.clone());
        }
        Ok(spender)
    }
    pub(super) fn members(
        &self,
        key: &RelationKey,
        role: u8,
        reads: &mut ReadSet,
    ) -> Result<Vec<Byte32>, Error> {
        let row = self.relation(key);
        let (version, members) = row.as_ref().map_or((None, Vec::new()), |row| {
            let row = row.lock();
            let mut members: Vec<_> = row
                .members
                .iter()
                .filter(|(_, member)| member.roles & role != 0)
                .map(|(hash, _)| hash.clone())
                .collect();
            if role & INPUT != 0
                && let Some(spender) = &row.spender
                && let Err(index) = members.binary_search(spender)
            {
                members.insert(index, spender.clone());
            }
            (Some(Arc::downgrade(&row.accepted_version)), members)
        });
        if let Some(old) = reads.relations.get(key) {
            if !same_weak(old, &version) {
                return Err(Error::Stale);
            }
        } else {
            reads.relations.insert(compact_relation(key), version);
        }
        Ok(members)
    }
    pub(super) fn peer_members(
        &self,
        peer: PeerIndex,
        reads: &mut ReadSet,
    ) -> Result<Vec<Byte32>, Error> {
        let row = self.peers.lock().get(&peer).cloned();
        let (version, hashes) = row.as_ref().map_or((None, Vec::new()), |row| {
            let row = row.lock();
            (
                Some(Arc::downgrade(&row.version)),
                row.members.iter().cloned().collect(),
            )
        });
        if reads
            .peers
            .get(&peer)
            .is_some_and(|old| !same_weak(old, &version))
        {
            return Err(Error::Stale);
        }
        reads.peers.entry(peer).or_insert(version);
        Ok(hashes)
    }
    /// Caller holds a bounded worker/handler/capture permit through destruction.
    pub(super) fn capture(
        &self,
        accepted_only: bool,
    ) -> (u64, Arc<Snapshot>, Vec<Arc<Entry>>, ReadSet) {
        #[cfg(feature = "profiling")]
        let _span =
            tracing::trace_span!(target: "ckb_tx_pool_profile", "tx_pool.authority.capture")
                .entered();
        let view = self.view.read();
        let guards = self.shards.each_ref().map(RwLock::read);
        let versions = guards.each_ref().map(|shard| {
            if accepted_only {
                shard.accepted_revision
            } else {
                shard.revision
            }
        });
        let entries = guards
            .iter()
            .flat_map(|shard| shard.owners.values())
            .filter(|entry| !accepted_only || entry.accepted().is_some())
            .cloned()
            .collect();
        let reads = if accepted_only {
            ReadSet {
                accepted: Some(Box::new(versions)),
                ..ReadSet::default()
            }
        } else {
            ReadSet {
                all: Some(Box::new(versions)),
                ..ReadSet::default()
            }
        };
        (view.revision, Arc::clone(&view.snapshot), entries, reads)
    }
    /// Capture one accepted descendant closure at a coherent cut. Every CHILD
    /// membership update holds an owner writer, so these read guards stabilize
    /// the existing relation rows too. Caller owns a bounded read/capture slot.
    #[expect(
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects,
        reason = "Keyed routing indexes SHARDS guards; the cursor visits each captured owner once and cannot exceed the bounded Vec length."
    )]
    pub(super) fn capture_descendants(
        &self,
        hash: &Byte32,
    ) -> Result<(Arc<Snapshot>, Vec<Arc<Entry>>), Error> {
        let view = self.view.read();
        let guards = self.shards.each_ref().map(RwLock::read);
        let snapshot = Arc::clone(&view.snapshot);
        let Some(root) = guards[self.owner_shard(hash)]
            .owners
            .get(hash)
            .filter(|entry| entry.accepted().is_some())
        else {
            return Ok((snapshot, Vec::new()));
        };
        let mut owners = vec![Arc::clone(root)];
        let mut seen = BTreeSet::from([hash.clone()]);
        let mut cursor = 0;
        while let Some(parent) = owners.get(cursor) {
            if let Some(row) = self.relation(&RelationKey::Children(parent.hash())) {
                let row = row.lock();
                for (hash, member) in &row.members {
                    if member.roles & CHILD != 0 && seen.insert(hash.clone()) {
                        let child = guards[self.owner_shard(hash)]
                            .owners
                            .get(hash)
                            .filter(|entry| entry.accepted().is_some())
                            .ok_or(Error::Stale)?;
                        owners.push(Arc::clone(child));
                    }
                }
            }
            cursor += 1;
        }
        Ok((snapshot, owners))
    }
    /// Owner reservations settle before their guards open, so the accepted
    /// account and owner/queue projections describe this same complete cut.
    pub(super) fn capture_summary(&self) -> Summary {
        #[cfg(feature = "profiling")]
        let _span =
            tracing::trace_span!(target: "ckb_tx_pool_profile", "tx_pool.authority.capture")
                .entered();
        let view = self.view.read();
        let guards = self.shards.each_ref().map(RwLock::read);
        Summary {
            snapshot: Arc::clone(&view.snapshot),
            accepted: self.budget.accepted_usage(),
            orphan: guards.iter().map(|shard| shard.orphan).sum(),
            proposed: guards.iter().map(|shard| shard.proposed).sum(),
            queued: self.queues.queued_len(),
            last_updated: guards
                .iter()
                .filter_map(|shard| shard.accepted_times.last().map(|(time, _)| *time))
                .max()
                .unwrap_or(0),
        }
    }
    /// Validate captured facts before bounded synchronous publication, including
    /// candidate pruning and replacement of the current template.
    /// The closure must not perform I/O, callbacks, allocation or await. Removed
    /// output is returned to the caller for destruction after all guards open.
    pub(super) fn read_selected<R>(
        &self,
        view: u64,
        reads: &ReadSet,
        publish: impl FnOnce() -> R,
    ) -> Result<R, Error> {
        if !reads.relations.is_empty() || !reads.peers.is_empty() {
            return Err(Error::Fault("non-point selected read"));
        }
        let mut dependency_footprint = LockFootprint::default();
        for point in reads.spenders.keys() {
            dependency_footprint.insert(
                self.route(&RelationKey::Dependency(DependencyKey::Cell(point.clone()))),
                false,
            );
        }
        let mut footprint = LockFootprint::default();
        for hash in reads.owners.keys() {
            footprint.insert(self.owner_shard(hash), false);
        }
        if reads.all.is_some() || reads.accepted.is_some() {
            for index in 0..SHARDS {
                footprint.insert(index, false);
            }
        }
        let current = self.view.read();
        let _dependencies = acquire(&self.dependency_gates, &dependency_footprint);
        let owners = acquire(&self.shards, &footprint);
        if self.is_faulted() {
            return Err(Error::Fault("closed generation"));
        }
        if current.revision != view {
            return Err(Error::Stale);
        }
        self.validate_reads(reads, &owners)?;
        Ok(publish())
    }

    pub(super) fn pop(
        self: &Arc<Self>,
        stage: WorkStage,
        small_only: bool,
    ) -> Result<Option<Job>, Error> {
        if self.is_stopped() {
            return Ok(None);
        }
        if self.chain_pending.load(Ordering::Acquire) {
            return Ok(None);
        }
        let Some((entry, memory)) = self.queues.pop(stage, small_only, &self.budget)? else {
            return Ok(None);
        };
        // Pop released the lane. A concurrent successor owns its own queue
        // item; discard this old selection without touching that projection.
        let (view, _) = self.snapshot();
        if self
            .get(&entry.hash(), &mut ReadSet::default())?
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &entry))
        {
            Ok(Some(Job::new(Arc::clone(self), entry, view, memory)))
        } else {
            Ok(None)
        }
    }

    pub(super) fn wake_page(&self, cursor: &mut Option<DependencyKey>) -> Option<WakePage> {
        let key = {
            let dirty = self.dirty.lock();
            cursor
                .as_ref()
                .and_then(|after| dirty.range((Excluded(after), Unbounded)).next())
                .or_else(|| dirty.first())?
                .clone()
        };
        // Rotate even when this row disappears or the prepared page goes stale.
        // Never retain the dirty-key guard while taking a relation guard.
        *cursor = Some(key.clone());
        let relation_key = RelationKey::Dependency(key.clone());
        let row = self.relation(&relation_key)?;
        let relation = row.lock();
        let wake = relation.wake.as_ref()?;
        let hashes = relation
            .members
            .range((wake.after.as_ref().map_or(Unbounded, Excluded), Unbounded))
            .filter(|(_, flags)| flags.roles & WAIT != 0 && wake.pass > flags.wait_after_pass)
            .take(WAKE_PAGE)
            .map(|(hash, _)| hash.clone())
            .collect::<Vec<_>>();
        let after = hashes.last().cloned().or_else(|| wake.after.clone());
        Some(WakePage {
            key,
            hashes,
            row: Arc::downgrade(&row),
            pass: wake.pass,
            after,
        })
    }
    pub(super) fn expired(
        &self,
        now: Instant,
        accepted_before: u64,
        max: usize,
    ) -> Vec<Arc<Entry>> {
        if max == 0 {
            return Vec::new();
        }
        let _view = self.view.read();
        let mut result = Vec::new();
        for shard in &self.shards {
            let shard = shard.read();
            for (_, hash) in shard.deadlines.iter().take_while(|(at, _)| *at <= now) {
                if let Some(entry) = shard.owners.get(hash) {
                    result.push(Arc::clone(entry));
                    if result.len() == max {
                        return result;
                    }
                }
            }
            for (_, hash) in shard
                .accepted_times
                .iter()
                .take_while(|(at, _)| *at < accepted_before)
            {
                if let Some(entry) = shard.owners.get(hash) {
                    result.push(Arc::clone(entry));
                    if result.len() == max {
                        return result;
                    }
                }
            }
        }
        result
    }

    pub(super) fn apply(&self, mut plan: Plan) -> Result<Option<Arc<Batch>>, Error> {
        let mut notice = None;
        self.apply_plan(&mut plan, &mut notice)
    }

    /// Optional replacement history may lose capacity to another admission.
    /// Keep this plan's exact notices for one synchronous retry; no reservation
    /// or failed plan escapes to the service's asynchronous capacity wait.
    pub(super) fn apply_admission(
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
        if notice.is_none() {
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
        let lifecycle_write = plan.snapshot.is_some() || plan.invalidate_view;
        let work_changed = lifecycle_write
            || plan.edits.values().any(|edit| {
                edit.after
                    .as_ref()
                    .is_some_and(|entry| matches!(entry.phase, Phase::Resolve | Phase::Verify(_)))
            });
        let template_changed = lifecycle_write
            || plan.edits.values().any(|edit| {
                edit.before
                    .as_ref()
                    .is_some_and(|entry| entry.accepted().is_some())
                    || edit
                        .after
                        .as_ref()
                        .is_some_and(|entry| entry.accepted().is_some())
            });
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
                .filter(|(_, _, edit)| {
                    edit.before
                        .as_ref()
                        .is_some_and(|entry| entry.accepted().is_some())
                        || edit
                            .after
                            .as_ref()
                            .is_some_and(|entry| entry.accepted().is_some())
                })
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
                        let proposal = proposal_key(hash);
                        let before_deadline = edit.before.as_deref().and_then(deadline);
                        let after_deadline = edit.after.as_deref().and_then(deadline);
                        if let Some(before) = &edit.before {
                            // Exact owner validation and the population bound
                            // make these derived phase-count updates infallible.
                            shard.orphan -= usize::from(matches!(before.phase, Phase::Waiting(_)));
                            shard.proposed -= usize::from(before.accepted().is_some_and(|value| {
                                value.status(&view.get().snapshot) == Status::Proposed
                            }));
                            // A prior edit in this same plan may already have
                            // transferred this proposal ID to its new owner.
                            if edit.after.is_none() && shard.proposals.get(proposal) == Some(hash) {
                                shard.proposals.remove(proposal);
                            }
                            if before_deadline != after_deadline
                                && let Some(deadline) = before_deadline
                            {
                                shard.deadlines.remove(&(deadline, hash.clone()));
                            }
                            if let Some(accepted) = before.accepted() {
                                shard
                                    .accepted_times
                                    .remove(&(accepted.timestamp, hash.clone()));
                            }
                            if matches!(before.phase, Phase::Resolve | Phase::Verify(_)) {
                                self.queues.remove(before);
                            }
                        }
                        if let Some(after) = &edit.after {
                            shard.orphan += usize::from(matches!(after.phase, Phase::Waiting(_)));
                            shard.proposed += usize::from(after.accepted().is_some_and(|value| {
                                value.status(&view.get().snapshot) == Status::Proposed
                            }));
                            // An owner update keeps its hash and proposal mapping.
                            if edit.before.is_none() {
                                shard.proposals.insert(*proposal, hash.clone());
                            }
                            if before_deadline != after_deadline
                                && let Some(deadline) = after_deadline
                            {
                                shard.deadlines.insert((deadline, hash.clone()));
                            }
                            if let Some(accepted) = after.accepted() {
                                shard
                                    .accepted_times
                                    .insert((accepted.timestamp, hash.clone()));
                            }
                            retired.extend(shard.owners.insert(hash.clone(), Arc::clone(after)));
                            if matches!(after.phase, Phase::Resolve | Phase::Verify(_)) {
                                self.queues.insert(after);
                            }
                        } else {
                            retired.extend(shard.owners.remove(hash));
                        }
                        shard.revision += 1;
                        if edit
                            .before
                            .as_ref()
                            .is_some_and(|entry| entry.accepted().is_some())
                            || edit
                                .after
                                .as_ref()
                                .is_some_and(|entry| entry.accepted().is_some())
                        {
                            shard.accepted_revision += 1;
                        }
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
                        shard.proposed = shard
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
        reason = "Guard indices originate from the same SHARDS-sized routing as revision arrays."
    )]
    fn validate_reads(
        &self,
        reads: &ReadSet,
        owners: &[(usize, Guard<'_, Shard>)],
    ) -> Result<(), Error> {
        for (hash, expected) in &reads.owners {
            let shard = owners
                .iter()
                .find(|(index, _)| *index == self.owner_shard(hash))
                .ok_or(Error::Fault("owner read support"))?
                .1
                .get();
            if !same_weak(expected, &shard.owners.get(hash).map(Arc::downgrade)) {
                return Err(Error::Stale);
            }
        }
        for (index, shard) in owners {
            if reads
                .all
                .as_ref()
                .is_some_and(|versions| versions[*index] != shard.get().revision)
                || reads
                    .accepted
                    .as_ref()
                    .is_some_and(|versions| versions[*index] != shard.get().accepted_revision)
            {
                return Err(Error::Stale);
            }
        }
        for (point, expected) in &reads.spenders {
            let row = self.relation(&RelationKey::Dependency(DependencyKey::Cell(point.clone())));
            if row.as_ref().and_then(|row| row.lock().spender.clone()) != *expected {
                return Err(Error::Stale);
            }
        }
        for (key, expected) in &reads.relations {
            let row = self.relation(key);
            if !same_weak(
                expected,
                &row.as_ref()
                    .map(|row| Arc::downgrade(&row.lock().accepted_version)),
            ) {
                return Err(Error::Stale);
            }
        }
        for (peer, expected) in &reads.peers {
            let row = self.peers.lock().get(peer).cloned();
            if !same_weak(
                expected,
                &row.as_ref().map(|row| Arc::downgrade(&row.lock().version)),
            ) {
                return Err(Error::Stale);
            }
        }
        Ok(())
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
    fn start_wake(&self, key: &DependencyKey) {
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
