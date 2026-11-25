use super::{
    budget::{Amount, compute_owner_amount, finish_accepted_amount},
    state::{FastHashMap, FastHashSet},
};
use crate::component::entry::TxEntry;
use crate::error::Reject;
use ckb_network::PeerIndex;
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{Capacity, Cycle, TransactionView, cell::ResolvedTransaction},
    packed::{Byte32, OutPoint, ProposalShortId},
    prelude::{Entity, Unpack},
};
use ckb_verification::cache::ScriptVerificationProof;
use std::{
    collections::BTreeSet,
    fmt,
    sync::{Arc, Weak},
    time::{Duration, Instant},
};

/// Resource failures that select distinct retry policies have named reasons.
/// Other limits carry diagnostic text; wording never selects control flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FullReason {
    Accepted,
    Pipeline,
    History,
    NoticeOutbox,
    Active,
    Other(&'static str),
}
impl From<&'static str> for FullReason {
    fn from(reason: &'static str) -> Self {
        Self::Other(reason)
    }
}
impl fmt::Display for FullReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Accepted => "accepted pool",
            Self::Pipeline => "pipeline",
            Self::History => "replacement history",
            Self::NoticeOutbox => "notice outbox",
            Self::Active => "active work",
            Self::Other(reason) => reason,
        })
    }
}

/// Expected operation outcomes share one route through the store/service.
/// A structural fault is distinct from input, stale work and resource pressure.
#[derive(Debug, Clone)]
pub(crate) enum Error {
    Stale,
    Full(FullReason),
    Rejected(Reject),
    Closed,
    Fault(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stale => f.write_str("tx-pool observation changed"),
            Self::Full(reason) => write!(f, "tx-pool resource unavailable: {reason}"),
            Self::Rejected(reason) => fmt::Display::fmt(reason, f),
            Self::Closed => f.write_str("tx-pool is closed"),
            Self::Fault(reason) => write!(f, "tx-pool invariant failed: {reason}"),
        }
    }
}
impl std::error::Error for Error {}
impl From<Reject> for Error {
    fn from(value: Reject) -> Self {
        Self::Rejected(value)
    }
}

/// Network provenance remains the same fact when a remote transaction becomes
/// a proposal and, if its proposal window expires, remote work again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RemoteOrigin {
    pub(super) peer: PeerIndex,
    pub(super) deadline: Instant,
}

/// Origin, retention and computation have different policies. Proposal work
/// retains its remote peer/deadline, but borrows trusted compute headroom and
/// drops the remote cycle declaration; the methods below own those decisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Source {
    Remote {
        origin: RemoteOrigin,
        cycles: Option<Cycle>,
    },
    Proposal {
        remote: Option<RemoteOrigin>,
    },
    Recovery,
    Local,
}

impl Source {
    /// Proposal priority drops the remote cycle declaration while retaining
    /// the peer and original deadline for a later return to remote policy.
    pub(super) fn promote_remote(self) -> Self {
        match self {
            Self::Remote { origin, .. } => Self::Proposal {
                remote: Some(origin),
            },
            source => source,
        }
    }

    /// An originless proposal has no network work to restore after expiry.
    pub(super) fn after_proposal_window(self) -> Option<Self> {
        match self {
            Self::Proposal {
                remote: Some(origin),
            } => Some(Self::Remote {
                origin,
                cycles: None,
            }),
            Self::Proposal { remote: None } => None,
            source => Some(source),
        }
    }

    pub(super) fn residency_peer(self) -> Option<PeerIndex> {
        match self {
            Self::Remote { origin, .. } => Some(origin.peer),
            Self::Proposal { remote } => remote.map(|origin| origin.peer),
            Self::Recovery | Self::Local => None,
        }
    }

    pub(super) fn compute_peer(self) -> Option<PeerIndex> {
        match self {
            Self::Remote { origin, .. } => Some(origin.peer),
            _ => None,
        }
    }

    pub(super) fn deadline(self) -> Option<Instant> {
        match self {
            Self::Remote { origin, .. } => Some(origin.deadline),
            Self::Proposal { remote } => remote.map(|origin| origin.deadline),
            Self::Recovery | Self::Local => None,
        }
    }

    pub(super) fn declared_cycles(self) -> Option<Cycle> {
        match self {
            Self::Remote { cycles, .. } => cycles,
            _ => None,
        }
    }

