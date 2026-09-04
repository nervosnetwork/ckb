//! Fixed physical partition for the tx-pool authority.
//!
//! This module owns only representation and routing. It contains no operation
//! enum or policy table: semantic delta types fold their own real keys.

#[cfg(test)]
use super::ban::PeerBanDelta;
use super::{
    ban::{PeerBanCommitPermit, PeerBanLease, StagedPeerBanSlot},
    dependency::{DependencyLevel, DependencyRelationSet, UnindexedDependencyLevel},
    indexes::{AcceptedDeadlineKey, DeadlineKey, DueRemote},
    plan::{AcceptedOrderKey, AncestorAggregate, DescendantAggregate, EvictionOrderKey},
    resources::{
        AcceptedResources, ChargeProjection, ResourceError, ResourceTotals, ResourceVector,
    },
    source::{AuthoritySourceVersions, PoolTemplateVersions},
    state::{AcceptedEntry, AcceptedStatus, DependencyKey, OwnedTx, ProposalId, RawTxHash},
};
use ahash::RandomState;
use ckb_network::PeerIndex;
use ckb_types::packed::OutPoint;
#[cfg(test)]
use ckb_util::parking_lot::Mutex;
use ckb_util::parking_lot::{MappedRwLockReadGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, hash_map},
    fmt,
    hash::{BuildHasher, Hash, Hasher},
    ops::{Deref, DerefMut},
    sync::Arc,
};

pub(super) const AUTHORITY_SHARD_COUNT: usize = 64;

#[derive(Clone)]
pub(super) struct AuthorityShardRouter {
    state: RandomState,
}

impl AuthorityShardRouter {
    pub(super) fn new() -> Self {
        #[cfg(test)]
        {
            // Exercise the production AHash router with one reproducible
            // layout. Concurrency properties select same/disjoint physical
            // cuts from this layout; per-test random seeding would make the
            // existence of those witnesses probabilistic rather than make
            // the implementation more thoroughly covered.
            Self {
                state: RandomState::with_seeds(
                    0x434b_422d_5458_504f,
                    0x4f4c_2d53_4841_5244,
                    0x2d52_4f55_5445_522d,
                    0x5445_5354_2d52_3101,
                ),
            }
        }
        #[cfg(not(test))]
        {
            let source = std::collections::hash_map::RandomState::new();
            let seed = |index: u8| {
                let mut hasher = source.build_hasher();
                b"ckb-tx-pool/authority-shard-router".hash(&mut hasher);
                index.hash(&mut hasher);
                hasher.finish()
            };
            Self {
                state: RandomState::with_seeds(seed(0), seed(1), seed(2), seed(3)),
            }
        }
    }

    pub(in crate::authority) fn shard<K: Hash>(&self, domain: &'static [u8], key: &K) -> usize {
        let mut hasher = self.state.build_hasher();
        domain.hash(&mut hasher);
        key.hash(&mut hasher);
        (hasher.finish() as usize) & (AUTHORITY_SHARD_COUNT - 1)
    }

    pub(in crate::authority) fn owner(&self, key: &RawTxHash) -> usize {
        self.shard(b"owner-resource/owner", key)
    }

    fn peer_resource(&self, peer: &PeerIndex) -> usize {
        self.shard(b"owner-resource/peer", peer)
    }
}

/// Sole physical owner map, partitioned once for one authority-layout
/// lifetime. Ordinary production mutations use exact inner shard cuts under
/// the shared generation barrier; no duplicate flat owner map or exclusive
/// owner fallback exists.
#[derive(Clone)]
pub(in crate::authority) struct ShardedOwnerMap {
    pub(in crate::authority) layout: Arc<AuthorityShardLayout>,
}

pub(in crate::authority) struct AuthorityShardLayout {
    pub(in crate::authority) router: AuthorityShardRouter,
    pub(in crate::authority) shards: Box<[RwLock<AuthorityShard>; AUTHORITY_SHARD_COUNT]>,
    dependency_relations: Box<[RwLock<DependencyRelationShard>; AUTHORITY_SHARD_COUNT]>,
    dependency_gates: Box<[RwLock<()>; AUTHORITY_SHARD_COUNT]>,
    #[cfg(test)]
    concurrent_removal_probe: Mutex<Option<Arc<ConcurrentRemovalProbe>>>,
    #[cfg(test)]
    dependency_maintenance_plan_probe: Mutex<Option<Arc<ConcurrentRemovalProbe>>>,
    #[cfg(test)]
    membership_dependency_plan_probe: Mutex<Option<Arc<ConcurrentRemovalProbe>>>,
    #[cfg(test)]
    shared_ingress_probe: Mutex<Option<(SharedIngressProbePhase, Arc<ConcurrentRemovalProbe>)>>,
    #[cfg(test)]
    shared_owner_commit_probe: Mutex<Option<Arc<ConcurrentRemovalProbe>>>,
    #[cfg(test)]
    compute_settlement_commit_probe: Mutex<Option<Arc<ConcurrentRemovalProbe>>>,
    #[cfg(test)]
    compute_exchange_probe: Mutex<Option<(ComputeExchangeProbePhase, Arc<ConcurrentRemovalProbe>)>>,
    #[cfg(test)]
    generation_payload_swaps: AtomicUsize,
}

#[cfg(test)]
#[derive(Debug)]
pub(in crate::authority) struct ConcurrentRemovalProbe {
    entered: std::sync::mpsc::Sender<()>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum SharedIngressProbePhase {
    ProjectionPreparedBeforeOwnerCut,
    AfterRetainedIngressHeadClassification,
    AfterRetainedIngressSemanticCut,
    EffectReadCutBeforeActivation,
    FinalCutBeforeActivation,
    DirectMembershipPreparedBeforeFinalCut,
    DirectMembershipBeforeResourcePlan,
    DirectRejectionEffectStagedBeforeReadCut,
    DirectRejectionReadCutBeforeActivation,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum ComputeExchangeProbePhase {
    AfterSchedulerWave,
}

#[cfg(test)]
impl ConcurrentRemovalProbe {
    pub(in crate::authority) fn new() -> (
        Arc<Self>,
        std::sync::mpsc::Receiver<()>,
        std::sync::mpsc::Sender<()>,
    ) {
        let (entered, observed) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        (
            Arc::new(Self {
                entered,
                release: Mutex::new(released),
            }),
            observed,
            release,
        )
    }

    pub(in crate::authority) fn enter(&self) {
        let _ = self.entered.send(());
        let _ = self.release.lock().recv();
    }
}

/// One physical peer identity row. Cohort membership and the peer-local ban
/// fence deliberately share the same routed shard, so a malformed revocation
/// cannot publish a fence while a same-peer owner insertion escapes through a
/// different authority.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::authority) struct PeerIngressRow {
    pub(in crate::authority) owners: HashSet<RawTxHash>,
    pub(in crate::authority) fence: PeerIngressFence,
}

impl PeerIngressRow {
    pub(in crate::authority) fn has_hidden_fence(&self) -> bool {
        matches!(self.fence, PeerIngressFence::Hidden { .. })
    }
}

impl AuthorityShard {
    pub(in crate::authority) fn peer_ingress_row(&self, peer: PeerIndex) -> Option<PeerIngressRow> {
        let owners = self.peer_ingress_owners.get(&peer);
        let fence = self.peer_fences.get(&peer);
        if owners.is_none_or(HashSet::is_empty)
            && fence.is_none_or(|fence| matches!(fence, PeerIngressFence::Absent))
        {
            return None;
        }
        Some(PeerIngressRow {
            owners: owners.cloned().unwrap_or_default(),
            fence: fence.cloned().unwrap_or_default(),
        })
    }

    pub(in crate::authority) fn peer_ingress_row_matches(
        &self,
        peer: PeerIndex,
        expected: Option<&PeerIngressRow>,
    ) -> bool {
        match expected {
            None => {
                self.peer_ingress_owners
                    .get(&peer)
                    .is_none_or(HashSet::is_empty)
                    && self
                        .peer_fences
                        .get(&peer)
                        .is_none_or(|fence| matches!(fence, PeerIngressFence::Absent))
            }
            Some(expected) => {
                self.peer_ingress_owners.get(&peer).map_or_else(
                    || expected.owners.is_empty(),
                    |owners| owners == &expected.owners,
                ) && self.peer_fences.get(&peer).map_or_else(
                    || matches!(expected.fence, PeerIngressFence::Absent),
                    |fence| fence == &expected.fence,
                )
            }
        }
    }
}

/// `Hidden` is an allocation-complete, externally invisible intent. Normal
/// peer-owner Apply rejects it as contention; the revocation session alone may
/// turn it into `Active` in the same physical cut that removes the cohort.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::authority) enum PeerIngressFence {
    #[default]
    Absent,
    Hidden {
        stage_id: u64,
        previous: Option<(PeerBanLease, u64)>,
        next: PeerBanLease,
    },
    Active {
        lease: PeerBanLease,
        revision: u64,
    },
}

impl PeerIngressFence {
    pub(in crate::authority) fn logical_lease(&self) -> Option<PeerBanLease> {
        match self {
            Self::Absent => None,
            Self::Hidden { previous, .. } => previous.map(|(lease, _)| lease),
            Self::Active { lease, .. } => Some(*lease),
        }
    }

