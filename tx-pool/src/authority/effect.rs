use super::ban::PeerBanLease;
use super::{
    rejection::{CommittedPublicReject, MembershipReject},
    state::{AcceptedStatus, ApplySequence, OwnedTx, PreAcceptedSource, RawTxHash},
};
use crate::error::Reject;
use ckb_network::PeerIndex;
use ckb_types::{
    core::{Capacity, FeeRate, TransactionBuilder, TransactionView},
    packed::OutPoint,
    prelude::Entity,
};
use ckb_util::parking_lot::Mutex;
use std::{
    collections::{HashMap, VecDeque},
    num::NonZeroUsize,
    sync::{
        Arc, LazyLock,
        atomic::{AtomicU8, Ordering},
    },
};

const EFFECT_ENVELOPE_BYTES: usize = 128;
/// Conservative residency charge for one raw-hash lookup entry into an
/// immutable committed effect batch. The projection duplicates neither the
/// transaction nor its rejection payload.
const PENDING_REJECT_INDEX_BYTES: usize = 128;
/// Conservative retained-memory charge for one detached packed hash and its
/// `Arc<Vec<RawTxHash>>` allocation share. This matches the existing relayer
/// projection bound without making the authority depend on the service layer.
const PARENT_TRANSACTION_HASH_BYTES: usize = 64;
/// Scalar and view residency beyond the packed transaction bytes retained by
/// one callback-compatible accepted-entry snapshot.
const COMMITTED_ENTRY_SNAPSHOT_OVERHEAD_BYTES: usize =
    std::mem::size_of::<CommittedEntrySnapshot>() + 64;
/// More simultaneously committing independent transitions cannot add owner
/// parallelism than there are physical owner shards. Keeping staging bounded
/// by the same architectural constant avoids a second tunable queue.
const MAX_STAGED_EFFECTS: usize = super::shard::AUTHORITY_SHARD_COUNT;

fn minimum_serialized_transaction_bytes() -> usize {
    static MINIMUM: LazyLock<usize> = LazyLock::new(|| {
        TransactionBuilder::default()
            .build()
            .data()
            .serialized_size_in_block()
            .max(1)
    });
    *MINIMUM
}

/// Checked upper bound for one UAK effect batch retaining `max_effects`
/// transaction outcomes whose packed transactions total `transaction_bytes`.
///
/// This formula deliberately uses the UAK committed snapshot and rejection
/// envelope, not an independently reconstructed `TxEntrySnapshot`. Keeping the
/// bound beside the values it charges prevents a later endpoint or snapshot
/// change from silently invalidating startup capacity validation.
fn effect_batch_charge_bound(
    transaction_bytes: usize,
    max_effects: usize,
) -> Result<usize, EffectConfigError> {
    let per_effect_metadata = EFFECT_ENVELOPE_BYTES
        .checked_add(COMMITTED_ENTRY_SNAPSHOT_OVERHEAD_BYTES)
        .and_then(|bytes| bytes.checked_add(PENDING_REJECT_INDEX_BYTES))
        .and_then(|bytes| bytes.checked_add(crate::constants::MAX_TX_POOL_REJECT_DESCRIPTION_BYTES))
        .ok_or(EffectConfigError::Arithmetic)?;
    max_effects
        .checked_mul(per_effect_metadata)
        .and_then(|metadata| transaction_bytes.checked_add(metadata))
        .ok_or(EffectConfigError::Arithmetic)
}