    /// Locally recovered work needs a retained producer. Network work can arrive
    /// before its producer, independently of proposal or declared-cycle policy.
    pub(super) fn requires_known_producer(self) -> bool {
        matches!(self, Self::Recovery | Self::Local)
    }

    pub(super) fn priority(self) -> u8 {
        match self {
            Self::Recovery | Self::Local => 0,
            Self::Proposal { .. } => 1,
            Self::Remote { .. } => 2,
        }
    }

    #[cfg(test)]
    pub(super) fn verification_time_limit(
        self,
        config: &ckb_app_config::TxPoolConfig,
    ) -> Option<Duration> {
        self.verification_time_limit_for_groups(config, 1)
    }

    pub(super) fn verification_time_limit_for_groups(
        self,
        config: &ckb_app_config::TxPoolConfig,
        script_groups: usize,
    ) -> Option<Duration> {
        let cap = config.max_tx_verify_time();
        Some(match self {
            Self::Local | Self::Recovery => return None,
            Self::Remote {
                cycles: Some(cycles),
                ..
            } => crate::verification::calibration::for_workload(cycles, script_groups, cap),
            Self::Remote { cycles: None, .. } | Self::Proposal { .. } => cap,
        })
    }
}

pub(super) use ckb_proposal_table::ProposalStatus as Status;

pub(super) fn status(snapshot: &Snapshot, proposal: &ProposalShortId) -> Status {
    snapshot.proposals().status(proposal)
}

pub(super) fn hash_key(hash: &Byte32) -> [u8; 32] {
    hash.unpack()
}

pub(super) fn proposal_key(hash: &[u8; 32]) -> [u8; ProposalShortId::TOTAL_SIZE] {
    let mut id = [0u8; ProposalShortId::TOTAL_SIZE];
    id.copy_from_slice(&hash[..ProposalShortId::TOTAL_SIZE]);
    id
}

/// Compact stack-owned 36-byte outpoint key (`tx_hash[0..32] || index_le[32..36]`).
///
/// Eliminates heap `Bytes` allocations (`compact_packed`), prevents accidental
/// retention of enclosing transaction or block buffers by construction, and
/// hashes in 4 instructions using the Blake2b-256 prefix mixed with bijective
/// Fibonacci hashing of the output index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct OutPointKey([u8; OutPoint::TOTAL_SIZE]);

impl OutPointKey {
    #[inline]
    pub(super) fn from_out_point(point: &OutPoint) -> Self {
        let bytes = point
            .as_slice()
            .first_chunk::<{ OutPoint::TOTAL_SIZE }>()
            .copied()
            .unwrap_or([0u8; OutPoint::TOTAL_SIZE]);
        Self(bytes)
    }

    #[inline]
    pub(super) fn from_parts(tx_hash: [u8; 32], index: u32) -> Self {
        let mut bytes = [0u8; OutPoint::TOTAL_SIZE];
        let (head, tail) = bytes.split_at_mut(32);
        head.copy_from_slice(&tx_hash);
        tail.copy_from_slice(&index.to_le_bytes());
        Self(bytes)
    }

    #[inline]
    pub(super) fn tx_hash(&self) -> &[u8; 32] {
        self.0.first_chunk::<32>().unwrap_or(&[0u8; 32])
    }

    #[inline]
    pub(super) fn index(&self) -> u32 {
        let bytes = self.0.last_chunk::<4>().copied().unwrap_or([0u8; 4]);
        u32::from_le_bytes(bytes)
    }

    pub(super) fn to_out_point(self) -> OutPoint {
        // `self.0` was captured directly from a validated `OutPoint::as_slice()`.
        OutPoint::new_unchecked(ckb_types::bytes::Bytes::copy_from_slice(&self.0))
    }
}

impl std::hash::Hash for OutPointKey {
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        if let (Some(head), Some(tail)) = (self.0.first_chunk::<16>(), self.0.last_chunk::<4>())
            && let (Some(lo), Some(hi)) = (head.first_chunk::<8>(), head.last_chunk::<8>())
        {
            let idx = u32::from_le_bytes(*tail);
            let mixed = u64::from_le_bytes(*lo)
                ^ u64::from(idx).wrapping_mul(0x9e37_79b9_7f4a_7c15)
                ^ u64::from_le_bytes(*hi).rotate_left(idx & 63);
            state.write_u64(mixed);
        } else {
            state.write(&self.0);
        }
    }
}