    pub(in crate::authority) const fn hidden_stage(&self) -> Option<u64> {
        match self {
            Self::Hidden { stage_id, .. } => Some(*stage_id),
            Self::Absent | Self::Active { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum PeerFenceStageError {
    Stale,
}

/// Linear owner of one hidden peer fence plus its bounded global slot. Drop
/// restores the exact previous physical row and releases the slot; activation
/// is permitted only inside the final owner/index shard cut.
#[must_use = "a hidden peer fence must activate with its cohort removal or roll back by Drop"]
pub(in crate::authority) struct StagedPeerIngressFence<'authority> {
    entries: &'authority ShardedOwnerMap,
    slot: Option<StagedPeerBanSlot<'authority>>,
}

/// The bank reservation has passed its final pre-owner check. Only this type
/// can activate the routed fence, so forgetting `begin_bank_commit` is not a
/// representable peer-revocation Apply path.
#[must_use = "a begun peer fence must activate with the exact owner cut"]
pub(in crate::authority) struct BegunPeerIngressFence<'authority> {
    entries: &'authority ShardedOwnerMap,
    permit: PeerBanCommitPermit<'authority>,
}

/// Generation-owned data in one physical shard. A generation replacement
/// swaps this whole value with an already-built carrier under the existing
/// shard lock. The peer-fence authority deliberately lives outside this value:
/// delayed Remote messages must still observe an active ban after all pool
/// ownership from the old generation has been retired.
#[derive(Debug, Default)]
pub(in crate::authority) struct AuthorityShardGeneration {
    owners: HashMap<RawTxHash, OwnedTx>,
    owner_removal_revision: OwnerShardRemovalRevision,
    pub(in crate::authority) membership_order_revision: MembershipOrderRevision,
    proposed_count: usize,
    resources: ResourceTotals,
    peer_resources: HashMap<PeerIndex, ResourceVector>,
    pub(in crate::authority) proposals: HashMap<ProposalId, RawTxHash>,
    pub(in crate::authority) peer_ingress_owners: HashMap<PeerIndex, HashSet<RawTxHash>>,
    // These owner-lifetime projections share the owner's physical shard. They
    // cannot create a cross-owner conflict or outlive that owner.
    pub(in crate::authority) context_sensitive_accepted: HashSet<RawTxHash>,
    pub(in crate::authority) deadlines: BTreeSet<DeadlineKey>,
    pub(in crate::authority) accepted_deadlines: BTreeSet<AcceptedDeadlineKey>,
    pub(in crate::authority) spenders: HashMap<OutPoint, RawTxHash>,
    // Causal and ordering rows are also owner-lifetime projections. Shared
    // conflict keys remain independently routed above and in dependency rows.
    pub(in crate::authority) parents: HashMap<RawTxHash, HashSet<RawTxHash>>,
    pub(in crate::authority) children: HashMap<RawTxHash, HashSet<RawTxHash>>,
    pub(in crate::authority) ancestor_aggregates: HashMap<RawTxHash, AncestorAggregate>,
    pub(in crate::authority) descendant_aggregates: HashMap<RawTxHash, DescendantAggregate>,
    pub(in crate::authority) accepted_order: BTreeSet<AcceptedOrderKey>,
    pub(in crate::authority) eviction_order: BTreeSet<EvictionOrderKey>,
    pub(in crate::authority) dependency_levels:
        std::collections::BTreeMap<DependencyKey, DependencyLevel>,
    pub(in crate::authority) dependency_dirty:
        std::collections::BTreeMap<DependencyKey, super::dependency::DirtyDependency>,
    pub(in crate::authority) dependency_unindexed: UnindexedDependencyLevel,
    relay_parent_source: u64,
    template_proposals_source: u64,
    template_transactions_source: u64,
}

/// One consumer-owner-routed relation partition. Relations are generation
/// payload, but their locks are independent from owner locks so publishing one
/// owner's final row cannot serialize an unrelated owner on a paused owner
/// cut.
#[derive(Debug, Default)]
pub(in crate::authority) struct DependencyRelationShard {
    pub(in crate::authority) rows: BTreeMap<DependencyKey, DependencyRelationSet>,
}

/// One fixed routed lock owns both the current generation payload and the
/// peer-fence truth. This keeps malformed cohort removal atomic with the owner
/// cut while allowing a generation swap to preserve active fences without a
/// copy, allocation, population scan, or second query authority.
#[derive(Debug, Default)]
pub(in crate::authority) struct AuthorityShard {
    generation: AuthorityShardGeneration,
    pub(in crate::authority) peer_fences: HashMap<PeerIndex, PeerIngressFence>,
}

impl Deref for AuthorityShard {
    type Target = AuthorityShardGeneration;

    fn deref(&self) -> &Self::Target {
        &self.generation
    }
}

impl DerefMut for AuthorityShard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.generation
    }
}

/// Monotonic negative-owner evidence for one fixed physical shard.
///
/// Exact owner versions prove positive identity, but an absent row can return
/// to the same value after `Absent -> Present -> Absent`.  Every successful
/// in-place owner removal advances this bounded witness. Whole-generation
/// replacement is fenced separately. Exhaustion never wraps: it permanently
/// disables shared-vacancy compilation for this shard while the canonical path
/// remains available.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum OwnerShardRemovalRevision {
    Active(u64),
    Exhausted,
}

impl Default for OwnerShardRemovalRevision {
    fn default() -> Self {
        Self::Active(0)
    }
}

impl OwnerShardRemovalRevision {
    fn advance(&mut self) {
        *self = match *self {
            Self::Active(current) => current
                .checked_add(1)
                .map(Self::Active)
                .unwrap_or(Self::Exhausted),
            Self::Exhausted => Self::Exhausted,
        };
    }

    fn vacancy_witness(self) -> Option<Self> {
        matches!(self, Self::Active(_)).then_some(self)
    }
}

/// Monotonic evidence for the accepted/eviction order projection in one
/// physical shard.  Capacity policy reads all 64 order sets under one fixed
/// cut and records these revisions; any later insertion, removal or aggregate
/// rekey makes the compiled frontier stale without retaining an owner-sized
/// snapshot.  Exhaustion never wraps into reusable absence evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum MembershipOrderRevision {
    Active(u64),
    Exhausted,
}

impl Default for MembershipOrderRevision {
    fn default() -> Self {
        Self::Active(0)
    }
}

impl MembershipOrderRevision {
    pub(in crate::authority) fn advance(&mut self) {
        *self = match *self {
            Self::Active(current) => current
                .checked_add(1)
                .map(Self::Active)
                .unwrap_or(Self::Exhausted),
            Self::Exhausted => Self::Exhausted,
        };
    }

    pub(in crate::authority) fn witness(self) -> Option<u64> {
        match self {
            Self::Active(revision) => Some(revision),
            Self::Exhausted => None,
        }
    }
}

/// A point read keeps the owning shard locked for the complete lifetime of
/// the borrowed transaction.  Returning this guard instead of cloning an
/// `OwnedTx` is the first production representation boundary that remains
/// sound after the outer authority lock is removed.
pub(in crate::authority) struct ShardedOwnerReadGuard<'map> {
    owner: MappedRwLockReadGuard<'map, OwnedTx>,
}

pub(in crate::authority) struct ShardedAcceptedReadGuard<'map> {
    entry: MappedRwLockReadGuard<'map, AcceptedEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum OwnerEntryKind {
    PreAccepted,
    Accepted,
    ReplacementHistory,
}

impl Deref for ShardedOwnerReadGuard<'_> {
    type Target = OwnedTx;

    fn deref(&self) -> &Self::Target {
        &self.owner
    }
}

impl<'map> ShardedOwnerReadGuard<'map> {
    pub(in crate::authority) fn into_accepted(
        self,
    ) -> Result<ShardedAcceptedReadGuard<'map>, Self> {
        match MappedRwLockReadGuard::try_map(self.owner, |owner| match owner {
            OwnedTx::Accepted(entry) => Some(entry),
            OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_) => None,
        }) {
            Ok(entry) => Ok(ShardedAcceptedReadGuard { entry }),
            Err(owner) => Err(Self { owner }),
        }
    }
}

impl Deref for ShardedAcceptedReadGuard<'_> {
    type Target = AcceptedEntry;

    fn deref(&self) -> &Self::Target {
        &self.entry
    }
}

/// One coherent full-owner read cut.  Full snapshots and ordered queries
/// deliberately hold all fixed shards; point and bounded reads use the point
/// guard above instead.
pub(in crate::authority) struct ShardedOwnerReadCut<'map> {
    router: AuthorityShardRouter,
    shards: [RwLockReadGuard<'map, AuthorityShard>; AUTHORITY_SHARD_COUNT],
}

/// One coherent full relation-bank read cut. Key-wide folds take this cut
/// only in test snapshots; production key-wide folds must release each bank
/// shard before taking the next so they never become a global writer barrier.
#[cfg(test)]
pub(in crate::authority) struct ShardedDependencyRelationReadCut<'map> {
    shards: [RwLockReadGuard<'map, DependencyRelationShard>; AUTHORITY_SHARD_COUNT],
}

#[derive(Clone, Copy, Default)]
pub(in crate::authority) struct ShardWriteSupport(u64);

#[derive(Clone, Copy, Default)]
pub(in crate::authority) struct ShardReadSupport(u64);

/// Complete physical support for one ordinary Apply. Reads may overlap reads;
/// any cross-Apply write collision makes the pair incompatible. The support is
/// compiled once from the same typed deltas consumed by Apply.
#[derive(Clone, Copy, Default)]
pub(in crate::authority) struct ShardApplySupport {
    reads: ShardReadSupport,
    writes: ShardWriteSupport,
}

/// Fixed dependency conflict classes acquired before owner shards. Ordinary
/// relation insertions take shared gates; exact-key folds, removals, and waiter
/// changes take write gates. The fixed bank avoids a second relation directory
/// and its lifecycle.
#[derive(Clone, Copy, Default)]
pub(in crate::authority) struct DependencyGateSupport {
    reads: u64,
    writes: u64,
}

