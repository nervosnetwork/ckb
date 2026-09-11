//! Live owners, tracked reads and their shared synchronization primitives.
//! [Plan] owns a prepared decision; [apply] keeps its preflight, coupled mutation,
//! guard release and retirement together.
mod apply;
mod plan;

use plan::same_weak;
pub(super) use plan::{Edit, Plan, ReadSet};

use super::{
    budget::{Amount, Budget, Limits},
    jobs::Job,
    model::{DependencyKey, Entry, Error, FullReason, Phase, RelationKey},
    notice::Outbox,
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
        reads.observe_owner(hash, entry.as_ref())?;
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
        reads.observe_owner(&point.tx_hash(), owner)?;
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
}