pub(super) fn parent_request_charge_bound(parent_count: usize) -> Option<usize> {
    parent_count
        .checked_mul(PARENT_TRANSACTION_HASH_BYTES)?
        .checked_add(EFFECT_ENVELOPE_BYTES)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct EffectCapacity {
    pub(super) batches: usize,
    pub(super) bytes: usize,
}

impl EffectCapacity {
    pub(super) const fn new(batches: usize, bytes: usize) -> Self {
        Self { batches, bytes }
    }

    fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            batches: self.batches.checked_add(other.batches)?,
            bytes: self.bytes.checked_add(other.bytes)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct EffectBatchBound {
    max_effects: usize,
    max_bytes: usize,
}

impl EffectBatchBound {
    pub(super) const fn new(max_effects: usize, max_bytes: usize) -> Self {
        Self {
            max_effects,
            max_bytes,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct EffectBatchBounds {
    remote: EffectBatchBound,
    trusted: EffectBatchBound,
    critical: EffectBatchBound,
}

impl EffectBatchBounds {
    pub(super) const fn new(
        remote: EffectBatchBound,
        trusted: EffectBatchBound,
        critical: EffectBatchBound,
    ) -> Self {
        Self {
            remote,
            trusted,
            critical,
        }
    }

    fn for_class(self, class: EffectClass) -> EffectBatchBound {
        match class {
            EffectClass::Remote => self.remote,
            EffectClass::Trusted => self.trusted,
            EffectClass::Critical => self.critical,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EffectConfigError {
    EmptyRemoteRegion,
    EmptyBatchBound,
    Arithmetic,
    IndivisibleBatch,
    Allocation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct EffectLimits {
    regions: EffectRegions,
    bounds: EffectBatchBounds,
}

impl EffectLimits {
    pub(super) fn production(
        max_pool_bytes: usize,
        retained_preaccepted_bytes: usize,
        max_block_bytes: usize,
        max_parent_count: usize,
    ) -> Result<Self, EffectConfigError> {
        let max_admission_effects = crate::constants::MAX_POOL_MUTATION_CANDIDATES
            .checked_add(1)
            .ok_or(EffectConfigError::Arithmetic)?;
        let admission_transaction_bytes = max_pool_bytes
            .checked_add(max_block_bytes)
            .ok_or(EffectConfigError::Arithmetic)?
            .min(
                max_admission_effects
                    .checked_mul(max_block_bytes)
                    .ok_or(EffectConfigError::Arithmetic)?,
            );
        let admission_effect_bytes =
            effect_batch_charge_bound(admission_transaction_bytes, max_admission_effects)?
                .max(4_096);

        let max_pool_effects = max_pool_bytes.div_ceil(minimum_serialized_transaction_bytes());
        let critical_effect_bytes =
            effect_batch_charge_bound(max_pool_bytes, max_pool_effects)?.max(4_096);
        let parent_request_effect_bytes =
            parent_request_charge_bound(max_parent_count).ok_or(EffectConfigError::Arithmetic)?;
        let resident_effect_bytes = max_pool_bytes
            .checked_add(retained_preaccepted_bytes)
            .and_then(|bytes| bytes.checked_mul(2))
            .ok_or(EffectConfigError::Arithmetic)?;
        let ordinary_effect_bytes = resident_effect_bytes
            .max(admission_effect_bytes)
            .max(parent_request_effect_bytes);

        Self::partitioned(
            EffectCapacity::new(
                crate::constants::EFFECT_JOURNAL_REMOTE_MAX_BATCHES,
                ordinary_effect_bytes,
            ),
            EffectCapacity::new(
                crate::constants::EFFECT_TRUSTED_HEADROOM_BATCHES,
                admission_effect_bytes,
            ),
            EffectCapacity::new(1, critical_effect_bytes),
            EffectBatchBounds::new(
                EffectBatchBound::new(max_admission_effects, ordinary_effect_bytes),
                EffectBatchBound::new(max_admission_effects, admission_effect_bytes),
                EffectBatchBound::new(max_pool_effects, critical_effect_bytes),
            ),
        )
    }

    pub(super) fn partitioned(
        remote: EffectCapacity,
        trusted_headroom: EffectCapacity,
        critical_headroom: EffectCapacity,
        bounds: EffectBatchBounds,
    ) -> Result<Self, EffectConfigError> {
        if remote.batches == 0 || remote.bytes == 0 {
            return Err(EffectConfigError::EmptyRemoteRegion);
        }
        if [bounds.remote, bounds.trusted, bounds.critical]
            .iter()
            .any(|bound| bound.max_effects == 0 || bound.max_bytes == 0)
        {
            return Err(EffectConfigError::EmptyBatchBound);
        }
        if EFFECT_ENVELOPE_BYTES > bounds.remote.max_bytes
            || EFFECT_ENVELOPE_BYTES > bounds.trusted.max_bytes
            || EFFECT_ENVELOPE_BYTES > bounds.critical.max_bytes
        {
            return Err(EffectConfigError::IndivisibleBatch);
        }
        let ordinary = remote
            .checked_add(trusted_headroom)
            .ok_or(EffectConfigError::Arithmetic)?;
        let total = ordinary
            .checked_add(critical_headroom)
            .ok_or(EffectConfigError::Arithmetic)?;
        if bounds.remote.max_bytes > remote.bytes
            || bounds.trusted.max_bytes > ordinary.bytes
            || bounds.critical.max_bytes > total.bytes
        {
            return Err(EffectConfigError::IndivisibleBatch);
        }
        Ok(Self {
            regions: EffectRegions::new(remote, ordinary, total),
            bounds,
        })
    }

    fn batch_bound(self, class: EffectClass) -> EffectBatchBound {
        self.bounds.for_class(class)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EffectClass {
    Remote,
    Trusted,
    Critical,
}

/// Capacity trust and overflow semantics are one closed policy. In
/// particular, non-rebuildable critical detail cannot accidentally inherit
/// the generation-reset fallback merely because it uses critical headroom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EffectPolicy {
    Remote,
    Trusted,
    CriticalDetail,
    CriticalRebuildable,
}

impl EffectPolicy {
    /// Select publication capacity from the authority that currently drives
    /// the owner, not from immutable ingress attribution. A transaction
    /// promoted from Remote to Proposal still reports its result to the
    /// original peer, but proposal-driven progress must remain available when
    /// the peer-controlled journal region is saturated.
    pub(super) const fn for_preaccepted_source(source: PreAcceptedSource) -> Self {
        match source {
            PreAcceptedSource::Remote(_) => Self::Remote,
            PreAcceptedSource::Proposal { .. } | PreAcceptedSource::Recovery(_) => Self::Trusted,
        }
    }

    const fn class(self) -> EffectClass {
        match self {
            Self::Remote => EffectClass::Remote,
            Self::Trusted => EffectClass::Trusted,
            Self::CriticalDetail | Self::CriticalRebuildable => EffectClass::Critical,
        }
    }

    const fn can_reset(self) -> bool {
        match self {
            Self::Remote | Self::Trusted | Self::CriticalDetail => false,
            Self::CriticalRebuildable => true,
        }
    }
}

/// Exact accepted-entry facts consumed by the existing callback surface.
///
/// These values are compiled from the same virtual membership projection as
/// the owner transition. An endpoint must never reread authority state after
/// Apply to reconstruct them: a later admission or reorg could otherwise pair
/// this committed outcome with a different ancestor/descendant generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CommittedEntrySnapshot {
    pub(super) tx: Arc<TransactionView>,
    pub(super) cycles: u64,
    pub(super) size: usize,
    pub(super) fee: Capacity,
    pub(super) ancestors_size: usize,
    pub(super) ancestors_fee: Capacity,
    pub(super) ancestors_cycles: u64,
    pub(super) ancestors_count: usize,
    pub(super) descendants_fee: Capacity,
    pub(super) descendants_size: usize,
    pub(super) descendants_cycles: u64,
    pub(super) descendants_count: usize,
    pub(super) timestamp: u64,
}

impl CommittedEntrySnapshot {
    fn charge_bytes(&self) -> Option<usize> {
        EFFECT_ENVELOPE_BYTES
            .checked_add(self.tx.data().total_size())?
            .checked_add(COMMITTED_ENTRY_SNAPSHOT_OVERHEAD_BYTES)
    }
}

/// Closed successful public outcomes. Payload shape makes callback and relay
/// differences structural rather than an adapter convention.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CommittedAcceptance {
    /// A candidate acquired Accepted ownership. The snapshot is the projected
    /// post-Apply membership view; ingress is the immutable relay attribution.
    Admission {
        entry: CommittedEntrySnapshot,
        status: AcceptedStatus,
        ingress_peer: Option<PeerIndex>,
    },
    /// A trusted synchronous caller observed an already Accepted raw hash.
    /// No membership callback is emitted because ownership did not change.
    Duplicate {
        tx_hash: RawTxHash,
        requesting_peer: Option<PeerIndex>,
    },
    /// Existing Accepted ownership changed proposal status because the chain
    /// view moved. It updates callback/template projections but does not emit
    /// a fresh network admission acknowledgement.
    ChainStatusChange {
        entry: CommittedEntrySnapshot,
        status: AcceptedStatus,
    },
}

/// Whether a chain conflict removed an externally invisible preaccepted
/// candidate or a callback-visible accepted member. The distinction controls
/// callback and relay policy and therefore cannot be inferred after Apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CommittedConflictOwner {
    PreAccepted {
        tx: Arc<TransactionView>,
        audience: RejectionAudience,
    },
    Accepted(CommittedEntrySnapshot),
}

impl CommittedConflictOwner {
    fn charge_bytes(&self) -> Option<usize> {
        match self {
            Self::PreAccepted { tx, .. } => {
                EFFECT_ENVELOPE_BYTES.checked_add(tx.data().total_size())
            }
            Self::Accepted(entry) => entry.charge_bytes(),
        }
    }
}

/// Closed rejected public outcomes. Each variant owns exactly the evidence
/// needed to reproduce the existing endpoint contract without a state read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CommittedRejection {
    /// Resolve/verify/policy work rejected a not-yet-Accepted owner.
    Validation {
        tx: Arc<TransactionView>,
        audience: RejectionAudience,
        reason: CommittedPublicReject,
    },
    /// Final membership policy rejected a not-yet-Accepted candidate.
    Membership {
        tx: Arc<TransactionView>,
        audience: RejectionAudience,
        reason: MembershipReject,
    },
    /// RBF displaced an Accepted member. The pre-Apply callback snapshot and
    /// winner identity remain committed even when recovery history is kept.
    Replaced {
        entry: CommittedEntrySnapshot,
        winner: RawTxHash,
    },
    /// Accepted-pool capacity removed an Accepted member.
    CapacityEvicted {
        entry: CommittedEntrySnapshot,
        fee_rate: FeeRate,
    },
    /// Accepted residency elapsed. Descendants removed with an expired root
    /// retain their own admission timestamp in the committed snapshot, exactly
    /// matching the historical callback and recent-reject contract.
    Expired { entry: CommittedEntrySnapshot },
    /// An attached chain transaction invalidated a resident owner.
    ChainConflict {
        owner: CommittedConflictOwner,
        out_point: OutPoint,
    },
}

impl CommittedRejection {
    fn raw_hash(&self) -> RawTxHash {
        let hash = match self {
            Self::Validation { tx, .. } | Self::Membership { tx, .. } => tx.hash(),
            Self::Replaced { entry, .. }
            | Self::CapacityEvicted { entry, .. }
            | Self::Expired { entry } => entry.tx.hash(),
            Self::ChainConflict { owner, .. } => match owner {
                CommittedConflictOwner::PreAccepted { tx, .. } => tx.hash(),
                CommittedConflictOwner::Accepted(entry) => entry.tx.hash(),
            },
        };
        RawTxHash(hash)
    }

    /// Compile the one public rejection represented by this committed cause.
    /// Both the publisher and the pending-RPC projection consume this method,
    /// so endpoint delivery cannot drift from the value visible before the
    /// recent-reject database write completes.
    pub(super) fn public_reject(&self) -> CommittedPublicReject {
        match self {
            Self::Validation { reason, .. } => reason.clone(),
            Self::Membership { reason, .. } => {
                CommittedPublicReject::new(reason.clone().into_public())
            }
            Self::Replaced { winner, .. } => CommittedPublicReject::new(Reject::RBFRejected(
                format!("replaced by tx {}", winner.0),
            )),
            Self::CapacityEvicted { fee_rate, .. } => CommittedPublicReject::new(Reject::Full(
                format!("the fee_rate for this transaction is: {fee_rate}"),
            )),
            Self::Expired { entry } => CommittedPublicReject::new(Reject::Expiry(entry.timestamp)),
            Self::ChainConflict { out_point, .. } => CommittedPublicReject::new(Reject::Resolve(
                ckb_types::core::error::OutPointError::Dead(out_point.clone()),
            )),
        }
    }

    /// Allocation-free mirror of the public recent-reject policy, used only
    /// while sealing a batch under the authority guard. An exhaustive test
    /// checks it against `public_reject().should_record()` for every cause.
    fn should_record_recent_reject(&self) -> bool {
        match self {
            Self::Validation { reason, .. } => reason.should_record(),
            Self::Membership { reason, .. } => reason.should_record_recent_reject(),
            Self::Replaced { .. } | Self::ChainConflict { .. } => true,
            Self::Expired { .. } => true,
            Self::CapacityEvicted { .. } => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct RejectionAudience {
    ingress_peer: Option<PeerIndex>,
}

/// Exact, bounded malformed-input evidence attached to an ingress-cohort
/// revocation. The constructor prevents an ordinary policy rejection from
/// acquiring peer-ban semantics merely because it followed the same worker
/// path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CommittedPeerBanRejection {
    tx_hash: RawTxHash,
    reason: CommittedPublicReject,
}

impl CommittedPeerBanRejection {
    pub(super) fn tx_hash(&self) -> &RawTxHash {
        &self.tx_hash
    }

    pub(super) fn reason(&self) -> &CommittedPublicReject {
        &self.reason
    }
}

/// A peer identity and its optional malformed culprit are sealed together, so
/// effect construction cannot accidentally ban one peer using another peer's
/// validation result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CommittedPeerCohortRevocation {
    lease: PeerBanLease,
    culprit: Option<CommittedPeerBanRejection>,
}

impl CommittedPeerCohortRevocation {
    pub(super) fn malformed(
        lease: PeerBanLease,
        tx_hash: RawTxHash,
        reason: CommittedPublicReject,
    ) -> Option<Self> {
        reason.is_malformed().then_some(Self {
            lease,
            culprit: Some(CommittedPeerBanRejection { tx_hash, reason }),
        })
    }

    #[cfg(test)]
    pub(super) const fn peer(&self) -> PeerIndex {
        self.lease.peer()
    }

    pub(super) const fn lease(&self) -> PeerBanLease {
        self.lease
    }

    pub(super) fn culprit(&self) -> Option<&CommittedPeerBanRejection> {
        self.culprit.as_ref()
    }
}

/// Non-empty, canonical request detail committed with a Remote missing wait.
/// The constructor is private so an empty request cannot occupy journal
/// capacity or pretend that an external liveness action was published.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ParentTransactionRequest {
    peer: PeerIndex,
    parents: Arc<Vec<RawTxHash>>,
}

/// Proof-carrying relay cleanup for a transaction that has an actual remote
/// ingress attribution. The private payload prevents transition code from
/// manufacturing this effect for trusted proposals or recovery owners.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CommittedRemoteIngressRelease {
    tx_hash: RawTxHash,
}

impl CommittedRemoteIngressRelease {
    /// A Remote boundary supplies the ingress attribution directly. The
    /// relayer's projection is keyed only by raw hash, so retaining the peer
    /// would add state without changing publication semantics.
    pub(super) const fn unretained_remote_submission(
        tx_hash: RawTxHash,
        _ingress_peer: PeerIndex,
    ) -> Self {
        Self { tx_hash }
    }

    /// Administrative removal may release relay state only when the removed
    /// owner itself proves a not-yet-Accepted Remote attribution.
    pub(super) fn removed_owner(tx_hash: RawTxHash, owner: &OwnedTx) -> Option<Self> {
        match owner {
            OwnedTx::PreAccepted(entry) => entry
                .source
                .ingress_peer()
                .map(|_ingress_peer| Self { tx_hash }),
            OwnedTx::Accepted(_) | OwnedTx::ReplacementHistory(_) => None,
        }
    }

    pub(super) const fn tx_hash(&self) -> &RawTxHash {
        &self.tx_hash
    }
}

impl ParentTransactionRequest {
    pub(super) fn new(peer: PeerIndex, parents: Arc<Vec<RawTxHash>>) -> Option<Self> {
        if parents.is_empty() {
            None
        } else {
            Some(Self { peer, parents })
        }
    }

    pub(super) const fn peer(&self) -> PeerIndex {
        self.peer
    }

    pub(super) fn parents(&self) -> &[RawTxHash] {
        &self.parents
    }
}

impl RejectionAudience {
    pub(super) const fn from_source(source: PreAcceptedSource) -> Self {
        Self {
            ingress_peer: source.ingress_peer(),
        }
    }

    pub(super) const fn from_ingress(ingress_peer: Option<PeerIndex>) -> Self {
        Self { ingress_peer }
    }

    pub(super) const fn has_ingress(self) -> bool {
        self.ingress_peer.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CommittedEffect {
    Accepted(CommittedAcceptance),
    Rejected(CommittedRejection),
    /// The transaction became canonical while it still had an in-flight
    /// Remote owner. This settles the relayer's verification projection as a
    /// successful known transaction without manufacturing a pool status,
    /// callback or rejection record.
    ChainCommitted {
        tx_hash: RawTxHash,
        ingress_peer: PeerIndex,
    },
    /// One bounded authority result for a complete not-yet-Accepted ingress
    /// cohort. The optional culprit retains only its exact hash and bounded
    /// malformed reason: publication can record the rejection and ban the
    /// peer without retaining every removed transaction or creating a raw-hash
    /// tombstone. Relay consumes this as a required generation reset.
    PeerCohortRevoked(CommittedPeerCohortRevocation),
    /// A remote residency lease elapsed before Accepted ownership. Expiry has
    /// the same refetch semantics as peer revocation, but remains remote
    /// capacity work and does not imply hostile-peer policy.
    RemoteExpired {
        tx_hash: RawTxHash,
    },
    /// A Remote submission was not retained (duplicate, revoked peer, or
    /// another terminal ingress disposition), or a remote-attributed
    /// not-yet-Accepted owner was removed. No Accepted fact is published; the
    /// relayer must only release its pending/known projection so another peer
    /// may supply the transaction later.
    RemoteIngressReleased(CommittedRemoteIngressRelease),
    /// A Remote owner entered `Waiting(Missing)`. The exact request and the
    /// durable wait share one authority Apply, so the relayer cannot observe a
    /// request for a stale lease or lose the only request for a committed wait.
    ParentTransactionsRequested(ParentTransactionRequest),
    GenerationReset,
}

impl CommittedEffect {
    fn recordable_rejection(&self) -> Option<CommittedRecentReject<'_>> {
        match self {
            Self::Rejected(rejection) if rejection.should_record_recent_reject() => {
                Some(CommittedRecentReject::Rejection(rejection))
            }
            Self::PeerCohortRevoked(revocation)
                if revocation
                    .culprit()
                    .is_some_and(|culprit| culprit.reason.should_record()) =>
            {
                revocation.culprit().map(CommittedRecentReject::PeerBan)
            }
            _ => None,
        }
    }

    fn charge_bytes(&self) -> Option<usize> {
        match self {
            Self::Accepted(acceptance) => match acceptance {
                CommittedAcceptance::Admission { entry, .. }
                | CommittedAcceptance::ChainStatusChange { entry, .. } => entry.charge_bytes(),
                CommittedAcceptance::Duplicate { .. } => Some(EFFECT_ENVELOPE_BYTES),
            },
            Self::Rejected(rejection) => {
                let retained = match rejection {
                    CommittedRejection::Validation { tx, reason, .. } => EFFECT_ENVELOPE_BYTES
                        .checked_add(tx.data().total_size())?
                        .checked_add(reason.description_bytes()),
                    CommittedRejection::Membership { tx, .. } => {
                        EFFECT_ENVELOPE_BYTES.checked_add(tx.data().total_size())
                    }
                    CommittedRejection::Replaced { entry, .. }
                    | CommittedRejection::CapacityEvicted { entry, .. }
                    | CommittedRejection::Expired { entry } => entry.charge_bytes(),
                    CommittedRejection::ChainConflict {
                        owner, out_point, ..
                    } => owner
                        .charge_bytes()?
                        .checked_add(out_point.as_slice().len()),
                }?;
                if rejection.should_record_recent_reject() {
                    retained.checked_add(PENDING_REJECT_INDEX_BYTES)
                } else {
                    Some(retained)
                }
            }
            Self::ChainCommitted { .. } => Some(EFFECT_ENVELOPE_BYTES),
            Self::PeerCohortRevoked(revocation) => {
                let retained =
                    revocation
                        .culprit()
                        .map_or(Some(EFFECT_ENVELOPE_BYTES), |culprit| {
                            EFFECT_ENVELOPE_BYTES
                                .checked_add(culprit.tx_hash.0.as_slice().len())?
                                .checked_add(culprit.reason.description_bytes())
                        })?;
                if revocation
                    .culprit()
                    .is_some_and(|culprit| culprit.reason.should_record())
                {
                    retained.checked_add(PENDING_REJECT_INDEX_BYTES)
                } else {
                    Some(retained)
                }
            }
            Self::RemoteExpired { .. } => Some(EFFECT_ENVELOPE_BYTES),
            Self::RemoteIngressReleased(_) => Some(EFFECT_ENVELOPE_BYTES),
            Self::ParentTransactionsRequested(request) => {
                parent_request_charge_bound(request.parents().len())
            }
            Self::GenerationReset => Some(0),
        }
    }
}

#[derive(Clone, Copy)]
enum CommittedRecentReject<'effect> {
    Rejection(&'effect CommittedRejection),
    PeerBan(&'effect CommittedPeerBanRejection),
}

impl CommittedRecentReject<'_> {
    fn raw_hash(self) -> RawTxHash {
        match self {
            Self::Rejection(rejection) => rejection.raw_hash(),
            Self::PeerBan(culprit) => culprit.tx_hash.clone(),
        }
    }

    fn public_reject(self) -> CommittedPublicReject {
        match self {
            Self::Rejection(rejection) => rejection.public_reject(),
            Self::PeerBan(culprit) => culprit.reason.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EffectBuildError {
    Empty,
    TooMany,
    TooLarge,
    Arithmetic,
    ReservedReset,
}

#[derive(Debug)]
pub(super) struct EffectBatch {
    effects: Vec<CommittedEffect>,
    publication_steps: usize,
    charge_bytes: usize,
    stage_state: AtomicU8,
}

impl PartialEq for EffectBatch {
    fn eq(&self, other: &Self) -> bool {
        self.effects == other.effects
            && self.publication_steps == other.publication_steps
            && self.charge_bytes == other.charge_bytes
    }
}

impl Eq for EffectBatch {}

impl EffectBatch {
    fn build(
        effects: Vec<CommittedEffect>,
        class: EffectClass,
        limits: EffectLimits,
    ) -> Result<Arc<Self>, EffectBuildError> {
        if effects.is_empty() {
            return Err(EffectBuildError::Empty);
        }
        let bound = limits.batch_bound(class);
        if effects.len() > bound.max_effects {
            return Err(EffectBuildError::TooMany);
        }
        if effects
            .iter()
            .any(|effect| matches!(effect, CommittedEffect::GenerationReset))
        {
            return Err(EffectBuildError::ReservedReset);
        }
        let publication_steps = effects
            .len()
            .checked_mul(EffectEndpoint::COUNT)
            .ok_or(EffectBuildError::Arithmetic)?;
        let charge_bytes = effects.iter().try_fold(0usize, |total, effect| {
            total.checked_add(effect.charge_bytes()?)
        });
        let charge_bytes = charge_bytes.ok_or(EffectBuildError::Arithmetic)?;
        if charge_bytes > bound.max_bytes {
            return Err(EffectBuildError::TooLarge);
        }
        Ok(Arc::new(Self {
            // The compiler already reserved this bounded carrier fallibly.
            // Moving it into the immutable batch avoids a second allocator
            // operation and keeps the scratch/resident proof on one value.
            effects,
            publication_steps,
            charge_bytes,
            stage_state: AtomicU8::new(EFFECT_STAGE_UNSTAGED),
        }))
    }

    fn reset() -> Result<Arc<Self>, EffectConfigError> {
        let mut effects = Vec::new();
        effects
            .try_reserve_exact(1)
            .map_err(|_| EffectConfigError::Allocation)?;
        effects.push(CommittedEffect::GenerationReset);
        Ok(Arc::new(Self {
            effects,
            publication_steps: EffectEndpoint::COUNT,
            charge_bytes: 0,
            stage_state: AtomicU8::new(EFFECT_STAGE_UNSTAGED),
        }))
    }

    pub(super) fn effects(&self) -> &[CommittedEffect] {
        &self.effects
    }

    pub(super) fn charge_bytes(&self) -> usize {
        self.charge_bytes
    }

    fn publication_steps(&self) -> usize {
        self.publication_steps
    }

    fn pending_recent_rejects(&self) -> impl Iterator<Item = (usize, CommittedRecentReject<'_>)> {
        self.effects
            .iter()
            .enumerate()
            .filter_map(|(index, effect)| {
                effect
                    .recordable_rejection()
                    .map(|rejection| (index, rejection))
            })
    }

    fn pending_recent_reject_count(&self) -> usize {
        self.pending_recent_rejects().count()
    }

    fn begin_stage(&self) -> Result<(), EffectError> {
        self.stage_state
            .compare_exchange(
                EFFECT_STAGE_UNSTAGED,
                EFFECT_STAGE_PENDING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map(|_| ())
            .map_err(|_| EffectError::Projection)
    }

    fn stage_state(&self) -> u8 {
        self.stage_state.load(Ordering::Acquire)
    }

    fn commit_stage(&self) {
        self.stage_state
            .store(EFFECT_STAGE_COMMITTED, Ordering::Release);
    }

    fn cancel_stage(&self) {
        self.stage_state
            .store(EFFECT_STAGE_CANCELLED, Ordering::Release);
    }
}

#[derive(Debug)]
pub(super) struct EffectPublication {
    policy: EffectPolicy,
    batch: Arc<EffectBatch>,
}

/// Scratch compiler for one canonical ordered transition family.
///
/// It incrementally proves that every complete item effect still fits the
/// same immutable journal batch. A full result is a prefix boundary, not a
/// partial publication. The compiler owns no journal state and its finished
/// publication must still pass [`EffectLog::plan_publication`] against the
/// same authority cut.
pub(super) struct OrderedEffectPublication {
    policy: EffectPolicy,
    effects: Vec<CommittedEffect>,
    charge_bytes: usize,
    limits: EffectLimits,
    usage: EffectRegionUsage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OrderedEffectAppendError {
    Full,
    Projection,
}

impl EffectPublication {
    fn new(
        policy: EffectPolicy,
        effects: Vec<CommittedEffect>,
        limits: EffectLimits,
    ) -> Result<Self, EffectBuildError> {
        Ok(Self {
            policy,
            batch: EffectBatch::build(effects, policy.class(), limits)?,
        })
    }
}

/// A non-empty prefix proven to fit the remote effect region's indivisible
/// batch shape. The selected count is carried with the publication so the
/// authority transition cannot remove more owners than the journal can
/// describe.
pub(super) struct RemoteEffectPrefix {
    publication: EffectPublication,
    selected: NonZeroUsize,
}

impl RemoteEffectPrefix {
    pub(super) fn into_parts(self) -> (EffectPublication, NonZeroUsize) {
        (self.publication, self.selected)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct EffectUsage {
    pub(super) batches: usize,
    pub(super) bytes: usize,
}

impl EffectUsage {
    fn checked_charge(self, bytes: usize) -> Option<Self> {
        Some(Self {
            batches: self.batches.checked_add(1)?,
            bytes: self.bytes.checked_add(bytes)?,
        })
    }

    fn checked_release(self, bytes: usize) -> Option<Self> {
        Some(Self {
            batches: self.batches.checked_sub(1)?,
            bytes: self.bytes.checked_sub(bytes)?,
        })
    }

    fn fits(self, bytes: usize, limit: EffectCapacity) -> bool {
        self.batches
            .checked_add(1)
            .is_some_and(|batches| batches <= limit.batches)
            && self
                .bytes
                .checked_add(bytes)
                .is_some_and(|total| total <= limit.bytes)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct EffectRegions {
    remote: EffectCapacity,
    ordinary: EffectCapacity,
    total: EffectCapacity,
}

impl EffectRegions {
    const fn new(remote: EffectCapacity, ordinary: EffectCapacity, total: EffectCapacity) -> Self {
        Self {
            remote,
            ordinary,
            total,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct EffectRegionUsage {
    remote: EffectUsage,
    ordinary: EffectUsage,
    total: EffectUsage,
}

impl EffectRegionUsage {
    fn fits(self, limits: EffectRegions, class: EffectClass, bytes: usize) -> bool {
        match class {
            EffectClass::Remote => {
                self.remote.fits(bytes, limits.remote)
                    && self.ordinary.fits(bytes, limits.ordinary)
                    && self.total.fits(bytes, limits.total)
            }
            EffectClass::Trusted => {
                self.ordinary.fits(bytes, limits.ordinary) && self.total.fits(bytes, limits.total)
            }
            EffectClass::Critical => self.total.fits(bytes, limits.total),
        }
    }

    fn checked_charge(self, class: EffectClass, bytes: usize) -> Option<Self> {
        match class {
            EffectClass::Remote => Some(Self {
                remote: self.remote.checked_charge(bytes)?,
                ordinary: self.ordinary.checked_charge(bytes)?,
                total: self.total.checked_charge(bytes)?,
            }),
            EffectClass::Trusted => Some(Self {
                remote: self.remote,
                ordinary: self.ordinary.checked_charge(bytes)?,
                total: self.total.checked_charge(bytes)?,
            }),
            EffectClass::Critical => Some(Self {
                remote: self.remote,
                ordinary: self.ordinary,
                total: self.total.checked_charge(bytes)?,
            }),
        }
    }

    fn checked_release(self, class: EffectClass, bytes: usize) -> Option<Self> {
        match class {
            EffectClass::Remote => Some(Self {
                remote: self.remote.checked_release(bytes)?,
                ordinary: self.ordinary.checked_release(bytes)?,
                total: self.total.checked_release(bytes)?,
            }),
            EffectClass::Trusted => Some(Self {
                remote: self.remote,
                ordinary: self.ordinary.checked_release(bytes)?,
                total: self.total.checked_release(bytes)?,
            }),
            EffectClass::Critical => Some(Self {
                remote: self.remote,
                ordinary: self.ordinary,
                total: self.total.checked_release(bytes)?,
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EffectRecord {
    sequence: ApplySequence,
    batch: Arc<EffectBatch>,
    processed: EffectProgress,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct QueuedEffectRecord {
    record: EffectRecord,
    class: EffectClass,
}

const EFFECT_STAGE_UNSTAGED: u8 = 0;
const EFFECT_STAGE_PENDING: u8 = 1;
const EFFECT_STAGE_COMMITTED: u8 = 2;
const EFFECT_STAGE_CANCELLED: u8 = 3;

#[derive(Debug)]
struct StagedEffectRecord {
    record: QueuedEffectRecord,
}

/// Move-only reservation for one non-publishable record in the sole effect
/// journal. Ordinary code either activates it after the owner cut has become
/// irreversible or rolls it back before any owner mutation. `Drop` is a
/// conservative safety net for abandoned pre-commit paths; the production
/// shared Apply uses explicit consumption so its wake transition remains
/// observable by the runtime.
#[must_use = "a staged effect must be activated after owner commit or rolled back before it"]
pub(super) struct StagedEffect {
    log: Arc<Mutex<EffectLog>>,
    batch: Arc<EffectBatch>,
    sequence: ApplySequence,
    class: EffectClass,
    charge_bytes: usize,
    finalized: bool,
}

impl StagedEffect {
    /// Publishability activation is allocation-free and has no ordinary
    /// failure arm. A later sequence may become ready first, but the log moves
    /// only the committed leading prefix into its existing FIFO.
    pub(super) fn activate_with_wake(mut self) -> EffectWakeTransition {
        let transition = {
            let mut log = self.log.lock();
            let before = log.wake_projection();
            log.activate_staged(self.sequence, &self.batch);
            EffectWakeTransition {
                before,
                after: log.wake_projection(),
            }
        };
        self.finalized = true;
        transition
    }

    #[cfg(test)]
    pub(super) fn activate(self) {
        let _wake = self.activate_with_wake();
    }

    /// Explicit pre-commit rollback returns the exact capacity charge before
    /// marking the slot cancelled. It is fallible only for an internal
    /// projection defect and therefore remains strictly before owner mutation.
    pub(super) fn rollback_with_wake(mut self) -> Result<EffectWakeTransition, EffectError> {
        let wake = {
            let mut log = self.log.lock();
            let before = log.wake_projection();
            log.release_staged_charge(self.class, self.charge_bytes)?;
            self.batch.cancel_stage();
            log.flush_staged_prefix();
            EffectWakeTransition {
                before,
                after: log.wake_projection(),
            }
        };
        self.finalized = true;
        Ok(wake)
    }

    #[cfg(test)]
    pub(super) fn rollback(self) -> Result<(), EffectError> {
        self.rollback_with_wake().map(|_| ())
    }
}

impl Drop for StagedEffect {
    fn drop(&mut self) {
        if self.finalized {
            return;
        }
        let mut log = self.log.lock();
        if log
            .release_staged_charge(self.class, self.charge_bytes)
            .is_ok()
        {
            self.batch.cancel_stage();
            log.flush_staged_prefix();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EffectError {
    Full,
    Allocation,
    Closed,
    SequenceOvertaken,
    Projection,
}

/// Exact failure surface of the prebuilt GenerationReset record. This path
/// never grows the journal or constructs a variable batch, so allocation and
/// capacity are intentionally unrepresentable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GenerationResetPlanError {
    Closed,
    SequenceOvertaken,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EffectSettlementPlanError {
    StaleLease,
    Projection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EffectClosePlanError {
    AlreadyClosed,
}

enum AppendPlan {
    /// The ordinary no-prefix hot path. The exclusive authority guard keeps
    /// the validated usage snapshot coherent until Apply.
    Direct {
        record: QueuedEffectRecord,
        usage: EffectRegionUsage,
    },
    /// A later owner transition reserved behind an existing staged prefix.
    /// The record and charge already reside in the sole journal; owner Apply
    /// performs only an allocation-free activation.
    BehindStagedPrefix(StagedEffect),
}

#[derive(Debug)]
struct PendingRejectIndex {
    sequence: ApplySequence,
    batch: Arc<EffectBatch>,
    effect_index: usize,
}

/// Stable O(1) read evidence cloned under the authority read guard. Public
/// rejection construction and JSON serialization happen only after that guard
/// is released.
#[derive(Debug)]
pub(super) struct PendingRecentReject {
    expected_hash: RawTxHash,
    batch: Arc<EffectBatch>,
    effect_index: usize,
}

impl PendingRecentReject {
    pub(super) fn public_reject(self) -> Result<CommittedPublicReject, EffectError> {
        let rejection = self
            .batch
            .effects()
            .get(self.effect_index)
            .and_then(CommittedEffect::recordable_rejection)
            .ok_or(EffectError::Projection)?;
        if rejection.raw_hash() != self.expected_hash {
            return Err(EffectError::Projection);
        }
        Ok(rejection.public_reject())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EffectDisposition {
    Published,
    CircuitDisposed,
    Retain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EffectToken {
    source: EffectLeaseSource,
    sequence: ApplySequence,
    processed: EffectProgress,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EffectLeaseSource {
    Queued,
    GenerationReset,
}

/// One deterministic external endpoint position within a committed outcome.
///
/// A semantic outcome may fan out to several endpoints. Persisting this step
/// in the sole effect authority prevents cancellation during a later endpoint
/// from replaying an earlier callback, ban or database write. Only the
/// currently executing endpoint retains the unavoidable at-least-once
/// action/acknowledgement window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EffectEndpoint {
    RecentReject,
    Callback,
    Ban,
    Relay,
}

impl EffectEndpoint {
    pub(super) const ORDER: [Self; 4] =
        [Self::RecentReject, Self::Callback, Self::Ban, Self::Relay];
    const COUNT: usize = Self::ORDER.len();

    fn from_offset(offset: usize) -> Option<Self> {
        Self::ORDER.get(offset).copied()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct EffectProgress(usize);

impl EffectProgress {
    fn current<'batch>(self, batch: &'batch EffectBatch) -> Option<EffectWork<'batch>> {
        if self.0 >= batch.publication_steps() {
            return None;
        }
        let effect_index = self.0 / EffectEndpoint::COUNT;
        let endpoint = EffectEndpoint::from_offset(self.0 % EffectEndpoint::COUNT)?;
        Some(EffectWork {
            effect_index,
            effect: batch.effects().get(effect_index)?,
            endpoint,
        })
    }

    fn advance(self, batch: &EffectBatch) -> Result<(Self, bool), EffectProgressError> {
        if self.current(batch).is_none() {
            return Err(EffectProgressError::Complete);
        }
        let next = Self(
            self.0
                .checked_add(1)
                .ok_or(EffectProgressError::Arithmetic)?,
        );
        Ok((next, next.0 == batch.publication_steps()))
    }

    fn is_complete(self, batch: &EffectBatch) -> bool {
        self.0 == batch.publication_steps()
    }
}

#[derive(Clone, Copy)]
pub(super) struct EffectWork<'batch> {
    pub(super) effect_index: usize,
    pub(super) effect: &'batch CommittedEffect,
    pub(super) endpoint: EffectEndpoint,
}

#[derive(Debug)]
#[must_use = "effect I/O must return its exact authority settlement"]
pub(super) struct EffectReceipt {
    token: EffectToken,
    batch: Arc<EffectBatch>,
    processed: EffectProgress,
}

/// Complete observation consumed by the sole effect publisher from one
/// coherent journal cut. `Idle` is temporary absence; only
/// `ClosedAndDrained` is terminal.
#[must_use = "the sole publisher must publish, wait, or terminate from this observation"]
pub(super) enum EffectPublicationObservation {
    Receipt(EffectReceipt),
    Idle,
    ClosedAndDrained,
}

impl EffectReceipt {
    /// The first not-yet-processed endpoint in this immutable batch receipt.
    /// Progress is tentative and local while endpoint I/O runs; only
    /// settlement can advance or remove the resident authority record.
    pub(super) fn current(&self) -> Option<EffectWork<'_>> {
        self.processed.current(&self.batch)
    }

    pub(super) fn mark_current_processed(&mut self) -> Result<bool, EffectProgressError> {
        let (processed, complete) = self.processed.advance(&self.batch)?;
        self.processed = processed;
        Ok(complete)
    }

    pub(super) fn retain(self) -> EffectSettlement {
        EffectSettlement {
            token: self.token,
            batch: self.batch,
            processed: self.processed,
            disposition: EffectDisposition::Retain,
        }
    }

    pub(super) fn into_complete(
        self,
    ) -> Result<CompletedEffectReceipt, EffectReceiptCompletionFailure> {
        if self.processed.is_complete(&self.batch) {
            Ok(CompletedEffectReceipt {
                token: self.token,
                batch: self.batch,
            })
        } else {
            Err(EffectReceiptCompletionFailure {
                error: EffectProgressError::Incomplete,
                receipt: self,
            })
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EffectProgressError {
    Complete,
    Incomplete,
    Arithmetic,
}

#[derive(Debug)]
#[must_use = "an incomplete effect receipt still requires settlement"]
pub(super) struct EffectReceiptCompletionFailure {
    error: EffectProgressError,
    receipt: EffectReceipt,
}

impl EffectReceiptCompletionFailure {
    pub(super) fn into_parts(self) -> (EffectProgressError, EffectReceipt) {
        (self.error, self.receipt)
    }
}

#[derive(Debug)]
#[must_use = "a completed effect receipt must settle the authority charge"]
pub(super) struct CompletedEffectReceipt {
    token: EffectToken,
    batch: Arc<EffectBatch>,
}

impl CompletedEffectReceipt {
    pub(super) fn published(self) -> EffectSettlement {
        let processed = EffectProgress(self.batch.publication_steps());
        EffectSettlement {
            token: self.token,
            batch: self.batch,
            processed,
            disposition: EffectDisposition::Published,
        }
    }

    pub(super) fn circuit_disposed(self) -> EffectSettlement {
        let processed = EffectProgress(self.batch.publication_steps());
        EffectSettlement {
            token: self.token,
            batch: self.batch,
            processed,
            disposition: EffectDisposition::CircuitDisposed,
        }
    }
}

#[derive(Debug)]
#[must_use = "effect settlement must be applied or discarded as stale"]
pub(super) struct EffectSettlement {
    token: EffectToken,
    batch: Arc<EffectBatch>,
    processed: EffectProgress,
    disposition: EffectDisposition,
}

impl EffectSettlement {
    /// Terminal rejection counters derived from the immutable committed batch.
    /// An incomplete retained lease contributes nothing; a complete retained
    /// lease is terminal because `plan_settlement` normalizes it to Published.
    /// The runtime publishes this copy only after the settlement Apply
    /// succeeds, so cancellation and lease replay cannot double-count it.
    pub(super) fn rejection_metrics(&self) -> crate::metrics::RejectionMetrics {
        let mut metrics = crate::metrics::RejectionMetrics::default();
        if !self.processed.is_complete(&self.batch) {
            return metrics;
        }
        for effect in self.batch.effects() {
            match effect {
                CommittedEffect::Rejected(rejection) => {
                    metrics.record(rejection.public_reject().reject());
                }
                CommittedEffect::PeerCohortRevoked(revocation) => {
                    if let Some(culprit) = revocation.culprit() {
                        metrics.record(culprit.reason().reject());
                    }
                }
                CommittedEffect::Accepted(_)
                | CommittedEffect::ChainCommitted { .. }
                | CommittedEffect::RemoteExpired { .. }
                | CommittedEffect::RemoteIngressReleased(_)
                | CommittedEffect::ParentTransactionsRequested(_)
                | CommittedEffect::GenerationReset => {}
            }
        }
        metrics
    }
}

struct SettlementMutation {
    disposition: EffectDisposition,
    processed: EffectProgress,
}

enum SettlementTarget {
    Queued(SettlementMutation),
    GenerationReset(SettlementMutation),
}

pub(in crate::authority) struct EffectSettlementMutation(SettlementTarget);

pub(super) enum EffectSettlementPlan {
    Apply(EffectSettlementMutation),
    Superseded,
}

struct ResetPlan {
    record: EffectRecord,
}

#[derive(Default)]
enum EffectMutation {
    #[default]
    None,
    Append(AppendPlan),
    Reset(ResetPlan),
    Close,
}

#[derive(Default)]
pub(super) struct EffectDelta(EffectMutation);

impl EffectDelta {
    pub(super) fn is_empty(&self) -> bool {
        matches!(self.0, EffectMutation::None)
    }

    /// Explicitly cancel the only effect-plan shape that has already changed
    /// the sole journal. Direct append, reset, close and no-op plans remain
    /// owned values with no live journal mutation.
    pub(super) fn rollback_staged_with_wake(
        self,
    ) -> Result<Option<EffectWakeTransition>, EffectError> {
        match self.0 {
            EffectMutation::Append(AppendPlan::BehindStagedPrefix(staged)) => {
                staged.rollback_with_wake().map(Some)
            }
            EffectMutation::None
            | EffectMutation::Append(AppendPlan::Direct { .. })
            | EffectMutation::Reset(_)
            | EffectMutation::Close => Ok(None),
        }
    }
}

#[cfg(test)]
impl EffectDelta {
    pub(in crate::authority) fn has_exclusive_write(&self) -> bool {
        !matches!(self.0, EffectMutation::None)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EffectPublisherLevel {
    Idle,
    Available,
    ClosedAndDrained,
}

/// Copy-only effect state used to derive post-commit wake edges.
///
/// The journal remains authoritative. This projection is captured before and
/// after Apply and is never retained by the authority or consulted for effect
/// selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct EffectWakeProjection {
    publisher: EffectPublisherLevel,
    usage: EffectRegionUsage,
}

/// Immediate journal edge returned by one job's explicit precommit rollback.
/// It carries no policy or authority and is consumed only as a lossy wake
/// prompt after the job has released every owner/generation guard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use = "the runtime must publish the exact staged-effect rollback edge"]
pub(in crate::authority) struct EffectWakeTransition {
    before: EffectWakeProjection,
    after: EffectWakeProjection,
}

impl EffectWakeTransition {
    pub(super) const fn projections(self) -> (EffectWakeProjection, EffectWakeProjection) {
        (self.before, self.after)
    }

    pub(super) fn publisher_advanced(self) -> bool {
        self.after.publisher_advanced_from(self.before)
    }

    pub(super) fn capacity_released(self) -> bool {
        self.after.capacity_released_from(self.before)
    }
}

impl EffectWakeProjection {
    pub(super) fn publisher_advanced_from(self, before: Self) -> bool {
        self.publisher != EffectPublisherLevel::Idle && self.publisher != before.publisher
    }

    pub(super) fn capacity_released_from(self, before: Self) -> bool {
        self.usage.remote.batches < before.usage.remote.batches
            || self.usage.remote.bytes < before.usage.remote.bytes
            || self.usage.ordinary.batches < before.usage.ordinary.batches
            || self.usage.ordinary.bytes < before.usage.ordinary.bytes
            || self.usage.total.batches < before.usage.total.batches
            || self.usage.total.bytes < before.usage.total.bytes
    }
}

#[derive(Debug)]
pub(super) struct EffectLog {
    limits: EffectLimits,
    queued: VecDeque<QueuedEffectRecord>,
    /// Bounded, non-publishable prefix reorder buffer for disjoint owner
    /// commits. This is staging inside the sole journal, not a second effect
    /// authority: every committed record moves into `queued` exactly once.
    staged: Vec<StagedEffectRecord>,
    latest_generation_reset: Option<EffectRecord>,
    /// Derived, charged lookup into resident immutable batches. Sequence is
    /// part of the value so completion of an older rejection cannot erase a
    /// newer result for the same raw transaction hash.
    pending_recent_rejects: HashMap<RawTxHash, PendingRejectIndex>,
    usage: EffectRegionUsage,
    closed: bool,
    generation_reset_batch: Arc<EffectBatch>,
}

impl EffectLog {
    pub(super) fn new(limits: EffectLimits) -> Result<Self, EffectConfigError> {
        let mut queued = VecDeque::new();
        queued
            .try_reserve(limits.regions.total.batches)
            .map_err(|_| EffectConfigError::Allocation)?;
        let mut staged = Vec::new();
        staged
            .try_reserve_exact(MAX_STAGED_EFFECTS)
            .map_err(|_| EffectConfigError::Allocation)?;
        Ok(Self {
            limits,
            queued,
            staged,
            latest_generation_reset: None,
            pending_recent_rejects: HashMap::new(),
            usage: EffectRegionUsage::default(),
            closed: false,
            generation_reset_batch: EffectBatch::reset()?,
        })
    }

    pub(super) fn ensure_open(&self) -> Result<(), EffectError> {
        if self.is_closed() {
            Err(EffectError::Closed)
        } else {
            Ok(())
        }
    }

    pub(super) const fn is_closed(&self) -> bool {
        self.closed
    }

    pub(super) fn limits(&self) -> EffectLimits {
        self.limits
    }

    pub(super) fn build_publication(
        &self,
        policy: EffectPolicy,
        effects: Vec<CommittedEffect>,
    ) -> Result<EffectPublication, EffectBuildError> {
        EffectPublication::new(policy, effects, self.limits)
    }

    /// Build the common one-effect publication without an infallible
    /// single-item `Vec` allocation. Shape failures are projection defects;
    /// allocation remains the ordinary resource outcome owned by `EffectError`.
    pub(super) fn build_single_publication(
        &self,
        policy: EffectPolicy,
        effect: CommittedEffect,
    ) -> Result<EffectPublication, EffectError> {
        let mut effects = Vec::new();
        effects
            .try_reserve_exact(1)
            .map_err(|_| EffectError::Allocation)?;
        effects.push(effect);
        EffectPublication::new(policy, effects, self.limits).map_err(|_| EffectError::Projection)
    }

    pub(super) fn ordered_publication(
        &self,
        policy: EffectPolicy,
        maximum_effects: usize,
    ) -> Result<OrderedEffectPublication, EffectError> {
        self.ensure_open()?;
        let mut effects = Vec::new();
        effects
            .try_reserve(maximum_effects.min(self.limits.batch_bound(policy.class()).max_effects))
            .map_err(|_| EffectError::Allocation)?;
        Ok(OrderedEffectPublication {
            policy,
            effects,
            charge_bytes: 0,
            limits: self.limits,
            usage: self.usage,
        })
    }

    /// Select the largest leading remote cleanup cohort that fits one effect
    /// batch. The caller supplies deadline order; this method preserves that
    /// order and never turns attacker-originated expiry into trusted or
    /// critical journal work.
    pub(super) fn build_remote_prefix(
        &self,
        mut effects: Vec<CommittedEffect>,
    ) -> Result<Option<RemoteEffectPrefix>, EffectBuildError> {
        let mut selected = 0usize;
        let mut bytes = 0usize;
        let bound = self.limits.batch_bound(EffectClass::Remote);
        for effect in &effects {
            if selected == bound.max_effects {
                break;
            }
            let effect_bytes = effect.charge_bytes().ok_or(EffectBuildError::Arithmetic)?;
            let next_bytes = bytes
                .checked_add(effect_bytes)
                .ok_or(EffectBuildError::Arithmetic)?;
            if next_bytes > bound.max_bytes {
                if selected == 0 {
                    return Err(EffectBuildError::TooLarge);
                }
                break;
            }
            bytes = next_bytes;
            selected = selected
                .checked_add(1)
                .ok_or(EffectBuildError::Arithmetic)?;
        }
        let Some(selected) = NonZeroUsize::new(selected) else {
            return Ok(None);
        };
        effects.truncate(selected.get());
        let publication = EffectPublication::new(EffectPolicy::Remote, effects, self.limits)?;
        Ok(Some(RemoteEffectPrefix {
            publication,
            selected,
        }))
    }

    pub(super) fn operational_usage(&self) -> crate::metrics::EffectUsage {
        crate::metrics::EffectUsage {
            remote_batches: self.usage.remote.batches,
            remote_bytes: self.usage.remote.bytes,
            ordinary_batches: self.usage.ordinary.batches,
            ordinary_bytes: self.usage.ordinary.bytes,
            total_batches: self.usage.total.batches,
            total_bytes: self.usage.total.bytes,
        }
    }

    pub(super) fn wake_projection(&self) -> EffectWakeProjection {
        EffectWakeProjection {
            publisher: self.publication_level(),
            usage: self.usage,
        }
    }

    pub(super) fn plan_publication_with_log(
        &mut self,
        log: &Arc<Mutex<Self>>,
        publication: &EffectPublication,
        sequence: ApplySequence,
    ) -> Result<EffectDelta, EffectError> {
        self.preflight_publication(publication)?;
        self.validate_new_sequence(sequence)?;
        let class = publication.policy.class();
        let bytes = publication.batch.charge_bytes();
        if self.usage.fits(self.limits.regions, class, bytes) {
            let pending_rejects = publication.batch.pending_recent_reject_count();
            let usage = self
                .usage
                .checked_charge(class, bytes)
                .ok_or(EffectError::Projection)?;
            let record = QueuedEffectRecord {
                record: EffectRecord {
                    sequence,
                    batch: Arc::clone(&publication.batch),
                    processed: EffectProgress::default(),
                },
                class,
            };
            if self.staged.is_empty() {
                if self
                    .pending_recent_rejects
                    .try_reserve(pending_rejects)
                    .is_err()
                {
                    return if publication.policy.can_reset() {
                        Ok(self.reset_delta(sequence))
                    } else {
                        Err(EffectError::Allocation)
                    };
                }
                return Ok(EffectDelta(EffectMutation::Append(AppendPlan::Direct {
                    record,
                    usage,
                })));
            }
            if self.staged.len() == MAX_STAGED_EFFECTS {
                return if publication.policy.can_reset() {
                    Ok(self.reset_delta(sequence))
                } else {
                    Err(EffectError::Full)
                };
            }
            let deferred_rejects = self
                .staged
                .iter()
                .filter(|staged| staged.record.record.batch.stage_state() != EFFECT_STAGE_CANCELLED)
                .try_fold(0usize, |count, staged| {
                    count.checked_add(staged.record.record.batch.pending_recent_reject_count())
                })
                .and_then(|count| count.checked_add(pending_rejects))
                .ok_or(EffectError::Projection)?;
            if self
                .pending_recent_rejects
                .try_reserve(deferred_rejects)
                .is_err()
            {
                return if publication.policy.can_reset() {
                    Ok(self.reset_delta(sequence))
                } else {
                    Err(EffectError::Allocation)
                };
            }
            // A later exclusive owner result cannot enter `queued` while an
            // earlier shared owner result is still pending. Reserve its exact
            // suffix slot and charge now, before any owner mutation; Apply
            // performs only the infallible activation below.
            publication.batch.begin_stage()?;
            self.usage = usage;
            let position = self
                .staged
                .binary_search_by_key(&sequence, |staged| staged.record.record.sequence)
                .unwrap_or_else(|position| position);
            self.staged.insert(position, StagedEffectRecord { record });
            return Ok(EffectDelta(EffectMutation::Append(
                AppendPlan::BehindStagedPrefix(StagedEffect {
                    log: Arc::clone(log),
                    batch: Arc::clone(&publication.batch),
                    sequence,
                    class,
                    charge_bytes: bytes,
                    finalized: false,
                }),
            )));
        }
        if publication.policy.can_reset() {
            return Ok(self.reset_delta(sequence));
        }
        Err(EffectError::Full)
    }

    /// Reserve one complete bounded publication in the sole log without
    /// making it visible. Pending-reject index capacity is reserved before
    /// the record enters the staged suffix, so activation is allocation-free
    /// for Accepted, rejection and remote-release batches alike.
    pub(super) fn stage_publication(
        log: &Arc<Mutex<Self>>,
        delta: EffectDelta,
    ) -> Result<StagedEffect, EffectError> {
        let EffectMutation::Append(plan) = delta.0 else {
            return Err(EffectError::Projection);
        };
        let record = match plan {
            AppendPlan::BehindStagedPrefix(staged) => {
                if !Arc::ptr_eq(log, &staged.log) {
                    return Err(EffectError::Projection);
                }
                return Ok(staged);
            }
            AppendPlan::Direct { record, .. } => record,
        };
        let mut effects = log.lock();
        effects.ensure_open()?;
        effects.validate_staged_sequence(record.record.sequence)?;
        if effects.staged.len() == MAX_STAGED_EFFECTS {
            return Err(EffectError::Full);
        }
        let class = record.class;
        let charge_bytes = record.record.batch.charge_bytes();
        if !effects
            .usage
            .fits(effects.limits.regions, class, charge_bytes)
        {
            return Err(EffectError::Full);
        }
        let pending_rejects = effects
            .staged
            .iter()
            .filter(|staged| staged.record.record.batch.stage_state() != EFFECT_STAGE_CANCELLED)
            .try_fold(0usize, |count, staged| {
                count.checked_add(staged.record.record.batch.pending_recent_reject_count())
            })
            .and_then(|count| count.checked_add(record.record.batch.pending_recent_reject_count()))
            .ok_or(EffectError::Projection)?;
        effects
            .pending_recent_rejects
            .try_reserve(pending_rejects)
            .map_err(|_| EffectError::Allocation)?;
        effects.usage = effects
            .usage
            .checked_charge(class, charge_bytes)
            .ok_or(EffectError::Projection)?;
        if let Err(error) = record.record.batch.begin_stage() {
            effects.release_staged_charge(class, charge_bytes)?;
            return Err(error);
        }
        let batch = Arc::clone(&record.record.batch);
        let sequence = record.record.sequence;
        let position = effects
            .staged
            .binary_search_by_key(&sequence, |staged| staged.record.record.sequence)
            .unwrap_or_else(|position| position);
        effects
            .staged
            .insert(position, StagedEffectRecord { record });
        Ok(StagedEffect {
            log: Arc::clone(log),
            batch,
            sequence,
            class,
            charge_bytes,
            finalized: false,
        })
    }

    fn release_staged_charge(
        &mut self,
        class: EffectClass,
        charge_bytes: usize,
    ) -> Result<(), EffectError> {
        self.usage = self
            .usage
            .checked_release(class, charge_bytes)
            .ok_or(EffectError::Projection)?;
        Ok(())
    }

    fn flush_staged_prefix(&mut self) {
        while let Some(staged) = self.staged.first() {
            match staged.record.record.batch.stage_state() {
                EFFECT_STAGE_PENDING => break,
                EFFECT_STAGE_COMMITTED => {
                    let staged = self.staged.remove(0);
                    self.queue_record(staged.record);
                }
                EFFECT_STAGE_CANCELLED => {
                    self.staged.remove(0);
                }
                _ => break,
            }
        }
    }

    fn activate_staged(&mut self, sequence: ApplySequence, batch: &Arc<EffectBatch>) {
        // The move-only token was created only after this exact sequence and
        // batch acquired a preallocated staged slot and capacity charge.
        debug_assert!(self.staged.iter().any(|staged| {
            staged.record.record.sequence == sequence
                && Arc::ptr_eq(&staged.record.record.batch, batch)
        }));
        batch.commit_stage();
        self.flush_staged_prefix();
    }

    fn queue_record(&mut self, record: QueuedEffectRecord) {
        let sequence = record.record.sequence;
        let batch = Arc::clone(&record.record.batch);
        for (effect_index, rejection) in batch.pending_recent_rejects() {
            self.pending_recent_rejects.insert(
                rejection.raw_hash(),
                PendingRejectIndex {
                    sequence,
                    batch: Arc::clone(&batch),
                    effect_index,
                },
            );
        }
        self.queued.push_back(record);
    }

    /// Validate every publication condition that is independent of its fresh
    /// Apply stamp. Callers use this before touching the global clock bank, so
    /// a closed journal, impossible batch shape, or full non-resettable region
    /// cannot consume identity/order capacity.
    pub(super) fn preflight_publication(
        &self,
        publication: &EffectPublication,
    ) -> Result<(), EffectError> {
        self.ensure_open()?;
        let class = publication.policy.class();
        let bytes = publication.batch.charge_bytes();
        let bound = self.limits.batch_bound(class);
        if publication.batch.effects().len() > bound.max_effects || bytes > bound.max_bytes {
            return Err(EffectError::Projection);
        }
        if self.usage.fits(self.limits.regions, class, bytes) || publication.policy.can_reset() {
            Ok(())
        } else {
            Err(EffectError::Full)
        }
    }

    pub(super) fn plan_generation_reset(
        &self,
        sequence: ApplySequence,
    ) -> Result<EffectDelta, GenerationResetPlanError> {
        if self.is_closed() {
            return Err(GenerationResetPlanError::Closed);
        }
        if self.sequence_is_overtaken(sequence) {
            return Err(GenerationResetPlanError::SequenceOvertaken);
        }
        Ok(self.reset_delta(sequence))
    }

    /// Publish chain-transition detail or collapse it to the same constant-size
    /// generation reset when either the batch shape or current journal capacity
    /// cannot preserve every item. Only chain convergence may use this path:
    /// the reset rebuilds its authoritative projections, while peer revocation
    /// and other non-rebuildable security actions require exact publication.
    pub(super) fn plan_chain_rebuildable(
        &mut self,
        log: &Arc<Mutex<Self>>,
        effects: Vec<CommittedEffect>,
        sequence: ApplySequence,
    ) -> Result<EffectDelta, EffectError> {
        self.ensure_open()?;
        self.validate_new_sequence(sequence)?;
        let publication =
            match EffectPublication::new(EffectPolicy::CriticalRebuildable, effects, self.limits) {
                Ok(publication) => publication,
                Err(
                    EffectBuildError::TooMany
                    | EffectBuildError::TooLarge
                    | EffectBuildError::Arithmetic,
                ) => return Ok(self.reset_delta(sequence)),
                Err(EffectBuildError::Empty | EffectBuildError::ReservedReset) => {
                    return Err(EffectError::Projection);
                }
            };
        self.plan_publication_with_log(log, &publication, sequence)
    }

    fn reset_delta(&self, sequence: ApplySequence) -> EffectDelta {
        EffectDelta(EffectMutation::Reset(ResetPlan {
            record: EffectRecord {
                sequence,
                batch: Arc::clone(&self.generation_reset_batch),
                processed: EffectProgress::default(),
            },
        }))
    }

    /// Borrow the minimum committed publication record without moving it out
    /// of the authority. Producer transitions cannot remove a queued head and
    /// a newer reset semantically subsumes an older reset receipt.
    fn publication_record(&self) -> Option<(EffectLeaseSource, &EffectRecord)> {
        let queued = self.queued.front();
        let reset = self.latest_generation_reset.as_ref();
        if let Some(staged) = self.staged.first()
            && queued
                .map(|queued| queued.record.sequence > staged.record.record.sequence)
                .unwrap_or(true)
            && reset
                .map(|reset| reset.sequence > staged.record.record.sequence)
                .unwrap_or(true)
        {
            return None;
        }
        match (queued, reset) {
            (Some(queued), Some(reset)) if reset.sequence < queued.record.sequence => {
                Some((EffectLeaseSource::GenerationReset, reset))
            }
            (Some(queued), _) => Some((EffectLeaseSource::Queued, &queued.record)),
            (None, Some(reset)) => Some((EffectLeaseSource::GenerationReset, reset)),
            (None, None) => None,
        }
    }

    fn publication_level(&self) -> EffectPublisherLevel {
        if self.publication_record().is_some() {
            EffectPublisherLevel::Available
        } else if self.is_closed_and_drained() {
            EffectPublisherLevel::ClosedAndDrained
        } else {
            EffectPublisherLevel::Idle
        }
    }

    /// Derive the publisher's total state from the same journal cut that owns
    /// head ordering and terminal drain. Only the Receipt arm clones the
    /// immutable batch pointer, and this method is not used by Apply wake
    /// projection.
    pub(super) fn publication_observation(&self) -> EffectPublicationObservation {
        let Some((source, record)) = self.publication_record() else {
            return if self.is_closed_and_drained() {
                EffectPublicationObservation::ClosedAndDrained
            } else {
                EffectPublicationObservation::Idle
            };
        };
        EffectPublicationObservation::Receipt(EffectReceipt {
            token: EffectToken {
                source,
                sequence: record.sequence,
                processed: record.processed,
            },
            batch: Arc::clone(&record.batch),
            processed: record.processed,
        })
    }

    pub(super) fn plan_settlement(
        &self,
        settlement: &EffectSettlement,
    ) -> Result<EffectSettlementPlan, EffectSettlementPlanError> {
        match settlement.token.source {
            EffectLeaseSource::Queued => {
                let queued = self
                    .queued
                    .front()
                    .ok_or(EffectSettlementPlanError::StaleLease)?;
                Self::validate_exact_record(&queued.record, settlement)?;
                let disposition = Self::validated_disposition(settlement)?;
                match disposition {
                    EffectDisposition::Published | EffectDisposition::CircuitDisposed => self
                        .usage
                        .checked_release(queued.class, queued.record.batch.charge_bytes()),
                    EffectDisposition::Retain => Some(self.usage),
                }
                .ok_or(EffectSettlementPlanError::Projection)?;
                Ok(EffectSettlementPlan::Apply(EffectSettlementMutation(
                    SettlementTarget::Queued(SettlementMutation {
                        disposition,
                        processed: settlement.processed,
                    }),
                )))
            }
            EffectLeaseSource::GenerationReset => {
                if !Arc::ptr_eq(&settlement.batch, &self.generation_reset_batch) {
                    return Err(EffectSettlementPlanError::StaleLease);
                }
                let reset = self
                    .latest_generation_reset
                    .as_ref()
                    .ok_or(EffectSettlementPlanError::StaleLease)?;
                if reset.sequence > settlement.token.sequence {
                    Self::validated_disposition(settlement)?;
                    return Ok(EffectSettlementPlan::Superseded);
                }
                Self::validate_exact_record(reset, settlement)?;
                let disposition = Self::validated_disposition(settlement)?;
                Ok(EffectSettlementPlan::Apply(EffectSettlementMutation(
                    SettlementTarget::GenerationReset(SettlementMutation {
                        disposition,
                        processed: settlement.processed,
                    }),
                )))
            }
        }
    }

    fn validate_exact_record(
        record: &EffectRecord,
        settlement: &EffectSettlement,
    ) -> Result<(), EffectSettlementPlanError> {
        if record.sequence != settlement.token.sequence
            || !Arc::ptr_eq(&record.batch, &settlement.batch)
            || record.processed != settlement.token.processed
        {
            return Err(EffectSettlementPlanError::StaleLease);
        }
        Ok(())
    }

    fn validated_disposition(
        settlement: &EffectSettlement,
    ) -> Result<EffectDisposition, EffectSettlementPlanError> {
        if settlement.processed < settlement.token.processed
            || settlement.processed.0 > settlement.batch.publication_steps()
        {
            return Err(EffectSettlementPlanError::Projection);
        }
        match settlement.disposition {
            EffectDisposition::Published | EffectDisposition::CircuitDisposed
                if !settlement.processed.is_complete(&settlement.batch) =>
            {
                Err(EffectSettlementPlanError::Projection)
            }
            EffectDisposition::Retain if settlement.processed.is_complete(&settlement.batch) => {
                Ok(EffectDisposition::Published)
            }
            disposition => Ok(disposition),
        }
    }

    pub(super) fn plan_close(&self) -> Result<EffectDelta, EffectClosePlanError> {
        if self.closed {
            return Err(EffectClosePlanError::AlreadyClosed);
        }
        Ok(EffectDelta(EffectMutation::Close))
    }

    pub(super) fn apply(&mut self, delta: EffectDelta) -> Option<Arc<EffectBatch>> {
        match delta.0 {
            EffectMutation::None => None,
            EffectMutation::Append(plan) => {
                match plan {
                    AppendPlan::Direct { record, usage } => {
                        self.usage = usage;
                        self.queue_record(record);
                    }
                    AppendPlan::BehindStagedPrefix(mut staged) => {
                        self.activate_staged(staged.sequence, &staged.batch);
                        staged.finalized = true;
                    }
                }
                None
            }
            EffectMutation::Reset(plan) => self
                .latest_generation_reset
                .replace(plan.record)
                .map(|record| record.batch),
            EffectMutation::Close => {
                self.closed = true;
                None
            }
        }
    }

    pub(in crate::authority) fn apply_settlement(
        &mut self,
        mutation: EffectSettlementMutation,
    ) -> Result<Option<Arc<EffectBatch>>, EffectSettlementPlanError> {
        match mutation.0 {
            SettlementTarget::Queued(plan) => self.apply_queued_settlement(plan),
            SettlementTarget::GenerationReset(plan) => self.apply_reset_settlement(plan),
        }
    }

    fn apply_queued_settlement(
        &mut self,
        plan: SettlementMutation,
    ) -> Result<Option<Arc<EffectBatch>>, EffectSettlementPlanError> {
        let queued = self
            .queued
            .front()
            .ok_or(EffectSettlementPlanError::StaleLease)?;
        let after_usage = match plan.disposition {
            EffectDisposition::Published | EffectDisposition::CircuitDisposed => self
                .usage
                .checked_release(queued.class, queued.record.batch.charge_bytes())
                .ok_or(EffectSettlementPlanError::Projection)?,
            EffectDisposition::Retain => self.usage,
        };
        let mut queued = self
            .queued
            .pop_front()
            .ok_or(EffectSettlementPlanError::StaleLease)?;
        match plan.disposition {
            EffectDisposition::Published | EffectDisposition::CircuitDisposed => {
                self.usage = after_usage;
                for (effect_index, rejection) in queued.record.batch.pending_recent_rejects() {
                    let hash = rejection.raw_hash();
                    if self
                        .pending_recent_rejects
                        .get(&hash)
                        .is_some_and(|pending| {
                            pending.sequence == queued.record.sequence
                                && pending.effect_index == effect_index
                                && Arc::ptr_eq(&pending.batch, &queued.record.batch)
                        })
                    {
                        self.pending_recent_rejects.remove(&hash);
                    }
                }
                Ok(Some(queued.record.batch))
            }
            EffectDisposition::Retain => {
                queued.record.processed = plan.processed;
                self.queued.push_front(queued);
                Ok(None)
            }
        }
    }

    fn apply_reset_settlement(
        &mut self,
        plan: SettlementMutation,
    ) -> Result<Option<Arc<EffectBatch>>, EffectSettlementPlanError> {
        let mut reset = self
            .latest_generation_reset
            .take()
            .ok_or(EffectSettlementPlanError::StaleLease)?;
        match plan.disposition {
            EffectDisposition::Published | EffectDisposition::CircuitDisposed => {
                Ok(Some(reset.batch))
            }
            EffectDisposition::Retain => {
                reset.processed = plan.processed;
                self.latest_generation_reset = Some(reset);
                Ok(None)
            }
        }
    }

    pub(super) fn is_closed_and_drained(&self) -> bool {
        self.closed
            && self.queued.is_empty()
            && self.staged.is_empty()
            && self.latest_generation_reset.is_none()
            && self.pending_recent_rejects.is_empty()
            && self.usage == EffectRegionUsage::default()
    }

    pub(super) fn pending_recent_reject(&self, hash: &RawTxHash) -> Option<PendingRecentReject> {
        let pending = self.pending_recent_rejects.get(hash)?;
        Some(PendingRecentReject {
            expected_hash: hash.clone(),
            batch: Arc::clone(&pending.batch),
            effect_index: pending.effect_index,
        })
    }

    fn validate_new_sequence(&self, sequence: ApplySequence) -> Result<(), EffectError> {
        if self.sequence_is_overtaken(sequence) {
            Err(EffectError::SequenceOvertaken)
        } else {
            Ok(())
        }
    }

    fn sequence_is_overtaken(&self, sequence: ApplySequence) -> bool {
        let queued = self.queued.back().map(|queued| queued.record.sequence);
        let staged = self
            .staged
            .last()
            .map(|staged| staged.record.record.sequence);
        let reset = self
            .latest_generation_reset
            .as_ref()
            .map(|reset| reset.sequence);
        let latest = queued.into_iter().chain(staged).chain(reset).max();
        latest.is_some_and(|latest| latest >= sequence)
    }

    fn validate_staged_sequence(&self, sequence: ApplySequence) -> Result<(), EffectError> {
        let queued = self.queued.back().map(|queued| queued.record.sequence);
        let reset = self
            .latest_generation_reset
            .as_ref()
            .map(|reset| reset.sequence);
        if queued
            .into_iter()
            .chain(reset)
            .any(|latest| latest >= sequence)
            || self
                .staged
                .binary_search_by_key(&sequence, |staged| staged.record.record.sequence)
                .is_ok()
        {
            Err(EffectError::SequenceOvertaken)
        } else {
            Ok(())
        }
    }
}

impl OrderedEffectPublication {
    /// Add one complete canonical item outcome. `Full` means the caller may
    /// commit the already compiled item prefix and retry this item later;
    /// `Projection` means one indivisible effect violates startup-proved
    /// shape or arithmetic and therefore cannot be repaired by truncation.
    pub(super) fn push(&mut self, effect: CommittedEffect) -> Result<(), OrderedEffectAppendError> {
        let effect_bytes = effect
            .charge_bytes()
            .ok_or(OrderedEffectAppendError::Projection)?;
        let next_count = self
            .effects
            .len()
            .checked_add(1)
            .ok_or(OrderedEffectAppendError::Projection)?;
        let next_bytes = self
            .charge_bytes
            .checked_add(effect_bytes)
            .ok_or(OrderedEffectAppendError::Projection)?;
        let bound = self.limits.batch_bound(self.policy.class());
        if next_count > bound.max_effects || next_bytes > bound.max_bytes {
            return if self.effects.is_empty() {
                Err(OrderedEffectAppendError::Projection)
            } else {
                Err(OrderedEffectAppendError::Full)
            };
        }
        if !self
            .usage
            .fits(self.limits.regions, self.policy.class(), next_bytes)
        {
            return Err(OrderedEffectAppendError::Full);
        }
        self.effects.push(effect);
        self.charge_bytes = next_bytes;
        Ok(())
    }

    pub(super) fn finish(self) -> Result<Option<EffectPublication>, EffectError> {
        if self.effects.is_empty() {
            return Ok(None);
        }
        EffectPublication::new(self.policy, self.effects, self.limits)
            .map(Some)
            .map_err(|_| EffectError::Projection)
    }
}

#[cfg(test)]
#[path = "tests/support/effect.rs"]
pub(in crate::authority) mod test_support;

#[cfg(test)]
mod staged_record_tests {
    use super::*;
    use ckb_types::packed::Byte32;

    const TEST_EFFECT_BYTES: usize = 1024 * 1024;

    fn limits(remote_batches: usize) -> EffectLimits {
        EffectLimits::partitioned(
            EffectCapacity::new(remote_batches, TEST_EFFECT_BYTES),
            EffectCapacity::new(1, TEST_EFFECT_BYTES),
            EffectCapacity::new(1, TEST_EFFECT_BYTES),
            EffectBatchBounds::new(
                EffectBatchBound::new(1, TEST_EFFECT_BYTES),
                EffectBatchBound::new(1, TEST_EFFECT_BYTES * 2),
                EffectBatchBound::new(1, TEST_EFFECT_BYTES * 3),
            ),
        )
        .expect("the staged-effect fixture has one indivisible slot per region")
    }

    fn log(remote_batches: usize) -> Arc<Mutex<EffectLog>> {
        Arc::new(Mutex::new(EffectLog::new(limits(remote_batches)).expect(
            "the staged-effect fixture reserves its bounded storage",
        )))
    }

    fn wide_log(batches: usize) -> Arc<Mutex<EffectLog>> {
        let capacity = EffectCapacity::new(batches, TEST_EFFECT_BYTES);
        let limits = EffectLimits::partitioned(
            capacity,
            capacity,
            capacity,
            EffectBatchBounds::new(
                EffectBatchBound::new(1, TEST_EFFECT_BYTES),
                EffectBatchBound::new(1, TEST_EFFECT_BYTES),
                EffectBatchBound::new(1, TEST_EFFECT_BYTES),
            ),
        )
        .expect("the wide staged-effect fixture preserves every capacity hierarchy");
        Arc::new(Mutex::new(
            EffectLog::new(limits).expect("the wide fixture reserves its bounded storage"),
        ))
    }

    fn publication(log: &Arc<Mutex<EffectLog>>, nonce: u8) -> EffectPublication {
        log.lock()
            .build_publication(
                EffectPolicy::Remote,
                vec![CommittedEffect::Accepted(CommittedAcceptance::Duplicate {
                    tx_hash: RawTxHash(Byte32::new([nonce; 32])),
                    requesting_peer: None,
                })],
            )
            .expect("one Accepted effect fits the fixture envelope")
    }

    fn rejection_publication(
        log: &Arc<Mutex<EffectLog>>,
        tx: Arc<TransactionView>,
        message: &str,
    ) -> EffectPublication {
        log.lock()
            .build_publication(
                EffectPolicy::Remote,
                vec![CommittedEffect::Rejected(CommittedRejection::Validation {
                    tx,
                    audience: RejectionAudience::from_ingress(None),
                    reason: CommittedPublicReject::new(Reject::Invalidated(message.to_owned())),
                })],
            )
            .expect("one rejection effect fits the fixture envelope")
    }

    fn delta(
        log: &Arc<Mutex<EffectLog>>,
        publication: &EffectPublication,
        sequence: u128,
    ) -> EffectDelta {
        log.lock()
            .plan_publication_with_log(log, publication, ApplySequence(sequence))
            .expect("the planned fixture publication fits before staging")
    }

    fn stage(
        log: &Arc<Mutex<EffectLog>>,
        publication: &EffectPublication,
        sequence: u128,
    ) -> StagedEffect {
        let delta = delta(log, publication, sequence);
        EffectLog::stage_publication(log, delta)
            .expect("the pure Accepted fixture acquires a staging slot")
    }

    fn observation_sequence(log: &Arc<Mutex<EffectLog>>) -> Option<ApplySequence> {
        match log.lock().publication_observation() {
            EffectPublicationObservation::Receipt(receipt) => Some(receipt.token.sequence),
            EffectPublicationObservation::Idle | EffectPublicationObservation::ClosedAndDrained => {
                None
            }
        }
    }

    fn settle_head(log: &Arc<Mutex<EffectLog>>) {
        let receipt = match log.lock().publication_observation() {
            EffectPublicationObservation::Receipt(receipt) => receipt,
            EffectPublicationObservation::Idle | EffectPublicationObservation::ClosedAndDrained => {
                panic!("the fixture head is publishable")
            }
        };
        let settlement = CompletedEffectReceipt {
            token: receipt.token,
            batch: receipt.batch,
        }
        .published();
        let delta = match log
            .lock()
            .plan_settlement(&settlement)
            .expect("the exact head settlement plans")
        {
            EffectSettlementPlan::Apply(delta) => delta,
            EffectSettlementPlan::Superseded => panic!("the exact head is not superseded"),
        };
        drop(
            log.lock()
                .apply_settlement(delta)
                .expect("the exact planned settlement remains applicable"),
        );
    }

    #[test]
    fn staged_capacity_competition_cannot_overbook_the_original_region() {
        let log = log(1);
        let first_publication = publication(&log, 1);
        let second_publication = publication(&log, 2);
        let first_delta = delta(&log, &first_publication, 1);
        let second_delta = delta(&log, &second_publication, 2);
        let first = EffectLog::stage_publication(&log, first_delta)
            .expect("the first stage owns the only remote slot");
        assert!(matches!(
            EffectLog::stage_publication(&log, second_delta),
            Err(EffectError::Full)
        ));
        first
            .rollback()
            .expect("pre-commit rollback releases capacity");
        assert_eq!(log.lock().usage, EffectRegionUsage::default());
    }

    #[test]
    fn staged_rejection_is_invisible_until_allocation_free_activation() {
        let log = log(1);
        let transaction = Arc::new(TransactionBuilder::default().version(61u32).build());
        let publication = rejection_publication(&log, Arc::clone(&transaction), "staged");
        let staged = stage(&log, &publication, 61);
        {
            let effects = log.lock();
            assert_eq!(effects.staged.len(), 1);
            assert!(effects.pending_recent_rejects.is_empty());
            assert!(matches!(
                effects.publication_observation(),
                EffectPublicationObservation::Idle
            ));
        }

        staged.activate();
        let effects = log.lock();
        assert!(effects.staged.is_empty());
        assert_eq!(effects.pending_recent_rejects.len(), 1);
        assert!(matches!(
            effects.publication_observation(),
            EffectPublicationObservation::Receipt(ref receipt)
                if receipt.token.sequence == ApplySequence(61)
        ));
    }

    #[test]
    fn staged_rejection_rollback_restores_capacity_without_visibility() {
        let log = log(1);
        let transaction = Arc::new(TransactionBuilder::default().version(62u32).build());
        let publication = rejection_publication(&log, transaction, "rollback");
        let staged = stage(&log, &publication, 62);
        staged
            .rollback()
            .expect("precommit rejection rollback returns its exact charge");

        let effects = log.lock();
        assert_eq!(effects.usage, EffectRegionUsage::default());
        assert!(effects.staged.is_empty());
        assert!(effects.pending_recent_rejects.is_empty());
        assert!(matches!(
            effects.publication_observation(),
            EffectPublicationObservation::Idle
        ));
    }

    #[test]
    fn earlier_sequence_overtaken_by_a_later_stage_is_typed_stale_input() {
        let log = wide_log(2);
        let later_publication = publication(&log, 2);
        let later = stage(&log, &later_publication, 2);
        let earlier_publication = publication(&log, 1);
        assert!(matches!(
            log.lock()
                .plan_publication_with_log(&log, &earlier_publication, ApplySequence(1)),
            Err(EffectError::SequenceOvertaken)
        ));
        later.activate();
        assert!(matches!(
            log.lock()
                .plan_publication_with_log(&log, &earlier_publication, ApplySequence(1)),
            Err(EffectError::SequenceOvertaken)
        ));
    }

    #[test]
    fn no_prefix_append_preserves_the_direct_hot_path() {
        let log = log(1);
        let publication = publication(&log, 2);
        let log_owners = Arc::strong_count(&log);
        let planned = delta(&log, &publication, 2);
        assert_eq!(Arc::strong_count(&log), log_owners);
        assert!(log.lock().staged.is_empty());
        drop(log.lock().apply(planned));
        assert_eq!(Arc::strong_count(&log), log_owners);
        assert!(log.lock().staged.is_empty());
        assert_eq!(observation_sequence(&log), Some(ApplySequence(2)));
    }

    #[test]
    fn reverse_activation_preserves_apply_sequence_publication_order() {
        let log = log(2);
        let first_publication = publication(&log, 3);
        let second_publication = publication(&log, 4);
        let first = stage(&log, &first_publication, 10);
        let second = stage(&log, &second_publication, 11);

        second.activate();
        assert_eq!(observation_sequence(&log), None);
        first.activate();
        assert_eq!(observation_sequence(&log), Some(ApplySequence(10)));
        settle_head(&log);
        assert_eq!(observation_sequence(&log), Some(ApplySequence(11)));
    }

    #[test]
    fn per_job_rollback_cancels_a_stale_prefix_and_exposes_its_committed_suffix() {
        let log = log(2);
        let first_publication = publication(&log, 31);
        let second_publication = publication(&log, 32);
        let first = stage(&log, &first_publication, 100);
        let second = stage(&log, &second_publication, 101);

        first
            .rollback()
            .expect("the stale job returns its exact precommit charge");
        second.activate();
        assert_eq!(observation_sequence(&log), Some(ApplySequence(101)));
        assert_eq!(log.lock().staged.len(), 0);
        settle_head(&log);
        assert_eq!(log.lock().usage, EffectRegionUsage::default());
    }

    #[test]
    fn per_job_terminalization_exposes_a_suffix_after_an_older_head_settles() {
        let log = log(3);
        let older = publication(&log, 40);
        let older_delta = delta(&log, &older, 90);
        drop(log.lock().apply(older_delta));
        let first_publication = publication(&log, 41);
        let second_publication = publication(&log, 42);
        let first = stage(&log, &first_publication, 100);
        let second = stage(&log, &second_publication, 101);

        settle_head(&log);
        assert_eq!(observation_sequence(&log), None);
        first
            .rollback()
            .expect("the stale job returns its exact precommit charge");
        second.activate();
        assert_eq!(observation_sequence(&log), Some(ApplySequence(101)));
    }

    #[test]
    fn activation_wake_uses_the_same_log_cut_after_the_old_head_settles() {
        let log = log(2);
        let old_publication = publication(&log, 51);
        let old = delta(&log, &old_publication, 150);
        drop(log.lock().apply(old));
        let staged_publication = publication(&log, 52);
        let staged = stage(&log, &staged_publication, 151);

        settle_head(&log);
        assert_eq!(observation_sequence(&log), None);
        let wake = staged.activate_with_wake();
        assert!(
            wake.publisher_advanced(),
            "Idle to publishable activation must retain its exact wake edge"
        );
        assert_eq!(observation_sequence(&log), Some(ApplySequence(151)));
    }

    #[test]
    fn later_exclusive_append_cannot_overtake_an_earlier_pending_stage() {
        let log = log(2);
        let earlier_publication = publication(&log, 43);
        let earlier = stage(&log, &earlier_publication, 110);
        let later_publication = publication(&log, 44);
        let later = delta(&log, &later_publication, 111);

        drop(log.lock().apply(later));
        assert_eq!(
            observation_sequence(&log),
            None,
            "a later committed owner effect cannot become publishable before the earlier staged sequence terminalizes"
        );
        earlier.activate();
        assert_eq!(observation_sequence(&log), Some(ApplySequence(110)));
        settle_head(&log);
        assert_eq!(observation_sequence(&log), Some(ApplySequence(111)));
    }

    #[test]
    fn staged_rollback_between_append_plan_and_apply_cannot_resurrect_usage() {
        let log = log(2);
        let earlier_publication = publication(&log, 45);
        let earlier = stage(&log, &earlier_publication, 120);
        let later_publication = publication(&log, 46);
        let later_charge = later_publication.batch.charge_bytes();
        let later = delta(&log, &later_publication, 121);

        earlier
            .rollback()
            .expect("the earlier owner never committed and returns its exact charge");
        drop(log.lock().apply(later));
        assert_eq!(
            log.lock().usage.remote,
            EffectUsage {
                batches: 1,
                bytes: later_charge,
            },
            "applying the later plan must not restore the rolled-back stage charge"
        );
    }

    #[test]
    fn staged_rollback_between_settlement_plan_and_apply_cannot_resurrect_usage() {
        let log = log(2);
        let queued_publication = publication(&log, 47);
        let queued = delta(&log, &queued_publication, 130);
        drop(log.lock().apply(queued));
        let staged_publication = publication(&log, 48);
        let staged = stage(&log, &staged_publication, 131);
        let receipt = match log.lock().publication_observation() {
            EffectPublicationObservation::Receipt(receipt) => receipt,
            EffectPublicationObservation::Idle | EffectPublicationObservation::ClosedAndDrained => {
                panic!("the older queued record is publishable")
            }
        };
        let settlement = CompletedEffectReceipt {
            token: receipt.token,
            batch: receipt.batch,
        }
        .published();
        let planned = match log
            .lock()
            .plan_settlement(&settlement)
            .expect("the exact queued settlement plans")
        {
            EffectSettlementPlan::Apply(delta) => delta,
            EffectSettlementPlan::Superseded => panic!("the queued head is not superseded"),
        };

        staged
            .rollback()
            .expect("the later owner never committed and returns its exact charge");
        drop(
            log.lock()
                .apply_settlement(planned)
                .expect("the exact planned settlement remains applicable"),
        );
        assert_eq!(
            log.lock().usage,
            EffectRegionUsage::default(),
            "settling the queued record must not restore the rolled-back stage charge"
        );
    }

    #[test]
    fn committed_suffix_reset_and_close_drain_in_sequence() {
        let log = log(2);
        let first_publication = publication(&log, 49);
        let first = stage(&log, &first_publication, 140);
        let second_publication = publication(&log, 50);
        let second = delta(&log, &second_publication, 141);
        drop(log.lock().apply(second));
        let reset = log
            .lock()
            .plan_generation_reset(ApplySequence(142))
            .expect("the later reset plans behind the staged suffix");
        drop(log.lock().apply(reset));
        let close = log
            .lock()
            .plan_close()
            .expect("close preserves every resident record");
        drop(log.lock().apply(close));

        assert_eq!(observation_sequence(&log), None);
        assert!(!log.lock().is_closed_and_drained());
        first.activate();
        assert_eq!(observation_sequence(&log), Some(ApplySequence(140)));
        settle_head(&log);
        assert_eq!(observation_sequence(&log), Some(ApplySequence(141)));
        settle_head(&log);
        assert_eq!(observation_sequence(&log), Some(ApplySequence(142)));
        settle_head(&log);
        assert!(log.lock().is_closed_and_drained());
    }

    #[test]
    fn full_staged_suffix_rejects_a_later_nonresettable_append_before_apply() {
        let log = wide_log(MAX_STAGED_EFFECTS + 1);
        let mut pending = Vec::new();
        pending
            .try_reserve_exact(MAX_STAGED_EFFECTS)
            .expect("the fixed staged-token carrier allocates");
        for offset in 0..MAX_STAGED_EFFECTS {
            let publication = publication(&log, u8::try_from(offset).expect("64 fits in u8"));
            pending.push(stage(
                &log,
                &publication,
                200 + u128::try_from(offset).expect("64 fits in u128"),
            ));
        }
        let overflow = publication(&log, 65);
        assert!(matches!(
            log.lock().plan_publication_with_log(
                &log,
                &overflow,
                ApplySequence(200 + u128::try_from(MAX_STAGED_EFFECTS).expect("64 fits in u128")),
            ),
            Err(EffectError::Full)
        ));
        assert_eq!(log.lock().staged.len(), MAX_STAGED_EFFECTS);
        drop(pending);
        assert!(log.lock().staged.is_empty());
        assert_eq!(log.lock().usage, EffectRegionUsage::default());
    }

    #[test]
    fn newer_same_hash_reject_becomes_visible_only_after_its_staged_barrier_clears() {
        let log = log(3);
        let transaction = Arc::new(TransactionBuilder::default().build());
        let hash = RawTxHash(transaction.hash());
        let old_publication =
            rejection_publication(&log, Arc::clone(&transaction), "old rejection");
        let old = delta(&log, &old_publication, 300);
        drop(log.lock().apply(old));
        let old_projection = log
            .lock()
            .pending_recent_reject(&hash)
            .expect("the old committed rejection owns the visible projection")
            .public_reject()
            .expect("the old projection is structurally valid");

        let barrier_publication = publication(&log, 66);
        let barrier = stage(&log, &barrier_publication, 301);
        let new_publication = rejection_publication(&log, transaction, "new rejection");
        let new = delta(&log, &new_publication, 302);
        drop(log.lock().apply(new));
        let still_old = log
            .lock()
            .pending_recent_reject(&hash)
            .expect("the blocked suffix cannot shadow the queued projection")
            .public_reject()
            .expect("the queued projection remains structurally valid");
        assert_eq!(still_old, old_projection);

        settle_head(&log);
        assert!(log.lock().pending_recent_reject(&hash).is_none());
        barrier
            .rollback()
            .expect("rolling back the uncommitted barrier exposes the committed suffix");
        let new_projection = log
            .lock()
            .pending_recent_reject(&hash)
            .expect("the newly queued rejection now owns the projection")
            .public_reject()
            .expect("the new projection is structurally valid");
        assert_ne!(new_projection, old_projection);
        assert_eq!(observation_sequence(&log), Some(ApplySequence(302)));
        settle_head(&log);
        assert!(log.lock().pending_recent_reject(&hash).is_none());
        assert_eq!(log.lock().usage, EffectRegionUsage::default());
    }

    #[test]
    fn committed_suffix_activation_and_flush_use_only_preallocated_storage() {
        let log = log(2);
        let barrier_publication = publication(&log, 67);
        let barrier = stage(&log, &barrier_publication, 310);
        let transaction = Arc::new(TransactionBuilder::default().build());
        let suffix_publication = rejection_publication(&log, transaction, "bounded suffix");
        let suffix = delta(&log, &suffix_publication, 311);
        let before = {
            let effects = log.lock();
            (
                effects.queued.capacity(),
                effects.staged.capacity(),
                effects.pending_recent_rejects.capacity(),
            )
        };

        drop(log.lock().apply(suffix));
        let after_activation = {
            let effects = log.lock();
            (
                effects.queued.capacity(),
                effects.staged.capacity(),
                effects.pending_recent_rejects.capacity(),
            )
        };
        assert_eq!(after_activation, before);
        barrier
            .rollback()
            .expect("the uncommitted prefix releases and flushes the suffix");
        let after_flush = {
            let effects = log.lock();
            (
                effects.queued.capacity(),
                effects.staged.capacity(),
                effects.pending_recent_rejects.capacity(),
            )
        };
        assert_eq!(after_flush, before);
        settle_head(&log);
        assert_eq!(log.lock().usage, EffectRegionUsage::default());
    }

    #[test]
    fn precommit_rollback_is_invisible_and_releases_the_exact_charge() {
        let log = log(1);
        let publication = publication(&log, 5);
        stage(&log, &publication, 20)
            .rollback()
            .expect("rollback is exact before owner mutation");
        let effects = log.lock();
        assert!(effects.staged.is_empty());
        assert!(effects.queued.is_empty());
        assert_eq!(effects.usage, EffectRegionUsage::default());
        assert!(effects.pending_recent_rejects.is_empty());
    }

    #[test]
    fn staged_record_is_not_visible_to_publisher_or_recent_reject_queries() {
        let log = log(1);
        let publication = publication(&log, 6);
        let staged = stage(&log, &publication, 30);
        assert_eq!(observation_sequence(&log), None);
        assert!(
            log.lock()
                .pending_recent_reject(&RawTxHash(Byte32::new([6; 32])))
                .is_none()
        );
        staged.activate();
        assert_eq!(observation_sequence(&log), Some(ApplySequence(30)));
    }

    #[test]
    fn close_and_later_reset_cannot_overtake_an_earlier_stage() {
        let closed_log = log(1);
        let closed_publication = publication(&closed_log, 7);
        let staged = stage(&closed_log, &closed_publication, 40);
        let close = closed_log
            .lock()
            .plan_close()
            .expect("close records after the already reserved stage");
        drop(closed_log.lock().apply(close));
        assert!(!closed_log.lock().is_closed_and_drained());
        staged.activate();
        assert_eq!(observation_sequence(&closed_log), Some(ApplySequence(40)));
        settle_head(&closed_log);
        assert!(closed_log.lock().is_closed_and_drained());

        let reset_log = log(1);
        let reset_publication = publication(&reset_log, 8);
        let staged = stage(&reset_log, &reset_publication, 50);
        let reset = reset_log
            .lock()
            .plan_generation_reset(ApplySequence(51))
            .expect("the later reset reserves its sequence");
        drop(reset_log.lock().apply(reset));
        assert_eq!(observation_sequence(&reset_log), None);
        staged.activate();
        assert_eq!(observation_sequence(&reset_log), Some(ApplySequence(50)));
        settle_head(&reset_log);
        assert_eq!(observation_sequence(&reset_log), Some(ApplySequence(51)));
    }
}