/// Proof that the complete, mode-coalesced dependency gate set was acquired in
/// canonical order before any owner shard. The guards are intentionally opaque.
pub(in crate::authority) struct DependencyGateCut<'map> {
    _reads: [Option<RwLockReadGuard<'map, ()>>; AUTHORITY_SHARD_COUNT],
    _writes: [Option<RwLockWriteGuard<'map, ()>>; AUTHORITY_SHARD_COUNT],
}

impl DependencyGateSupport {
    pub(in crate::authority) fn read(&mut self, shard: usize) {
        self.reads |= 1u64 << shard;
    }

    pub(in crate::authority) fn write(&mut self, shard: usize) {
        self.writes |= 1u64 << shard;
        self.reads &= !(1u64 << shard);
    }

    pub(in crate::authority) fn include(&mut self, other: Self) {
        self.writes |= other.writes;
        self.reads = (self.reads | other.reads) & !self.writes;
    }

    #[cfg(test)]
    pub(in crate::authority) fn is_compatible(self, other: Self) -> bool {
        self.writes & (other.reads | other.writes) == 0
            && other.writes & (self.reads | self.writes) == 0
    }

    fn reads(self, shard: usize) -> bool {
        self.reads & (1u64 << shard) != 0
    }

    fn writes(self, shard: usize) -> bool {
        self.writes & (1u64 << shard) != 0
    }
}

/// Exact, allocation-free per-shard source targets prepared before owner
/// mutation under the same fixed write cut. Counts are semantic owner changes,
/// so batching and a no-interleave singleton fold reach the same source while
/// every actual commit still advances at least one changed shard.
pub(in crate::authority) struct ShardOwnerSourceAdvance {
    relay_parents: [Option<u64>; AUTHORITY_SHARD_COUNT],
    proposals: [Option<u64>; AUTHORITY_SHARD_COUNT],
    transactions: [Option<u64>; AUTHORITY_SHARD_COUNT],
}

#[derive(Clone, Copy)]
pub(in crate::authority) struct ShardOwnerSourceCounts {
    relay_parents: [u64; AUTHORITY_SHARD_COUNT],
    proposals: [u64; AUTHORITY_SHARD_COUNT],
    transactions: [u64; AUTHORITY_SHARD_COUNT],
}

pub(in crate::authority) struct ShardOwnerSourcePlan {
    counts: ShardOwnerSourceCounts,
    exclusive_advance: ShardOwnerSourceAdvance,
}

impl ShardOwnerSourcePlan {
    pub(in crate::authority) const fn none() -> Self {
        Self {
            counts: ShardOwnerSourceCounts::none(),
            exclusive_advance: ShardOwnerSourceAdvance {
                relay_parents: [None; AUTHORITY_SHARD_COUNT],
                proposals: [None; AUTHORITY_SHARD_COUNT],
                transactions: [None; AUTHORITY_SHARD_COUNT],
            },
        }
    }

    pub(in crate::authority) fn counts(&self) -> ShardOwnerSourceCounts {
        self.counts
    }

    pub(in crate::authority) fn into_exclusive_advance(self) -> ShardOwnerSourceAdvance {
        self.exclusive_advance
    }
}

impl ShardOwnerSourceCounts {
    pub(in crate::authority) const fn none() -> Self {
        Self {
            relay_parents: [0; AUTHORITY_SHARD_COUNT],
            proposals: [0; AUTHORITY_SHARD_COUNT],
            transactions: [0; AUTHORITY_SHARD_COUNT],
        }
    }

    pub(in crate::authority) fn record(
        &mut self,
        shard: usize,
        relay_parent: bool,
        proposal: bool,
        transaction: bool,
    ) -> Option<()> {
        if relay_parent {
            let count = self.relay_parents.get_mut(shard)?;
            *count = count.checked_add(1)?;
        }
        if proposal {
            let count = self.proposals.get_mut(shard)?;
            *count = count.checked_add(1)?;
        }
        if transaction {
            let count = self.transactions.get_mut(shard)?;
            *count = count.checked_add(1)?;
        }
        Some(())
    }

    pub(in crate::authority) fn changed(self) -> (bool, bool) {
        (
            self.proposals.iter().any(|count| *count != 0),
            self.transactions.iter().any(|count| *count != 0),
        )
    }
}

impl ShardApplySupport {
    pub(in crate::authority) fn new(reads: ShardReadSupport, writes: ShardWriteSupport) -> Self {
        Self { reads, writes }
    }

    pub(in crate::authority) fn reads(self) -> ShardReadSupport {
        self.reads
    }

    pub(in crate::authority) fn writes(self) -> ShardWriteSupport {
        self.writes
    }

    pub(in crate::authority) fn is_compatible(self, other: Self) -> bool {
        self.writes.0 & (other.writes.0 | other.reads.0) == 0 && other.writes.0 & self.reads.0 == 0
    }
}

impl ShardReadSupport {
    pub(in crate::authority) fn insert(&mut self, shard: usize) {
        self.0 |= 1u64 << shard;
    }

    pub(in crate::authority) fn include(&mut self, other: Self) {
        self.0 |= other.0;
    }

    fn contains(self, shard: usize) -> bool {
        self.0 & (1u64 << shard) != 0
    }

    #[cfg(test)]
    pub(in crate::authority) fn is_disjoint_from_writes(self, writes: ShardWriteSupport) -> bool {
        self.0 & writes.0 == 0
    }

    #[cfg(test)]
    pub(in crate::authority) fn mask_for_foundation(self) -> u64 {
        self.0
    }
}

impl ShardWriteSupport {
    pub(in crate::authority) fn insert(&mut self, shard: usize) {
        self.0 |= 1u64 << shard;
    }

    pub(in crate::authority) fn include(&mut self, other: Self) {
        self.0 |= other.0;
    }

    #[cfg(test)]
    pub(in crate::authority) fn is_disjoint(self, other: Self) -> bool {
        self.0 & other.0 == 0
    }

    #[cfg(test)]
    pub(in crate::authority) fn mask_for_foundation(self) -> u64 {
        self.0
    }

    fn contains(self, shard: usize) -> bool {
        self.0 & (1u64 << shard) != 0
    }
}

/// Sorted fixed-layout write bundle. Construction walks the 64 physical
/// shards in ascending order and allocates nothing, so two disjoint bundles
/// can overlap while a multi-shard transition remains atomic to readers.
pub(in crate::authority) struct ShardedOwnerWriteCut<'map> {
    reads: [Option<RwLockReadGuard<'map, AuthorityShard>>; AUTHORITY_SHARD_COUNT],
    writes: [Option<RwLockWriteGuard<'map, AuthorityShard>>; AUTHORITY_SHARD_COUNT],
}

/// Sorted relation-bank cut acquired after dependency gates and before owner
/// shards. Read and write support use the same fixed routing mask as owner
/// cuts, but guard an independent physical bank.
pub(in crate::authority) struct ShardedDependencyRelationWriteCut<'map> {
    reads: [Option<RwLockReadGuard<'map, DependencyRelationShard>>; AUTHORITY_SHARD_COUNT],
    writes: [Option<RwLockWriteGuard<'map, DependencyRelationShard>>; AUTHORITY_SHARD_COUNT],
}

impl ShardedDependencyRelationWriteCut<'_> {
    #[expect(
        clippy::expect_used,
        reason = "support is folded from the same dependency witness consumed by this sealed relation cut"
    )]
    pub(in crate::authority) fn projection_shard(&self, shard: usize) -> &DependencyRelationShard {
        self.writes
            .get(shard)
            .and_then(Option::as_deref)
            .or_else(|| self.reads.get(shard).and_then(Option::as_deref))
            .expect("relation support contains every shard consumed by the prepared witness")
    }

    #[expect(
        clippy::expect_used,
        reason = "write support is folded from the same dependency delta consumed by this sealed relation cut"
    )]
    pub(in crate::authority) fn projection_shard_mut(
        &mut self,
        shard: usize,
    ) -> &mut DependencyRelationShard {
        self.writes
            .get_mut(shard)
            .and_then(Option::as_deref_mut)
            .expect("relation write support contains every shard changed by the prepared delta")
    }
}

#[cfg(test)]
impl ShardedDependencyRelationReadCut<'_> {
    pub(in crate::authority) fn shards(&self) -> impl Iterator<Item = &DependencyRelationShard> {
        self.shards.iter().map(Deref::deref)
    }
}

impl ShardedOwnerWriteCut<'_> {
    #[expect(
        clippy::expect_used,
        reason = "support is folded from the same typed witness consumed by this sealed write cut"
    )]
    pub(in crate::authority) fn projection_shard(&self, shard: usize) -> &AuthorityShard {
        self.writes
            .get(shard)
            .and_then(Option::as_deref)
            .or_else(|| self.reads.get(shard).and_then(Option::as_deref))
            .expect("write support contains every shard consumed by the prepared witness")
    }

    #[expect(
        clippy::expect_used,
        reason = "support is folded from the same owner/status/resource plans consumed by this sealed write cut"
    )]
    fn shard_mut(&mut self, shard: usize) -> &mut AuthorityShard {
        self.writes
            .get_mut(shard)
            .and_then(Option::as_deref_mut)
            .expect("write support contains every shard consumed by the prepared owner delta")
    }

    pub(in crate::authority) fn projection_shard_mut(
        &mut self,
        shard: usize,
    ) -> &mut AuthorityShard {
        self.shard_mut(shard)
    }

    pub(in crate::authority) fn replace(
        &mut self,
        shard: usize,
        key: RawTxHash,
        after: Option<OwnedTx>,
    ) -> Option<OwnedTx> {
        let shard = self.shard_mut(shard);
        match after {
            Some(after) => shard.owners.insert(key, after),
            None => {
                let removed = shard.owners.remove(&key);
                if removed.is_some() {
                    shard.owner_removal_revision.advance();
                }
                removed
            }
        }
    }

    pub(in crate::authority) fn owner_removal_revision(
        &self,
        shard: usize,
    ) -> OwnerShardRemovalRevision {
        self.projection_shard(shard).owner_removal_revision
    }

    pub(in crate::authority) fn membership_order_revision(
        &self,
        shard: usize,
    ) -> MembershipOrderRevision {
        self.projection_shard(shard).membership_order_revision
    }

    pub(in crate::authority) fn owner_version(
        &self,
        shard: usize,
        key: &RawTxHash,
    ) -> Option<super::state::EntryVersion> {
        self.projection_shard(shard)
            .owners
            .get(key)
            .map(|owner| owner.record().version)
    }

    pub(in crate::authority) fn owner_version_and_accepted(
        &self,
        shard: usize,
        key: &RawTxHash,
    ) -> Option<(super::state::EntryVersion, bool)> {
        self.projection_shard(shard).owners.get(key).map(|owner| {
            (
                owner.record().version,
                matches!(owner, OwnedTx::Accepted(_)),
            )
        })
    }

    pub(in crate::authority) fn owner_and_vacancy_revision(
        &self,
        entries: &ShardedOwnerMap,
        key: &RawTxHash,
    ) -> (Option<OwnedTx>, Option<OwnerShardRemovalRevision>) {
        let shard = entries.layout.router.owner(key);
        let projection = self.projection_shard(shard);
        (
            projection.owners.get(key).cloned(),
            projection.owner_removal_revision.vacancy_witness(),
        )
    }

    pub(in crate::authority) fn owner(
        &self,
        entries: &ShardedOwnerMap,
        key: &RawTxHash,
    ) -> Option<&OwnedTx> {
        let shard = entries.layout.router.owner(key);
        self.projection_shard(shard).owners.get(key)
    }

    pub(in crate::authority) fn membership_spender(
        &self,
        entries: &ShardedOwnerMap,
        input: &OutPoint,
    ) -> Option<&RawTxHash> {
        let shard = entries.layout.router.shard(b"membership/spender", input);
        self.projection_shard(shard).spenders.get(input)
    }

    pub(in crate::authority) fn proposal_owner(
        &self,
        entries: &ShardedOwnerMap,
        proposal: &ProposalId,
    ) -> Option<&RawTxHash> {
        let shard = entries.layout.router.shard(b"index/proposal", proposal);
        self.projection_shard(shard).proposals.get(proposal)
    }

    pub(in crate::authority) fn peer_resource(
        &self,
        entries: &ShardedOwnerMap,
        peer: PeerIndex,
    ) -> ResourceVector {
        let shard = entries.layout.router.peer_resource(&peer);
        self.projection_shard(shard)
            .peer_resources
            .get(&peer)
            .copied()
            .unwrap_or_default()
    }

    pub(in crate::authority) fn peer_is_banned_at(
        &self,
        entries: &ShardedOwnerMap,
        peer: PeerIndex,
        now: std::time::Instant,
    ) -> Result<bool, PeerFenceStageError> {
        let shard = entries.layout.router.shard(b"index/peer", &peer);
        let fence = self.projection_shard(shard).peer_fences.get(&peer);
        if fence.is_some_and(|fence| matches!(fence, PeerIngressFence::Hidden { .. })) {
            return Err(PeerFenceStageError::Stale);
        }
        Ok(fence
            .and_then(PeerIngressFence::logical_lease)
            .is_some_and(|lease| lease.remaining_at(now).is_some()))
    }

    pub(in crate::authority) fn apply_proposed_counts(&mut self, plan: ShardProposedCountPlan) {
        for (shard, _expected, target) in plan.0 {
            self.shard_mut(usize::from(shard)).proposed_count = target;
        }
    }

    pub(in crate::authority) fn proposed_count_plan_is_fresh(
        &self,
        plan: &ShardProposedCountPlan,
    ) -> bool {
        plan.0.iter().all(|(shard, expected, _)| {
            self.projection_shard(usize::from(*shard)).proposed_count == *expected
        })
    }

    /// Verify that an absolute plan compiled by the canonical membership
    /// projector represents exactly the Proposed members in this sealed
    /// owner-removal cohort. The live aggregate base may have advanced due to
    /// a disjoint same-shard commit; Apply subtracts these exact current
    /// owners relatively instead of installing the stale absolute target.
    pub(in crate::authority) fn proposed_removal_plan_matches<'key>(
        &self,
        entries: &ShardedOwnerMap,
        owner_keys: impl IntoIterator<Item = &'key RawTxHash>,
        plan: &ShardProposedCountPlan,
    ) -> bool {
        let mut removed = [0usize; AUTHORITY_SHARD_COUNT];
        for key in owner_keys {
            let shard = entries.owner_shard(key);
            if matches!(
                self.owner(entries, key),
                Some(OwnedTx::Accepted(entry)) if entry.status() == AcceptedStatus::Proposed
            ) {
                let Some(removed) = removed.get_mut(shard) else {
                    return false;
                };
                let Some(next) = removed.checked_add(1) else {
                    return false;
                };
                *removed = next;
            }
        }
        let mut planned = [0usize; AUTHORITY_SHARD_COUNT];
        for (shard, before, after) in &plan.0 {
            let Some(delta) = before.checked_sub(*after) else {
                return false;
            };
            let Some(planned) = planned.get_mut(usize::from(*shard)) else {
                return false;
            };
            *planned = delta;
        }
        planned == removed
    }

    /// Rebase the already-proven removal-only Proposed deltas onto the live
    /// aggregate rows held by this cut. No row is inserted and no allocation
    /// is performed.
    pub(in crate::authority) fn rebase_proposed_removal_plan(
        &self,
        plan: &mut ShardProposedCountPlan,
    ) -> Result<(), ShardProposedCountPlanError> {
        for (shard, expected, target) in &mut plan.0 {
            let removed = expected
                .checked_sub(*target)
                .ok_or(ShardProposedCountPlanError::Projection)?;
            let current = self.projection_shard(usize::from(*shard)).proposed_count;
            let rebased = current
                .checked_sub(removed)
                .ok_or(ShardProposedCountPlanError::Projection)?;
            *expected = current;
            *target = rebased;
        }
        Ok(())
    }

    pub(in crate::authority) fn apply_resource_plan(&mut self, plan: ShardResourcePlan) {
        for (shard, _expected, target) in plan.aggregates {
            self.shard_mut(usize::from(shard)).resources = target;
        }
        for (shard, peer, _expected, target) in plan.peers {
            let rows = &mut self.shard_mut(usize::from(shard)).peer_resources;
            if target == ResourceVector::default() {
                rows.remove(&peer);
            } else {
                rows.insert(peer, target);
            }
        }
    }

    /// Revalidate every resource row captured before the outer generation
    /// guard was released. Exact owner versions do not detect an unrelated
    /// commit that changed an aggregate or peer row in the same physical cut.
    pub(in crate::authority) fn resource_plan_is_fresh(&self, plan: &ShardResourcePlan) -> bool {
        plan.aggregates.iter().all(|(shard, expected, _)| {
            self.projection_shard(usize::from(*shard)).resources == *expected
        }) && plan.peers.iter().all(|(shard, peer, expected, _)| {
            self.projection_shard(usize::from(*shard))
                .peer_resources
                .get(peer)
                .copied()
                .unwrap_or_default()
                == *expected
        })
    }

    /// Rebase a sealed owner-to-Nowhere resource plan onto the aggregate rows
    /// held by this final cut. Every old row must describe subtraction only;
    /// the same exact delta is applied to the current row in place so the
    /// ordinary apply engine composes disjoint same-shard progress.
    pub(in crate::authority) fn rebase_owner_removal_resource_plan(
        &self,
        plan: &mut ShardResourcePlan,
    ) -> Result<(), ResourceError> {
        for (shard, expected, target) in &mut plan.aggregates {
            let removed = expected
                .checked_sub_aggregate(*target)
                .ok_or(ResourceError::ExistingChargeMismatch)?;
            let current = self.projection_shard(usize::from(*shard)).resources;
            let rebased = current
                .checked_sub_aggregate(removed)
                .ok_or(ResourceError::Arithmetic)?;
            *expected = current;
            *target = rebased;
        }
        for (shard, peer, expected, target) in &mut plan.peers {
            let removed = expected
                .checked_sub(*target)
                .ok_or(ResourceError::ExistingChargeMismatch)?;
            let current = self
                .projection_shard(usize::from(*shard))
                .peer_resources
                .get(peer)
                .copied()
                .unwrap_or_default();
            let rebased = current
                .checked_sub(removed)
                .ok_or(ResourceError::Arithmetic)?;
            *expected = current;
            *target = rebased;
        }
        Ok(())
    }

    pub(in crate::authority) fn prepare_owner_source_advance(
        &self,
        counts: ShardOwnerSourceCounts,
    ) -> Option<ShardOwnerSourceAdvance> {
        let mut relay_parents = [None; AUTHORITY_SHARD_COUNT];
        let mut proposals = [None; AUTHORITY_SHARD_COUNT];
        let mut transactions = [None; AUTHORITY_SHARD_COUNT];
        for (
            shard,
            (
                ((relay_parent_count, proposal_count), transaction_count),
                ((relay_parent_target, proposal_target), transaction_target),
            ),
        ) in counts
            .relay_parents
            .into_iter()
            .zip(counts.proposals)
            .zip(counts.transactions)
            .zip(
                relay_parents
                    .iter_mut()
                    .zip(&mut proposals)
                    .zip(&mut transactions),
            )
            .enumerate()
        {
            if relay_parent_count == 0 && proposal_count == 0 && transaction_count == 0 {
                continue;
            }
            let current = self.projection_shard(shard);
            if relay_parent_count != 0 {
                *relay_parent_target = Some(
                    current
                        .relay_parent_source
                        .checked_add(relay_parent_count)?,
                );
            }
            if proposal_count != 0 {
                *proposal_target = Some(
                    current
                        .template_proposals_source
                        .checked_add(proposal_count)?,
                );
            }
            if transaction_count != 0 {
                *transaction_target = Some(
                    current
                        .template_transactions_source
                        .checked_add(transaction_count)?,
                );
            }
        }
        Some(ShardOwnerSourceAdvance {
            relay_parents,
            proposals,
            transactions,
        })
    }

    pub(in crate::authority) fn apply_owner_source_advance(
        &mut self,
        advance: ShardOwnerSourceAdvance,
    ) {
        for (shard, ((relay_parent, proposal), transaction)) in advance
            .relay_parents
            .into_iter()
            .zip(advance.proposals)
            .zip(advance.transactions)
            .enumerate()
        {
            if let Some(target) = relay_parent {
                self.shard_mut(shard).relay_parent_source = target;
            }
            if let Some(target) = proposal {
                self.shard_mut(shard).template_proposals_source = target;
            }
            if let Some(target) = transaction {
                self.shard_mut(shard).template_transactions_source = target;
            }
        }
    }
}