pub(super) fn matches_weak(expected: &Option<Weak<Entry>>, current: Option<&Arc<Entry>>) -> bool {
    expected.as_ref().map(Weak::as_ptr) == current.map(Arc::as_ptr)
}

/// Minimal off-lock read premises captured during resolution and checked under
/// the single-writer AuthorityState lock before committing an outcome.
///
/// Only producer/owner identity (including absence) and resolution-time spenders
/// need to cross the off-lock boundary; membership, RBF, late parents and
/// eviction read the live AuthorityState directly under one write guard.
#[derive(Clone, Debug, Default)]
pub(in crate::authority) struct Premises {
    pub(super) owners: FastHashMap<[u8; 32], Option<Weak<Entry>>>,
    pub(super) spenders: FastHashMap<OutPointKey, Option<[u8; 32]>>,
    has_spent: bool,
}

impl Premises {
    pub(super) fn reserve(&mut self, points: usize) {
        self.owners.reserve(points);
        self.spenders.reserve(points);
    }

    #[inline]
    pub(super) fn has_spent(&self) -> bool {
        self.has_spent
    }

    pub(super) fn observe_owner_key(
        &mut self,
        key: [u8; 32],
        entry: Option<&Arc<Entry>>,
    ) -> Result<(), Error> {
        match self.owners.get(&key) {
            Some(existing) if !matches_weak(existing, entry) => Err(Error::Stale),
            Some(_) => Ok(()),
            None => {
                self.owners.insert(key, entry.map(Arc::downgrade));
                Ok(())
            }
        }
    }

    pub(in crate::authority) fn observe_owner(
        &mut self,
        hash: &Byte32,
        entry: Option<&Arc<Entry>>,
    ) -> Result<(), Error> {
        self.observe_owner_key(hash_key(hash), entry)
    }

    pub(in crate::authority) fn check_owner(
        &self,
        hash: &Byte32,
        entry: Option<&Arc<Entry>>,
    ) -> Result<(), Error> {
        let key = hash_key(hash);
        if self
            .owners
            .get(&key)
            .is_some_and(|expected| !matches_weak(expected, entry))
        {
            return Err(Error::Stale);
        }
        Ok(())
    }

    pub(super) fn observe_spender_key(
        &mut self,
        key: OutPointKey,
        spender: Option<[u8; 32]>,
    ) -> Result<(), Error> {
        match self.spenders.get(&key) {
            Some(existing) if *existing != spender => Err(Error::Stale),
            Some(_) => Ok(()),
            None => {
                if spender.is_some() {
                    self.has_spent = true;
                }
                self.spenders.insert(key, spender);
                Ok(())
            }
        }
    }
}

/// Resolution retains compact cell content and weak producer observations.
/// The snapshot is held only by bounded active work, never by queued entries.
#[derive(Clone, Debug)]
pub(super) struct Resolved {
    pub(super) transaction: Arc<ResolvedTransaction>,
    pub(super) fee: Capacity,
    pub(super) view: u64,
    pub(super) premises: Premises,
    pub(super) pool_cells: BTreeSet<OutPoint>,
}

/// Retain the canonical script proof for the lifetime of an accepted owner.
/// A bounded shared cache may evict it before the transaction reaches a block.
#[derive(Clone, Copy, Debug)]
pub(super) enum VerifiedScripts {
    Vm(ScriptVerificationProof),
    #[cfg(any(test, feature = "internal", feature = "packing-bench"))]
    Fixture(Cycle),
}

impl VerifiedScripts {
    pub(super) fn cycles(self) -> Cycle {
        match self {
            Self::Vm(proof) => proof.cycles(),
            #[cfg(any(test, feature = "internal", feature = "packing-bench"))]
            Self::Fixture(cycles) => cycles,
        }
    }