pub(in crate::authority) struct ShardResourcePlan {
    aggregates: Vec<(u8, ResourceTotals, ResourceTotals)>,
    peers: Vec<(u8, PeerIndex, ResourceVector, ResourceVector)>,
}

impl ShardResourcePlan {
    pub(in crate::authority) fn empty() -> Self {
        Self {
            aggregates: Vec::new(),
            peers: Vec::new(),
        }
    }

    pub(in crate::authority) fn validate_peer_targets(
        &self,
        limit: ResourceVector,
    ) -> Result<(), ResourceError> {
        if let Some(peer) = self
            .peers
            .iter()
            .filter_map(|(_, peer, _, target)| (!target.fits(limit)).then_some(*peer))
            .min()
        {
            return Err(ResourceError::PeerLimit(peer));
        }
        Ok(())
    }
}
impl fmt::Debug for ShardedOwnerMap {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ShardedOwnerMap")
            .field("shards", &AUTHORITY_SHARD_COUNT)
            .field("owners", &self.len())
            .finish()
    }
}

impl<'authority> StagedPeerIngressFence<'authority> {
    pub(in crate::authority) fn peer(&self) -> Option<PeerIndex> {
        self.slot.as_ref().map(|slot| slot.lease().peer())
    }

    pub(in crate::authority) fn stage_id(&self) -> Option<u64> {
        self.slot.as_ref().map(StagedPeerBanSlot::stage_id)
    }

    pub(in crate::authority) fn extend_final_write_support(&self, support: &mut ShardWriteSupport) {
        let Some(slot) = self.slot.as_ref() else {
            return;
        };
        support.insert(
            self.entries
                .layout
                .router
                .shard(b"index/peer", &slot.lease().peer()),
        );
        if let Some(victim) = slot.victim() {
            support.insert(
                self.entries
                    .layout
                    .router
                    .shard(b"index/peer", &victim.peer()),
            );
        }
    }

    pub(in crate::authority) fn prestate_is_fresh(&self, cut: &ShardedOwnerWriteCut<'_>) -> bool {
        let Some(slot) = self.slot.as_ref() else {
            return false;
        };
        let peer = slot.lease().peer();
        let shard = self.entries.layout.router.shard(b"index/peer", &peer);
        let target = cut.projection_shard(shard).peer_fences.get(&peer);
        let target_fresh = target.is_some_and(|fence| {
            matches!(
                fence,
                PeerIngressFence::Hidden {
                    stage_id,
                    previous,
                    next,
                } if *stage_id == slot.stage_id()
                    && previous.map(|(lease, _)| lease) == slot.target_previous()
                    && *next == slot.lease()
            )
        });
        target_fresh
            && slot.victim().is_none_or(|victim| {
                let victim_shard = self
                    .entries
                    .layout
                    .router
                    .shard(b"index/peer", &victim.peer());
                cut.projection_shard(victim_shard)
                    .peer_fences
                    .get(&victim.peer())
                    .is_some_and(|fence| {
                        matches!(
                            fence,
                            PeerIngressFence::Active { lease, .. } if *lease == victim
                        )
                    })
            })
    }

    pub(in crate::authority) fn begin_bank_commit(
        mut self,
        cut: &mut ShardedOwnerWriteCut<'_>,
    ) -> Result<BegunPeerIngressFence<'authority>, crate::authority::ban::PeerBanError> {
        let Some(slot) = self.slot.as_mut() else {
            return Err(crate::authority::ban::PeerBanError::Faulted);
        };
        let permit = match slot.begin_in_place() {
            Ok(permit) => permit,
            Err(error) => {
                self.rollback_hidden_in_cut(cut);
                drop(self.slot.take());
                return Err(error);
            }
        };
        drop(self.slot.take());
        Ok(BegunPeerIngressFence {
            entries: self.entries,
            permit,
        })
    }

    fn rollback_hidden_in_cut(&mut self, cut: &mut ShardedOwnerWriteCut<'_>) {
        let Some(slot) = self.slot.as_ref() else {
            return;
        };
        let peer = slot.lease().peer();
        let shard = self.entries.layout.router.shard(b"index/peer", &peer);
        self.rollback_hidden_fence(&mut cut.projection_shard_mut(shard).peer_fences, peer);
    }

    fn rollback_hidden_fence(
        &mut self,
        peer_fences: &mut HashMap<PeerIndex, PeerIngressFence>,
        peer: PeerIndex,
    ) {
        let Some(slot) = self.slot.as_ref() else {
            return;
        };
        let marker_matches = peer_fences
            .get(&peer)
            .and_then(PeerIngressFence::hidden_stage)
            == Some(slot.stage_id());
        if !marker_matches {
            if let Some(slot) = self.slot.as_mut() {
                slot.mark_faulted();
            }
            return;
        }
        let previous = match peer_fences.get(&peer).cloned() {
            Some(PeerIngressFence::Hidden { previous, .. }) => previous,
            None | Some(PeerIngressFence::Absent | PeerIngressFence::Active { .. }) => return,
        };
        if let Some((lease, revision)) = previous {
            peer_fences.insert(peer, PeerIngressFence::Active { lease, revision });
        } else {
            peer_fences.remove(&peer);
        }
    }
}

impl BegunPeerIngressFence<'_> {
    /// This runs only after every fallible preflight and owner prestate check.
    /// The target row already owns its map slot and the optional victim row is
    /// removed in place, so activation performs no allocation.
    pub(in crate::authority) fn activate(self, cut: &mut ShardedOwnerWriteCut<'_>) {
        let peer = self.permit.lease().peer();
        let shard = self.entries.layout.router.shard(b"index/peer", &peer);
        if let Some(fence) = cut.projection_shard_mut(shard).peer_fences.get_mut(&peer) {
            *fence = PeerIngressFence::Active {
                lease: self.permit.lease(),
                revision: self.permit.stage_id(),
            };
        }
        if let Some(victim) = self.permit.victim()
            && victim.peer() != peer
        {
            let victim_shard = self
                .entries
                .layout
                .router
                .shard(b"index/peer", &victim.peer());
            let fences = &mut cut.projection_shard_mut(victim_shard).peer_fences;
            if fences
                .get(&victim.peer())
                .is_some_and(|fence| fence.logical_lease() == Some(victim))
            {
                fences.remove(&victim.peer());
            }
        }
        self.permit.finish();
    }
}

impl Drop for StagedPeerIngressFence<'_> {
    fn drop(&mut self) {
        let Some(slot) = self.slot.as_ref() else {
            return;
        };
        let peer = slot.lease().peer();
        let shard = self.entries.layout.router.shard(b"index/peer", &peer);
        let Some(shard) = self.entries.layout.shards.get(shard) else {
            if let Some(slot) = self.slot.as_mut() {
                slot.mark_faulted();
            }
            return;
        };
        let mut rows = shard.write();
        self.rollback_hidden_fence(&mut rows.peer_fences, peer);
    }
}

impl ShardedOwnerMap {
    pub(in crate::authority) fn new(router: AuthorityShardRouter) -> Self {
        Self {
            layout: Arc::new(AuthorityShardLayout {
                router,
                shards: Box::new(std::array::from_fn(|_| {
                    RwLock::new(AuthorityShard::default())
                })),
                dependency_relations: Box::new(std::array::from_fn(|_| {
                    RwLock::new(DependencyRelationShard::default())
                })),
                dependency_gates: Box::new(std::array::from_fn(|_| RwLock::new(()))),
                #[cfg(test)]
                concurrent_removal_probe: Mutex::new(None),
                #[cfg(test)]
                dependency_maintenance_plan_probe: Mutex::new(None),
                #[cfg(test)]
                membership_dependency_plan_probe: Mutex::new(None),
                #[cfg(test)]
                shared_ingress_probe: Mutex::new(None),
                #[cfg(test)]
                shared_owner_commit_probe: Mutex::new(None),
                #[cfg(test)]
                compute_settlement_commit_probe: Mutex::new(None),
                #[cfg(test)]
                compute_exchange_probe: Mutex::new(None),
                #[cfg(test)]
                generation_payload_swaps: AtomicUsize::new(0),
            }),
        }
    }