    pub(super) fn proof(self) -> Option<ScriptVerificationProof> {
        match self {
            Self::Vm(proof) => Some(proof),
            #[cfg(any(test, feature = "internal", feature = "packing-bench"))]
            Self::Fixture(_) => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct Accepted {
    pub(super) transaction: Arc<ResolvedTransaction>,
    pub(super) scripts: VerifiedScripts,
    pub(super) fee: Capacity,
    pub(super) size: usize,
    pub(super) timestamp: u64,
    pub(super) parents: BTreeSet<Byte32>,
    pub(super) context_sensitive: bool,
    #[cfg(any(test, feature = "internal"))]
    pub(super) forced_status: Option<Status>,
}

impl Accepted {
    pub(super) fn projection(&self) -> TxEntry {
        TxEntry::new_with_timestamp(
            Arc::clone(&self.transaction),
            self.scripts.cycles(),
            self.fee,
            self.size,
            self.timestamp,
        )
    }

    pub(super) fn dependencies(&self) -> impl Iterator<Item = &OutPoint> + '_ {
        self.transaction.related_dep_out_points()
    }

    pub(super) fn status(&self, snapshot: &Snapshot) -> Status {
        self.status_with(|| {
            snapshot
                .proposals()
                .status_reader(self.transaction.transaction.proposal_short_id_reader())
        })
    }

    /// A captured population can share one proposal lookup. Synthetic fixtures
    /// retain the same explicit override at every projection boundary.
    pub(super) fn status_with(&self, canonical: impl FnOnce() -> Status) -> Status {
        #[cfg(any(test, feature = "internal"))]
        if let Some(status) = self.forced_status {
            return status;
        }
        canonical()
    }
}

/// Why a replaced owner can recover, including which availability changes matter.
#[derive(Clone, Debug)]
pub(super) enum RecoveryTriggers {
    /// Every recorded dependency must be available again after replacement.
    BlockedDependencies(BTreeSet<OutPoint>),
    /// Policy-only eviction retries when any original input becomes available.
    RetryInputs(BTreeSet<OutPoint>),
}

impl RecoveryTriggers {
    pub(super) fn keys(&self) -> &BTreeSet<OutPoint> {
        match self {
            Self::BlockedDependencies(keys) | Self::RetryInputs(keys) => keys,
        }
    }

    pub(super) fn is_ready(
        &self,
        mut available: impl FnMut(&OutPoint) -> Result<bool, Error>,
    ) -> Result<bool, Error> {
        let (keys, require_all) = match self {
            Self::BlockedDependencies(keys) => (keys, true),
            Self::RetryInputs(keys) => (keys, false),
        };
        for key in keys {
            if available(key)? != require_all {
                return Ok(!require_all);
            }
        }
        Ok(require_all)
    }

    /// Creating policy-only history is not a retry reason. A blocked owner
    /// can use that event only if the dependency has no final spender.
    pub(super) fn defer_creation_event(&self, spent: bool) -> bool {
        match self {
            Self::BlockedDependencies(_) => spent,
            Self::RetryInputs(_) => true,
        }
    }
}

/// Projection rules and their independent oracle are indexed under
/// "Changing a rule safely" in docs/MAINTENANCE.md.
#[derive(Clone, Debug)]
pub(super) enum Phase {
    Resolve,
    Verify(Arc<Resolved>),
    Waiting(BTreeSet<OutPoint>),
    Accepted(Accepted),
    Replaced(RecoveryTriggers),
}

/// Immutable owner fact; every phase or metadata change constructs a fresh Arc.
/// Its exact resource charge is computed once at construction.
#[derive(Clone, Debug)]
pub(super) struct Entry {
    pub(super) transaction: Arc<TransactionView>,
    pub(super) arrival: u64,
    pub(super) source: Source,
    pub(super) phase: Phase,
    charge: Amount,
}

impl Entry {
    pub(super) fn try_new(
        transaction: Arc<TransactionView>,
        arrival: u64,
        source: Source,
        phase: Phase,
    ) -> Result<Arc<Self>, Error> {
        let charge = compute_owner_amount(&transaction, &phase)?;
        Ok(Arc::new(Self {
            transaction,
            arrival,
            source,
            phase,
            charge,
        }))
    }

    pub(super) fn try_with_phase(&self, phase: Phase) -> Result<Arc<Self>, Error> {
        Self::try_new(
            Arc::clone(&self.transaction),
            self.arrival,
            self.source,
            phase,
        )
    }

    /// Construct an accepted successor using a base charge precomputed off-lock,
    /// avoiding repeated cell/byte scans inside `state.write()`.
    pub(super) fn try_with_accepted(
        &self,
        accepted: Accepted,
        base_charge: Amount,
    ) -> Result<Arc<Self>, Error> {
        let charge = finish_accepted_amount(base_charge, accepted.parents.len())?;
        Ok(Arc::new(Self {
            transaction: Arc::clone(&self.transaction),
            arrival: self.arrival,
            source: self.source,
            phase: Phase::Accepted(accepted),
            charge,
        }))
    }

    #[cfg(test)]
    pub(super) fn with_phase(&self, phase: Phase) -> Arc<Self> {
        self.try_with_phase(phase)
            .expect("test owner phase transition fits arithmetic")
    }

    #[cfg(test)]
    pub(super) fn with_source(&self, source: Source) -> Arc<Self> {
        Self::try_new(
            Arc::clone(&self.transaction),
            self.arrival,
            source,
            self.phase.clone(),
        )
        .expect("test owner source transition fits arithmetic")
    }

    #[cfg(test)]
    pub(super) fn with_source_and_phase(&self, source: Source, phase: Phase) -> Arc<Self> {
        Self::try_new(Arc::clone(&self.transaction), self.arrival, source, phase)
            .expect("test owner transition fits arithmetic")
    }

    pub(super) fn charge(&self) -> Amount {
        self.charge
    }

    pub(super) fn hash(&self) -> Byte32 {
        self.transaction.hash()
    }

    pub(super) fn hash_bytes(&self) -> [u8; 32] {
        hash_key(self.transaction.hash_ref())
    }

    pub(super) fn proposal(&self) -> ProposalShortId {
        self.transaction.proposal_short_id()
    }

    pub(super) fn accepted(&self) -> Option<&Accepted> {
        match &self.phase {
            Phase::Accepted(entry) => Some(entry),
            _ => None,
        }
    }

    pub(super) fn preaccepted(&self) -> bool {
        !matches!(self.phase, Phase::Accepted(_) | Phase::Replaced(_))
    }

    /// The retained peer cohort excludes accepted owners and replacement history.
    pub(super) fn preaccepted_peer(&self) -> Option<PeerIndex> {
        self.preaccepted()
            .then(|| self.source.residency_peer())
            .flatten()
    }

    pub(super) fn preaccepted_deadline(&self) -> Option<Instant> {
        self.preaccepted().then(|| self.source.deadline()).flatten()
    }

    /// Find the first cell dependency of a preaccepted owner that appears in `spent`.
    /// Declared inputs/cell-deps are checked first, followed by expanded Verify or Waiting keys.
    pub(super) fn find_spent_preaccepted(
        &self,
        spent: &FastHashSet<OutPointKey>,
    ) -> Option<OutPoint> {
        if let Some(point) = self
            .transaction
            .input_pts_iter()
            .chain(self.transaction.cell_deps_iter().map(|dep| dep.out_point()))
            .find(|point| spent.contains(&OutPointKey::from_out_point(point)))
        {
            return Some(point);
        }
        match &self.phase {
            Phase::Verify(resolved) => resolved
                .transaction
                .resolved_inputs
                .iter()
                .chain(&resolved.transaction.resolved_cell_deps)
                .chain(&resolved.transaction.resolved_dep_groups)
                .map(|cell| &cell.out_point)
                .find(|point| spent.contains(&OutPointKey::from_out_point(point)))
                .cloned(),
            Phase::Waiting(keys) => keys
                .iter()
                .find(|point| spent.contains(&OutPointKey::from_out_point(point)))
                .cloned(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::state::HashKeyBuildHasher;
    use std::hash::BuildHasher;

    #[test]
    fn out_point_key_round_trips_and_spreads_same_tx_indices() {
        let tx_hash = Byte32::new([0x5a; 32]);
        let mut hashes = std::collections::HashSet::new();
        for idx in 0..1024u32 {
            let point = OutPoint::new(tx_hash.clone(), idx);
            let key = OutPointKey::from_out_point(&point);
            assert_eq!(key.tx_hash(), &[0x5a; 32]);
            assert_eq!(key.index(), idx);
            assert_eq!(key.to_out_point(), point);
            assert!(hashes.insert(HashKeyBuildHasher.hash_one(key)));
        }
    }
}