    pub(in crate::authority) fn stage_peer_ingress_fence<'authority>(
        &'authority self,
        slot: StagedPeerBanSlot<'authority>,
    ) -> Result<StagedPeerIngressFence<'authority>, PeerFenceStageError> {
        let peer = slot.lease().peer();
        let shard = self.layout.router.shard(b"index/peer", &peer);
        let mut shard = self
            .layout
            .shards
            .get(shard)
            .ok_or(PeerFenceStageError::Stale)?
            .write();
        if !shard.peer_fences.contains_key(&peer) {
            shard.peer_fences.reserve(1);
        }
        let current = shard.peer_fences.get(&peer);
        if current.and_then(PeerIngressFence::hidden_stage).is_some()
            || current.and_then(PeerIngressFence::logical_lease) != slot.target_previous()
        {
            return Err(PeerFenceStageError::Stale);
        }
        let previous = match current {
            None | Some(PeerIngressFence::Absent) => None,
            Some(PeerIngressFence::Active { lease, revision }) => Some((*lease, *revision)),
            Some(PeerIngressFence::Hidden { .. }) => return Err(PeerFenceStageError::Stale),
        };
        shard.peer_fences.insert(
            peer,
            PeerIngressFence::Hidden {
                stage_id: slot.stage_id(),
                previous,
                next: slot.lease(),
            },
        );
        drop(shard);
        Ok(StagedPeerIngressFence {
            entries: self,
            slot: Some(slot),
        })
    }

    #[cfg(test)]
    pub(in crate::authority) fn apply_exclusive_peer_fence(&self, delta: PeerBanDelta) {
        if !delta.records_new() {
            return;
        }
        let peer = delta.lease().peer();
        let mut support = ShardWriteSupport::default();
        support.insert(self.layout.router.shard(b"index/peer", &peer));
        if let Some(victim) = delta.victim() {
            support.insert(self.layout.router.shard(b"index/peer", &victim.peer()));
        }
        let mut cut = self.write_cut(support);
        let target_shard = self.layout.router.shard(b"index/peer", &peer);
        cut.projection_shard_mut(target_shard).peer_fences.insert(
            peer,
            PeerIngressFence::Active {
                lease: delta.lease(),
                revision: delta.stage_id(),
            },
        );
        if let Some(victim) = delta.victim()
            && victim.peer() != peer
        {
            let victim_shard = self.layout.router.shard(b"index/peer", &victim.peer());
            let fences = &mut cut.projection_shard_mut(victim_shard).peer_fences;
            if fences
                .get(&victim.peer())
                .is_some_and(|fence| fence.logical_lease() == Some(victim))
            {
                fences.remove(&victim.peer());
            }
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn peer_ingress_row(&self, peer: PeerIndex) -> Option<PeerIngressRow> {
        let shard = self.layout.router.shard(b"index/peer", &peer);
        self.layout.shards.get(shard)?.read().peer_ingress_row(peer)
    }

    pub(in crate::authority) fn peer_is_banned_at(
        &self,
        peer: PeerIndex,
        now: std::time::Instant,
    ) -> Result<bool, PeerFenceStageError> {
        let shard = self.layout.router.shard(b"index/peer", &peer);
        let Some(shard) = self.layout.shards.get(shard) else {
            return Err(PeerFenceStageError::Stale);
        };
        let shard = shard.read();
        let fence = shard.peer_fences.get(&peer);
        if fence.is_some_and(|fence| matches!(fence, PeerIngressFence::Hidden { .. })) {
            return Err(PeerFenceStageError::Stale);
        }
        Ok(fence
            .and_then(PeerIngressFence::logical_lease)
            .is_some_and(|lease| lease.remaining_at(now).is_some()))
    }

    /// Swap only generation-owned shard payloads into one already-built
    /// carrier. The live routed locks and peer-fence maps remain in place, so
    /// active ban truth survives without copying or allocating. The carrier
    /// receives the complete retired owner/index/resource payload and is
    /// destroyed only after the outer authority guard opens.
    #[expect(
        clippy::indexing_slicing,
        reason = "from_fn enumerates exactly the two fixed 64-shard arrays before any payload swap"
    )]
    pub(in crate::authority) fn swap_generation_payload_with(&self, carrier: &ShardedOwnerMap) {
        // Relations are always acquired before owners. The outer generation
        // writer has already excluded ordinary shared operations; preserving
        // the production lock order here keeps that proof true for any future
        // caller as well.
        let mut live_relations: [RwLockWriteGuard<'_, DependencyRelationShard>;
            AUTHORITY_SHARD_COUNT] =
            std::array::from_fn(|shard| self.layout.dependency_relations[shard].write());
        let mut carrier_relations: [RwLockWriteGuard<'_, DependencyRelationShard>;
            AUTHORITY_SHARD_COUNT] =
            std::array::from_fn(|shard| carrier.layout.dependency_relations[shard].write());
        let mut live: [RwLockWriteGuard<'_, AuthorityShard>; AUTHORITY_SHARD_COUNT] =
            std::array::from_fn(|shard| self.layout.shards[shard].write());
        // The carrier is private to FreshGeneration and cannot be observed or
        // locked by another task. Acquire every live guard first so even a
        // future caller outside the current store writer cannot observe a
        // half-old/half-new generation.
        let mut carrier: [RwLockWriteGuard<'_, AuthorityShard>; AUTHORITY_SHARD_COUNT] =
            std::array::from_fn(|shard| carrier.layout.shards[shard].write());
        for (live, carrier) in live_relations.iter_mut().zip(&mut carrier_relations) {
            std::mem::swap(&mut live.rows, &mut carrier.rows);
        }
        for (live, carrier) in live.iter_mut().zip(&mut carrier) {
            std::mem::swap(&mut live.generation, &mut carrier.generation);
            #[cfg(test)]
            self.layout
                .generation_payload_swaps
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn peer_ingress_row_count_for_test(&self) -> usize {
        self.layout
            .shards
            .iter()
            .map(|shard| {
                let shard = shard.read();
                shard.peer_ingress_owners.len()
                    + shard
                        .peer_fences
                        .keys()
                        .filter(|peer| !shard.peer_ingress_owners.contains_key(peer))
                        .count()
            })
            .sum()
    }

    #[cfg(test)]
    pub(in crate::authority) fn same_layout_for_test(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.layout, &other.layout)
    }

    #[cfg(test)]
    pub(in crate::authority) fn generation_payload_swaps_for_test(&self) -> usize {
        self.layout.generation_payload_swaps.load(Ordering::Relaxed)
    }

    pub(in crate::authority) fn router(&self) -> AuthorityShardRouter {
        self.layout.router.clone()
    }

    #[cfg(test)]
    pub(in crate::authority) fn set_concurrent_removal_probe(
        &self,
        probe: Option<Arc<ConcurrentRemovalProbe>>,
    ) {
        *self.layout.concurrent_removal_probe.lock() = probe;
    }

    #[cfg(test)]
    pub(in crate::authority) fn enter_concurrent_removal_probe(&self) {
        let probe = self.layout.concurrent_removal_probe.lock().clone();
        if let Some(probe) = probe {
            probe.enter();
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn set_dependency_maintenance_plan_probe(
        &self,
        probe: Option<Arc<ConcurrentRemovalProbe>>,
    ) {
        *self.layout.dependency_maintenance_plan_probe.lock() = probe;
    }

    #[cfg(test)]
    pub(in crate::authority) fn enter_dependency_maintenance_plan_probe(&self) {
        let probe = self.layout.dependency_maintenance_plan_probe.lock().clone();
        if let Some(probe) = probe {
            probe.enter();
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn set_membership_dependency_plan_probe(
        &self,
        probe: Option<Arc<ConcurrentRemovalProbe>>,
    ) {
        *self.layout.membership_dependency_plan_probe.lock() = probe;
    }

    #[cfg(test)]
    pub(in crate::authority) fn enter_membership_dependency_plan_probe(&self) {
        let probe = self.layout.membership_dependency_plan_probe.lock().clone();
        if let Some(probe) = probe {
            probe.enter();
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn set_shared_ingress_probe(
        &self,
        phase: SharedIngressProbePhase,
        probe: Option<Arc<ConcurrentRemovalProbe>>,
    ) {
        *self.layout.shared_ingress_probe.lock() = probe.map(|probe| (phase, probe));
    }

    #[cfg(test)]
    pub(in crate::authority) fn enter_shared_ingress_probe(&self, phase: SharedIngressProbePhase) {
        let probe = self
            .layout
            .shared_ingress_probe
            .lock()
            .as_ref()
            .filter(|(expected, _)| *expected == phase)
            .map(|(_, probe)| Arc::clone(probe));
        if let Some(probe) = probe {
            probe.enter();
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn set_shared_owner_commit_probe(
        &self,
        probe: Option<Arc<ConcurrentRemovalProbe>>,
    ) {
        *self.layout.shared_owner_commit_probe.lock() = probe;
    }

    #[cfg(test)]
    pub(in crate::authority) fn enter_shared_owner_commit_probe(&self) {
        let probe = {
            let configured = self.layout.shared_owner_commit_probe.lock();
            configured.clone()
        };
        if let Some(probe) = probe {
            probe.enter();
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn set_compute_settlement_commit_probe(
        &self,
        probe: Option<Arc<ConcurrentRemovalProbe>>,
    ) {
        *self.layout.compute_settlement_commit_probe.lock() = probe;
    }

    #[cfg(test)]
    pub(in crate::authority) fn enter_compute_settlement_commit_probe(&self) {
        let probe = self.layout.compute_settlement_commit_probe.lock().clone();
        if let Some(probe) = probe {
            probe.enter();
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn set_compute_exchange_probe(
        &self,
        phase: ComputeExchangeProbePhase,
        probe: Option<Arc<ConcurrentRemovalProbe>>,
    ) {
        *self.layout.compute_exchange_probe.lock() = probe.map(|probe| (phase, probe));
    }

    #[cfg(test)]
    pub(in crate::authority) fn enter_compute_exchange_probe(
        &self,
        phase: ComputeExchangeProbePhase,
    ) {
        let probe = self
            .layout
            .compute_exchange_probe
            .lock()
            .as_ref()
            .filter(|(expected, _)| *expected == phase)
            .map(|(_, probe)| Arc::clone(probe));
        if let Some(probe) = probe {
            probe.enter();
        }
    }

    pub(in crate::authority) fn owner_resource_write_support<'key>(
        &self,
        owner_keys: impl IntoIterator<Item = &'key RawTxHash>,
        proposed_counts: &ShardProposedCountPlan,
        resources: &ShardResourcePlan,
    ) -> ShardWriteSupport {
        let mut support = ShardWriteSupport::default();
        for key in owner_keys {
            support.insert(self.layout.router.owner(key));
        }
        for (shard, _, _) in &proposed_counts.0 {
            support.insert(usize::from(*shard));
        }
        for (shard, _, _) in &resources.aggregates {
            support.insert(usize::from(*shard));
        }
        for (shard, _, _, _) in &resources.peers {
            support.insert(usize::from(*shard));
        }
        support
    }

    /// Acquire dependency conflict classes before any owner shard. A write
    /// request dominates a read request for the same fixed class, preventing
    /// self-upgrade while the ascending walk gives every mixed operation one
    /// lock order.
    #[expect(
        clippy::indexing_slicing,
        reason = "the loop is bounded by the fixed dependency-gate array"
    )]
    pub(in crate::authority) fn dependency_gate_cut(
        &self,
        support: DependencyGateSupport,
    ) -> DependencyGateCut<'_> {
        let mut reads = std::array::from_fn(|_| None);
        let mut writes = std::array::from_fn(|_| None);
        for shard in 0..AUTHORITY_SHARD_COUNT {
            if support.writes(shard) {
                writes[shard] = Some(self.layout.dependency_gates[shard].write());
            } else if support.reads(shard) {
                reads[shard] = Some(self.layout.dependency_gates[shard].read());
            }
        }
        DependencyGateCut {
            _reads: reads,
            _writes: writes,
        }
    }

    #[cfg(test)]
    #[expect(
        clippy::indexing_slicing,
        reason = "the loop is bounded by the fixed dependency-gate array"
    )]
    pub(in crate::authority) fn try_dependency_gate_cut(
        &self,
        support: DependencyGateSupport,
    ) -> Option<DependencyGateCut<'_>> {
        let mut unavailable = false;
        let mut reads = std::array::from_fn(|_| None);
        let mut writes = std::array::from_fn(|_| None);
        for shard in 0..AUTHORITY_SHARD_COUNT {
            if support.writes(shard) {
                match self.layout.dependency_gates[shard].try_write() {
                    Some(guard) => writes[shard] = Some(guard),
                    None => unavailable = true,
                }
            } else if support.reads(shard) {
                match self.layout.dependency_gates[shard].try_read() {
                    Some(guard) => reads[shard] = Some(guard),
                    None => unavailable = true,
                }
            }
        }
        (!unavailable).then_some(DependencyGateCut {
            _reads: reads,
            _writes: writes,
        })
    }

    #[cfg(test)]
    pub(in crate::authority) fn owner_write_support<'key>(
        &self,
        owner_keys: impl IntoIterator<Item = &'key RawTxHash>,
    ) -> ShardWriteSupport {
        let mut support = ShardWriteSupport::default();
        for key in owner_keys {
            support.insert(self.layout.router.owner(key));
        }
        support
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "array::from_fn supplies only indices in the fixed 64-shard array"
    )]
    pub(in crate::authority) fn write_cut(
        &self,
        support: ShardWriteSupport,
    ) -> ShardedOwnerWriteCut<'_> {
        ShardedOwnerWriteCut {
            reads: std::array::from_fn(|_| None),
            writes: std::array::from_fn(|shard| {
                support
                    .contains(shard)
                    .then(|| self.layout.shards[shard].write())
            }),
        }
    }

    /// Hold only the physical rows read by one retained-ingress fold. This is
    /// the coherent read-side counterpart of the routed shared owner cut: raw
    /// owners, proposal identity rows and peer-resource rows cannot move while
    /// the no-owner decision is staged, but disjoint shards remain writable.
    pub(in crate::authority) fn retained_ingress_read_cut(
        &self,
        owner_keys: &[RawTxHash],
        proposals: &[ProposalId],
        peers: impl IntoIterator<Item = PeerIndex>,
    ) -> ShardedOwnerWriteCut<'_> {
        let mut reads = ShardReadSupport::default();
        for key in owner_keys {
            reads.insert(self.layout.router.owner(key));
        }
        for proposal in proposals {
            reads.insert(self.layout.router.shard(b"index/proposal", proposal));
        }
        for peer in peers {
            reads.insert(self.layout.router.peer_resource(&peer));
            reads.insert(self.layout.router.shard(b"index/peer", &peer));
        }
        // Old-owner peer attribution is discovered from the same cut that
        // protects the owner observation. If it names a previously unseen
        // physical peer shard, release and monotonically expand the support.
        // Each pass adds at least one of the fixed 64 shards, so the closure is
        // bounded without a retry budget or global fallback.
        loop {
            let cut = self.mixed_cut(reads, ShardWriteSupport::default());
            let mut expanded = reads;
            for key in owner_keys {
                if let Some(peer) = cut.owner(self, key).and_then(|owner| match owner {
                    OwnedTx::PreAccepted(entry) => entry.source.ingress_peer(),
                    OwnedTx::Accepted(_) | OwnedTx::ReplacementHistory(_) => None,
                }) {
                    expanded.insert(self.layout.router.peer_resource(&peer));
                    expanded.insert(self.layout.router.shard(b"index/peer", &peer));
                }
            }
            if expanded.0 == reads.0 {
                return cut;
            }
            drop(cut);
            reads = expanded;
        }
    }

    /// Acquire one complete mixed support in canonical shard order. Read-only
    /// premises may overlap each other; every write dominates a read for the
    /// same shard. This is the physical basis for independent transactions
    /// sharing immutable cell-deps without weakening OCC freshness.
    #[expect(
        clippy::indexing_slicing,
        reason = "the loop is bounded by the same fixed 64-shard array length"
    )]
    pub(in crate::authority) fn mixed_cut(
        &self,
        reads: ShardReadSupport,
        writes: ShardWriteSupport,
    ) -> ShardedOwnerWriteCut<'_> {
        let mut read_guards: [Option<RwLockReadGuard<'_, AuthorityShard>>; AUTHORITY_SHARD_COUNT] =
            std::array::from_fn(|_| None);
        let mut write_guards: [Option<RwLockWriteGuard<'_, AuthorityShard>>;
            AUTHORITY_SHARD_COUNT] = std::array::from_fn(|_| None);
        for shard in 0..AUTHORITY_SHARD_COUNT {
            if writes.contains(shard) {
                write_guards[shard] = Some(self.layout.shards[shard].write());
            } else if reads.contains(shard) {
                read_guards[shard] = Some(self.layout.shards[shard].read());
            }
        }
        ShardedOwnerWriteCut {
            reads: read_guards,
            writes: write_guards,
        }
    }

    /// Acquire one relation support in canonical shard order. Callers acquire
    /// dependency gates first and owner cuts only after this returns.
    #[expect(
        clippy::indexing_slicing,
        reason = "the loop is bounded by the same fixed 64-shard relation bank"
    )]
    pub(in crate::authority) fn dependency_relation_mixed_cut(
        &self,
        reads: ShardReadSupport,
        writes: ShardWriteSupport,
    ) -> ShardedDependencyRelationWriteCut<'_> {
        let mut read_guards = std::array::from_fn(|_| None);
        let mut write_guards = std::array::from_fn(|_| None);
        for shard in 0..AUTHORITY_SHARD_COUNT {
            if writes.contains(shard) {
                write_guards[shard] = Some(self.layout.dependency_relations[shard].write());
            } else if reads.contains(shard) {
                read_guards[shard] = Some(self.layout.dependency_relations[shard].read());
            }
        }
        ShardedDependencyRelationWriteCut {
            reads: read_guards,
            writes: write_guards,
        }
    }

    /// Read one relation partition. Key-wide callers must hold the exact-key
    /// gate and drop this guard before advancing to the next shard.
    #[expect(
        clippy::indexing_slicing,
        reason = "the caller iterates or routes within the fixed 64-shard relation bank"
    )]
    pub(in crate::authority) fn dependency_relation_shard_read(
        &self,
        shard: usize,
    ) -> RwLockReadGuard<'_, DependencyRelationShard> {
        self.layout.dependency_relations[shard].read()
    }

    #[cfg(test)]
    pub(in crate::authority) fn try_write_cut(
        &self,
        support: ShardWriteSupport,
    ) -> Option<ShardedOwnerWriteCut<'_>> {
        let mut unavailable = false;
        let writes = std::array::from_fn(|shard| {
            if !support.contains(shard) {
                return None;
            }
            match self.layout.shards[shard].try_write() {
                Some(guard) => Some(guard),
                None => {
                    unavailable = true;
                    None
                }
            }
        });
        (!unavailable).then_some(ShardedOwnerWriteCut {
            reads: std::array::from_fn(|_| None),
            writes,
        })
    }

    pub(in crate::authority) fn owner_shard(&self, key: &RawTxHash) -> usize {
        self.layout.router.owner(key)
    }

    /// Capture the complete owner prestate and the same bounded removal
    /// witness from one physical shard guard. A positive version proves the
    /// current contents while this witness rules out Present -> Absent ->
    /// Present ABA; an absent row uses it as the ordinary vacancy proof.
    /// Taking either fact in a separate read would recreate a torn premise
    /// before the final OCC cut.
    #[expect(
        clippy::indexing_slicing,
        reason = "owner() masks to the fixed 64-entry array range"
    )]
    pub(in crate::authority) fn owner_and_vacancy_revision(
        &self,
        key: &RawTxHash,
    ) -> (Option<OwnedTx>, Option<OwnerShardRemovalRevision>) {
        let shard = self.layout.shards[self.layout.router.owner(key)].read();
        match shard.owners.get(key) {
            Some(owner) => (
                Some(owner.clone()),
                shard.owner_removal_revision.vacancy_witness(),
            ),
            None => (None, shard.owner_removal_revision.vacancy_witness()),
        }
    }

    /// Capture only the incarnation facts needed by optimistic policy. This
    /// avoids cloning the owner payload on the dominant fact-only read while
    /// pairing absence with the same guarded vacancy revision.
    #[expect(
        clippy::indexing_slicing,
        reason = "owner() masks to the fixed 64-entry array range"
    )]
    pub(in crate::authority) fn owner_fact_and_vacancy_revision(
        &self,
        key: &RawTxHash,
    ) -> (
        Option<(super::state::EntryVersion, OwnerEntryKind)>,
        Option<OwnerShardRemovalRevision>,
    ) {
        let shard = self.layout.shards[self.layout.router.owner(key)].read();
        let fact = shard.owners.get(key).map(|owner| {
            let kind = match owner {
                OwnedTx::PreAccepted(_) => OwnerEntryKind::PreAccepted,
                OwnedTx::Accepted(_) => OwnerEntryKind::Accepted,
                OwnedTx::ReplacementHistory(_) => OwnerEntryKind::ReplacementHistory,
            };
            (owner.record().version, kind)
        });
        (fact, shard.owner_removal_revision.vacancy_witness())
    }

    pub(in crate::authority) fn plan_owner_sources<'entry>(
        &self,
        replacements: impl IntoIterator<
            Item = (
                &'entry RawTxHash,
                Option<&'entry OwnedTx>,
                Option<&'entry OwnedTx>,
            ),
        >,
    ) -> Option<ShardOwnerSourcePlan> {
        let mut counts = ShardOwnerSourceCounts::none();
        for (key, before, after) in replacements {
            let relay_parent = AuthoritySourceVersions::relay_parent_change(before, after);
            let (proposal, transaction) =
                AuthoritySourceVersions::template_selection_change(before, after);
            counts.record(self.owner_shard(key), relay_parent, proposal, transaction)?;
        }
        let exclusive_advance = self.prepare_owner_source_advance(counts)?;
        Some(ShardOwnerSourcePlan {
            counts,
            exclusive_advance,
        })
    }

    fn prepare_owner_source_advance(
        &self,
        counts: ShardOwnerSourceCounts,
    ) -> Option<ShardOwnerSourceAdvance> {
        let mut relay_parents = [None; AUTHORITY_SHARD_COUNT];
        let mut proposals = [None; AUTHORITY_SHARD_COUNT];
        let mut transactions = [None; AUTHORITY_SHARD_COUNT];
        for (
            (current, ((relay_parent_count, proposal_count), transaction_count)),
            ((relay_parent_target, proposal_target), transaction_target),
        ) in self
            .layout
            .shards
            .iter()
            .zip(
                counts
                    .relay_parents
                    .into_iter()
                    .zip(counts.proposals)
                    .zip(counts.transactions),
            )
            .zip(
                relay_parents
                    .iter_mut()
                    .zip(&mut proposals)
                    .zip(&mut transactions),
            )
        {
            if relay_parent_count == 0 && proposal_count == 0 && transaction_count == 0 {
                continue;
            }
            let current = current.read();
            if relay_parent_count != 0 {
                *relay_parent_target = Some(
                    current
                        .relay_parent_source
                        .checked_add(relay_parent_count)?,
                );
            }
            if proposal_count != 0 {
                *proposal_target = Some(
                    current
                        .template_proposals_source
                        .checked_add(proposal_count)?,
                );
            }
            if transaction_count != 0 {
                *transaction_target = Some(
                    current
                        .template_transactions_source
                        .checked_add(transaction_count)?,
                );
            }
        }
        Some(ShardOwnerSourceAdvance {
            relay_parents,
            proposals,
            transactions,
        })
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "owner() masks to the fixed 64-entry array range"
    )]
    pub(in crate::authority) fn get(&self, key: &RawTxHash) -> Option<ShardedOwnerReadGuard<'_>> {
        RwLockReadGuard::try_map(
            self.layout.shards[self.layout.router.owner(key)].read(),
            |shard| shard.owners.get(key),
        )
        .ok()
        .map(|owner| ShardedOwnerReadGuard { owner })
    }

    /// Observe an owner and its mandatory co-located child row under one
    /// shard guard. The boolean distinguishes optimistic owner change from a
    /// stable Accepted-owner/projection contradiction.
    #[expect(
        clippy::indexing_slicing,
        reason = "owner() masks to the fixed 64-entry array range"
    )]
    pub(in crate::authority) fn accepted_child_row_observation(
        &self,
        key: &RawTxHash,
    ) -> (bool, Option<HashSet<RawTxHash>>) {
        let shard = self.layout.shards[self.layout.router.owner(key)].read();
        (
            matches!(shard.owners.get(key), Some(OwnedTx::Accepted(_))),
            shard.children.get(key).cloned(),
        )
    }

    pub(in crate::authority) fn contains_key(&self, key: &RawTxHash) -> bool {
        self.get(key).is_some()
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "owner() masks to the fixed 64-entry array range"
    )]
    #[cfg(test)]
    pub(in crate::authority) fn insert(
        &mut self,
        key: RawTxHash,
        owner: OwnedTx,
    ) -> Option<OwnedTx> {
        let shard = self.layout.router.owner(&key);
        self.layout.shards[shard].write().owners.insert(key, owner)
    }

    pub(in crate::authority) fn len(&self) -> usize {
        self.layout
            .shards
            .iter()
            .map(|shard| shard.read().owners.len())
            .sum()
    }

    #[cfg(test)]
    pub(in crate::authority) fn relay_parent_sources(&self) -> [u64; AUTHORITY_SHARD_COUNT] {
        self.read_all().relay_parent_sources()
    }

    #[cfg(test)]
    pub(in crate::authority) fn snapshot_for_test(&self) -> Vec<(RawTxHash, OwnedTx)> {
        let owners = self.read_all();
        owners
            .iter()
            .map(|(hash, owner)| (hash.clone(), owner.clone()))
            .collect()
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "owner() masks to the fixed 64-entry array range"
    )]
    pub(in crate::authority) fn reserve_keys<'key>(
        &self,
        keys: impl IntoIterator<Item = &'key RawTxHash>,
    ) {
        let mut additional = [0usize; AUTHORITY_SHARD_COUNT];
        for key in keys {
            let shard = self.layout.router.owner(key);
            additional[shard] = additional[shard].saturating_add(1);
        }
        for (shard, additional) in self.layout.shards.iter().zip(additional) {
            if additional != 0 {
                shard.write().owners.reserve(additional);
            }
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn status_counts(&self) -> Option<super::plan::StatusCounts> {
        let owners = self.read_all();
        let counts = owners.status_counts()?;
        (owners.proposed_count()? == counts.proposed).then_some(counts)
    }

    pub(in crate::authority) fn resource_totals(&self) -> Option<ResourceTotals> {
        self.layout
            .shards
            .iter()
            .try_fold(ResourceTotals::default(), |totals, shard| {
                totals.checked_add_aggregate(shard.read().resources)
            })
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "peer_resource() masks to the fixed 64-entry array range"
    )]
    pub(in crate::authority) fn peer_resource(&self, peer: PeerIndex) -> ResourceVector {
        self.layout.shards[self.layout.router.peer_resource(&peer)]
            .read()
            .peer_resources
            .get(&peer)
            .copied()
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(in crate::authority) fn peer_resources_snapshot_for_test(
        &self,
    ) -> HashMap<PeerIndex, ResourceVector> {
        let mut peers = HashMap::new();
        for shard in &self.layout.shards[..] {
            let shard = shard.read();
            peers.extend(
                shard
                    .peer_resources
                    .iter()
                    .map(|(peer, resources)| (*peer, *resources)),
            );
        }
        peers
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "router outputs mask to the fixed 64-entry array range"
    )]
    pub(in crate::authority) fn plan_resource_transitions<'change>(
        &self,
        changes: impl IntoIterator<Item = (&'change RawTxHash, ChargeProjection, ChargeProjection)>,
    ) -> Result<ShardResourcePlan, ResourceError> {
        let mut aggregate_targets = [None; AUTHORITY_SHARD_COUNT];
        let mut peer_targets = HashMap::new();
        for (key, before, after) in changes {
            let owner_shard = self.layout.router.owner(key);
            let (_, aggregate) = aggregate_targets[owner_shard].get_or_insert_with(|| {
                let current = self.layout.shards[owner_shard].read().resources;
                (current, current)
            });
            *aggregate = aggregate.checked_remove(before)?.checked_add(after)?;

            for (peer, resources, add) in before
                .peer
                .map(|(peer, resources)| (peer, resources, false))
                .into_iter()
                .chain(after.peer.map(|(peer, resources)| (peer, resources, true)))
            {
                let (_, target) = peer_targets.entry(peer).or_insert_with(|| {
                    let current = self.peer_resource(peer);
                    (current, current)
                });
                *target = if add {
                    target
                        .checked_add(resources)
                        .ok_or(ResourceError::Arithmetic)?
                } else {
                    target
                        .checked_sub(resources)
                        .ok_or(ResourceError::Arithmetic)?
                };
            }
        }

        let mut peer_insertions = [0usize; AUTHORITY_SHARD_COUNT];
        for (peer, (_, target)) in &peer_targets {
            let shard = self.layout.router.peer_resource(peer);
            if *target != ResourceVector::default()
                && !self.layout.shards[shard]
                    .read()
                    .peer_resources
                    .contains_key(peer)
            {
                peer_insertions[shard] = peer_insertions[shard]
                    .checked_add(1)
                    .ok_or(ResourceError::Arithmetic)?;
            }
        }
        for (shard, additional) in self.layout.shards.iter().zip(peer_insertions) {
            if additional == 0 {
                continue;
            }
            shard.write().peer_resources.reserve(additional);
        }

        let mut aggregates = Vec::with_capacity(AUTHORITY_SHARD_COUNT);
        aggregates.extend(aggregate_targets.into_iter().enumerate().filter_map(
            |(shard, target)| target.map(|(expected, target)| (shard as u8, expected, target)),
        ));
        let mut peers = Vec::with_capacity(peer_targets.len());
        peers.extend(peer_targets.into_iter().map(|(peer, (expected, target))| {
            (
                self.layout.router.peer_resource(&peer) as u8,
                peer,
                expected,
                target,
            )
        }));
        Ok(ShardResourcePlan { aggregates, peers })
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "planned shard ids originate only from masked router outputs or fixed enumeration"
    )]
    #[cfg(test)]
    pub(in crate::authority) fn apply_resource_plan(&mut self, plan: ShardResourcePlan) {
        for (shard, _expected, target) in plan.aggregates {
            self.layout.shards[usize::from(shard)].write().resources = target;
        }
        for (shard, peer, _expected, target) in plan.peers {
            let mut shard = self.layout.shards[usize::from(shard)].write();
            let rows = &mut shard.peer_resources;
            if target == ResourceVector::default() {
                rows.remove(&peer);
            } else {
                rows.insert(peer, target);
            }
        }
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "owner() masks to the fixed 64-entry array range"
    )]
    pub(in crate::authority) fn plan_proposed_counts<'change>(
        &self,
        changes: impl IntoIterator<
            Item = (
                &'change RawTxHash,
                Option<AcceptedStatus>,
                Option<AcceptedStatus>,
            ),
        >,
    ) -> Result<ShardProposedCountPlan, ShardProposedCountPlanError> {
        let mut targets = [None; AUTHORITY_SHARD_COUNT];
        let mut changed_shards = 0usize;
        for (key, before, after) in changes {
            let before = before == Some(AcceptedStatus::Proposed);
            let after = after == Some(AcceptedStatus::Proposed);
            if before == after {
                continue;
            }
            let shard = self.layout.router.owner(key);
            let (base, target) = targets[shard].get_or_insert_with(|| {
                let current = self.layout.shards[shard].read().proposed_count;
                (current, current)
            });
            let was_changed = *base != *target;
            if before {
                *target = target
                    .checked_sub(1)
                    .ok_or(ShardProposedCountPlanError::Projection)?;
            }
            if after {
                *target = target
                    .checked_add(1)
                    .ok_or(ShardProposedCountPlanError::Arithmetic)?;
            }
            let is_changed = *base != *target;
            match (was_changed, is_changed) {
                (false, true) => {
                    changed_shards = changed_shards
                        .checked_add(1)
                        .ok_or(ShardProposedCountPlanError::Arithmetic)?;
                }
                (true, false) => {
                    changed_shards = changed_shards
                        .checked_sub(1)
                        .ok_or(ShardProposedCountPlanError::Projection)?;
                }
                (false, false) | (true, true) => {}
            }
        }
        let mut planned = Vec::with_capacity(changed_shards);
        planned.extend(
            targets
                .into_iter()
                .enumerate()
                .filter_map(|(shard, target)| {
                    target
                        .filter(|(before, after)| before != after)
                        .map(|(before, after)| (shard as u8, before, after))
                }),
        );
        Ok(ShardProposedCountPlan(planned))
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "array::from_fn supplies only indices in the fixed 64-shard array"
    )]
    pub(in crate::authority) fn read_all(&self) -> ShardedOwnerReadCut<'_> {
        ShardedOwnerReadCut {
            router: self.layout.router.clone(),
            shards: std::array::from_fn(|shard| self.layout.shards[shard].read()),
        }
    }

    /// Acquire the complete relation bank before any owner cut for a coherent
    /// test-only snapshot. Ordinary production folds never use this global
    /// cut.
    #[cfg(test)]
    #[expect(
        clippy::indexing_slicing,
        reason = "array::from_fn supplies only indices in the fixed 64-shard relation bank"
    )]
    pub(in crate::authority) fn dependency_relations_read_all(
        &self,
    ) -> ShardedDependencyRelationReadCut<'_> {
        ShardedDependencyRelationReadCut {
            shards: std::array::from_fn(|shard| self.layout.dependency_relations[shard].read()),
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn try_read_all(&self) -> Option<ShardedOwnerReadCut<'_>> {
        let guards = self
            .layout
            .shards
            .iter()
            .map(RwLock::try_read)
            .collect::<Option<Vec<_>>>()?;
        let shards = guards.try_into().ok()?;
        Some(ShardedOwnerReadCut {
            router: self.layout.router.clone(),
            shards,
        })
    }
}

impl ShardedOwnerReadCut<'_> {
    #[expect(
        clippy::indexing_slicing,
        reason = "the caller routes through the fixed 64-shard authority layout"
    )]
    pub(in crate::authority) fn projection_shard(&self, shard: usize) -> &AuthorityShard {
        &self.shards[shard]
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "owner() masks to the fixed 64-entry array range"
    )]
    pub(in crate::authority) fn get(&self, key: &RawTxHash) -> Option<&OwnedTx> {
        self.shards[self.router.owner(key)].owners.get(key)
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "the sole shard router masks every domain/key result to the fixed 64-shard array"
    )]
    pub(in crate::authority) fn membership_spender(&self, input: &OutPoint) -> Option<&RawTxHash> {
        self.shards[self.router.shard(b"membership/spender", input)]
            .spenders
            .get(input)
    }

    pub(in crate::authority) fn len(&self) -> usize {
        self.shards.iter().map(|shard| shard.owners.len()).sum()
    }

    pub(in crate::authority) fn proposed_count(&self) -> Option<usize> {
        self.shards.iter().try_fold(0usize, |total, shard| {
            total.checked_add(shard.proposed_count)
        })
    }

    pub(in crate::authority) fn accepted_count(&self) -> Option<usize> {
        self.shards.iter().try_fold(0usize, |total, shard| {
            total.checked_add(shard.resources.accepted.entries)
        })
    }

    #[cfg(test)]
    pub(in crate::authority) fn status_counts(&self) -> Option<super::plan::StatusCounts> {
        self.values().try_fold(
            super::plan::StatusCounts::default(),
            |counts, owner| match owner {
                OwnedTx::Accepted(entry) => counts.checked_add(entry.status()),
                OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_) => Some(counts),
            },
        )
    }

    pub(in crate::authority) fn accepted_resources(&self) -> Option<AcceptedResources> {
        self.shards
            .iter()
            .try_fold(AcceptedResources::default(), |total, shard| {
                total.checked_add(shard.resources.accepted)
            })
    }

    /// Exact generation-local relayer source vector captured by the same
    /// fixed full-shard cut used for deadline rows and Waiting owners.
    pub(in crate::authority) fn relay_parent_sources(&self) -> [u64; AUTHORITY_SHARD_COUNT] {
        self.shards
            .each_ref()
            .map(|shard| shard.relay_parent_source)
    }

    /// Scan one bounded Remote deadline page without reacquiring shard locks.
    /// The caller therefore binds the page, owner rows and source vector to
    /// this one coherent physical cut.
    pub(in crate::authority) fn remote_page_into(
        &self,
        after: Option<&DueRemote>,
        limit: usize,
        page: &mut Vec<DueRemote>,
    ) -> Result<bool, super::indexes::IndexError> {
        page.clear();
        let total = self.shards.iter().map(|shard| shard.deadlines.len()).sum();
        if page.capacity() < limit.min(total) {
            return Err(super::indexes::IndexError::Allocation);
        }
        let after = after.map(|cursor| DeadlineKey {
            expires_at: cursor.expires_at,
            hash: cursor.hash.clone(),
        });
        let start = after
            .as_ref()
            .map_or(std::ops::Bound::Unbounded, std::ops::Bound::Excluded);
        let mut rows: [std::collections::btree_set::Range<'_, DeadlineKey>; AUTHORITY_SHARD_COUNT] =
            self.shards
                .each_ref()
                .map(|shard| shard.deadlines.range((start, std::ops::Bound::Unbounded)));
        let mut heads: [Option<&DeadlineKey>; AUTHORITY_SHARD_COUNT] =
            rows.each_mut().map(|row| row.next());
        while page.len() < limit {
            let Some((shard, expires_at, hash)) = heads
                .iter()
                .enumerate()
                .filter_map(|(shard, row)| row.map(|row| (shard, row)))
                .min_by(|(_, left), (_, right)| left.cmp(right))
                .map(|(shard, deadline)| (shard, deadline.expires_at, deadline.hash.clone()))
            else {
                break;
            };
            let (head, row) = heads
                .iter_mut()
                .zip(rows.iter_mut())
                .nth(shard)
                .ok_or(super::indexes::IndexError::Projection)?;
            *head = row.next();
            page.push(DueRemote { expires_at, hash });
        }
        Ok(heads.iter().any(Option::is_some))
    }

    pub(in crate::authority) fn template_sources(
        &self,
        mut base: PoolTemplateVersions,
    ) -> PoolTemplateVersions {
        base.proposals = base.proposals.with_shards(
            self.shards
                .each_ref()
                .map(|shard| shard.template_proposals_source),
        );
        base.transactions = base.transactions.with_shards(
            self.shards
                .each_ref()
                .map(|shard| shard.template_transactions_source),
        );
        base
    }

    pub(in crate::authority) fn membership_parents(
        &self,
        key: &RawTxHash,
    ) -> Option<&HashSet<RawTxHash>> {
        let shard = self.shards.get(self.router.owner(key))?;
        shard.parents.get(key)
    }

    pub(in crate::authority) fn proposal_owner(&self, proposal: &ProposalId) -> Option<&RawTxHash> {
        self.shards
            .get(self.router.shard(b"index/proposal", proposal))?
            .proposals
            .get(proposal)
    }

    pub(in crate::authority) fn membership_ancestor(
        &self,
        key: &RawTxHash,
    ) -> Option<AncestorAggregate> {
        self.shards
            .get(self.router.owner(key))?
            .ancestor_aggregates
            .get(key)
            .copied()
    }

    pub(in crate::authority) fn membership_descendant(
        &self,
        key: &RawTxHash,
    ) -> Option<DescendantAggregate> {
        self.shards
            .get(self.router.owner(key))?
            .descendant_aggregates
            .get(key)
            .copied()
    }

    pub(in crate::authority) fn accepted_order(&self) -> Vec<AcceptedOrderKey> {
        let count = self
            .shards
            .iter()
            .map(|shard| shard.accepted_order.len())
            .sum();
        let mut order = Vec::with_capacity(count);
        for shard in &self.shards {
            order.extend(shard.accepted_order.iter().cloned());
        }
        order.sort_unstable();
        order
    }

    /// Borrow every canonical accepted-order key without materializing or
    /// globally sorting a second collection. Consumers whose public contract
    /// does not observe the global fee-priority order can validate every
    /// derived key while keeping their lock-held work linear.
    pub(in crate::authority) fn accepted_orders(
        &self,
    ) -> impl Iterator<Item = &AcceptedOrderKey> + '_ {
        self.shards
            .iter()
            .flat_map(|shard| shard.accepted_order.iter())
    }

    pub(in crate::authority) fn contains_accepted_order(&self, key: &AcceptedOrderKey) -> bool {
        self.shards
            .get(self.router.owner(key.hash()))
            .is_some_and(|shard| shard.accepted_order.contains(key))
    }

    pub(in crate::authority) fn contains_eviction_order(&self, key: &EvictionOrderKey) -> bool {
        self.shards
            .get(self.router.owner(&key.hash))
            .is_some_and(|shard| shard.eviction_order.contains(key))
    }

    pub(in crate::authority) fn iter(&self) -> ShardedOwnerIter<'_> {
        ShardedOwnerIter {
            shards: self.shards.iter(),
            current: None,
        }
    }

    pub(in crate::authority) fn values(&self) -> impl ExactSizeIterator<Item = &OwnedTx> + '_ {
        self.iter().map(|(_, owner)| owner)
    }
}

impl<'map> IntoIterator for &'map ShardedOwnerReadCut<'_> {
    type Item = (&'map RawTxHash, &'map OwnedTx);
    type IntoIter = ShardedOwnerIter<'map>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

pub(in crate::authority) struct ShardedOwnerIter<'map> {
    shards: std::slice::Iter<'map, RwLockReadGuard<'map, AuthorityShard>>,
    current: Option<hash_map::Iter<'map, RawTxHash, OwnedTx>>,
}

impl<'map> Iterator for ShardedOwnerIter<'map> {
    type Item = (&'map RawTxHash, &'map OwnedTx);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(item) = self.current.as_mut().and_then(Iterator::next) {
                return Some(item);
            }
            self.current = Some(self.shards.next()?.owners.iter());
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.len()))
    }
}

impl ExactSizeIterator for ShardedOwnerIter<'_> {
    fn len(&self) -> usize {
        let current = self.current.as_ref().map_or(0, ExactSizeIterator::len);
        self.shards
            .clone()
            .map(|shard| shard.owners.len())
            .fold(current, usize::saturating_add)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum ShardProposedCountPlanError {
    Projection,
    Arithmetic,
}

#[derive(Debug, Default)]
pub(in crate::authority) struct ShardProposedCountPlan(Vec<(u8, usize, usize)>);

#[cfg(test)]
mod owner_removal_revision_tests {
    use super::OwnerShardRemovalRevision;

    #[test]
    fn vacancy_revision_exhaustion_never_wraps_into_reusable_evidence() {
        let mut revision = OwnerShardRemovalRevision::Active(u64::MAX - 1);
        revision.advance();
        assert_eq!(revision, OwnerShardRemovalRevision::Active(u64::MAX));
        assert_eq!(revision.vacancy_witness(), Some(revision));

        revision.advance();
        assert_eq!(revision, OwnerShardRemovalRevision::Exhausted);
        assert_eq!(revision.vacancy_witness(), None);

        revision.advance();
        assert_eq!(revision, OwnerShardRemovalRevision::Exhausted);
    }
}
