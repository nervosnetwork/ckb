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
use std::{
    collections::{HashMap, VecDeque},
    num::NonZeroUsize,
    sync::{Arc, LazyLock},
};

const EFFECT_ENVELOPE_BYTES: usize = 128;
/// Conservative residency charge for one raw-hash lookup entry into an
/// immutable committed effect batch. The projection duplicates neither the
/// transaction nor its rejection payload.
const PENDING_REJECT_INDEX_BYTES: usize = 128;
/// Conservative retained-memory charge for one detached packed hash and its
/// `Arc<[RawTxHash]>` allocation share. This matches the existing relayer
/// projection bound without making the authority depend on the service layer.
const PARENT_TRANSACTION_HASH_BYTES: usize = 64;
/// Scalar and view residency beyond the packed transaction bytes retained by
/// one callback-compatible accepted-entry snapshot.
const COMMITTED_ENTRY_SNAPSHOT_OVERHEAD_BYTES: usize =
    std::mem::size_of::<CommittedEntrySnapshot>() + 64;

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
    parents: Arc<[RawTxHash]>,
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
    pub(super) fn new(peer: PeerIndex, parents: Arc<[RawTxHash]>) -> Option<Self> {
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

#[derive(Debug, PartialEq, Eq)]
pub(super) struct EffectBatch {
    effects: Box<[CommittedEffect]>,
    publication_steps: usize,
    charge_bytes: usize,
}

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
            effects: effects.into_boxed_slice(),
            publication_steps,
            charge_bytes,
        }))
    }

    fn reset() -> Arc<Self> {
        Arc::new(Self {
            effects: Box::new([CommittedEffect::GenerationReset]),
            publication_steps: EffectEndpoint::COUNT,
            charge_bytes: 0,
        })
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
}

#[derive(Debug)]
pub(super) struct EffectPublication {
    policy: EffectPolicy,
    batch: Arc<EffectBatch>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EffectError {
    Full,
    Allocation,
    Closed,
    Projection,
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

struct AppendPlan {
    record: QueuedEffectRecord,
    usage: EffectRegionUsage,
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
    after_usage: EffectRegionUsage,
}

enum SettlementTarget {
    Queued(SettlementMutation),
    GenerationReset(SettlementMutation),
}

pub(super) enum EffectSettlementPlan {
    Apply(EffectDelta),
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
    Settle(SettlementTarget),
    Reset(ResetPlan),
    Close,
}

#[derive(Default)]
pub(super) struct EffectDelta(EffectMutation);

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
        Ok(Self {
            limits,
            queued,
            latest_generation_reset: None,
            pending_recent_rejects: HashMap::new(),
            usage: EffectRegionUsage::default(),
            closed: false,
            generation_reset_batch: EffectBatch::reset(),
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
        let publisher = if self.is_closed_and_drained() {
            EffectPublisherLevel::ClosedAndDrained
        } else if !self.queued.is_empty() || self.latest_generation_reset.is_some() {
            EffectPublisherLevel::Available
        } else {
            EffectPublisherLevel::Idle
        };
        EffectWakeProjection {
            publisher,
            usage: self.usage,
        }
    }

    pub(super) fn plan_publication(
        &mut self,
        publication: &EffectPublication,
        sequence: ApplySequence,
    ) -> Result<EffectDelta, EffectError> {
        self.ensure_open()?;
        self.validate_new_sequence(sequence)?;
        let class = publication.policy.class();
        let bytes = publication.batch.charge_bytes();
        let bound = self.limits.batch_bound(class);
        if publication.batch.effects().len() > bound.max_effects || bytes > bound.max_bytes {
            return Err(EffectError::Projection);
        }
        if self.usage.fits(self.limits.regions, class, bytes) {
            if self
                .pending_recent_rejects
                .try_reserve(publication.batch.pending_recent_reject_count())
                .is_err()
            {
                return if publication.policy.can_reset() {
                    Ok(self.reset_delta(sequence))
                } else {
                    Err(EffectError::Allocation)
                };
            }
            let usage = self
                .usage
                .checked_charge(class, bytes)
                .ok_or(EffectError::Projection)?;
            return Ok(EffectDelta(EffectMutation::Append(AppendPlan {
                record: QueuedEffectRecord {
                    record: EffectRecord {
                        sequence,
                        batch: Arc::clone(&publication.batch),
                        processed: EffectProgress::default(),
                    },
                    class,
                },
                usage,
            })));
        }
        if publication.policy.can_reset() {
            return Ok(self.reset_delta(sequence));
        }
        Err(EffectError::Full)
    }

    pub(super) fn plan_generation_reset(
        &self,
        sequence: ApplySequence,
    ) -> Result<EffectDelta, EffectError> {
        self.ensure_open()?;
        self.validate_new_sequence(sequence)?;
        Ok(self.reset_delta(sequence))
    }

    /// Publish chain-transition detail or collapse it to the same constant-size
    /// generation reset when either the batch shape or current journal capacity
    /// cannot preserve every item. Only chain convergence may use this path:
    /// the reset rebuilds its authoritative projections, while peer revocation
    /// and other non-rebuildable security actions require exact publication.
    pub(super) fn plan_chain_rebuildable(
        &mut self,
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
        self.plan_publication(&publication, sequence)
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
    /// of the authority. Production can call this only through the linear sole
    /// publisher claim; producer transitions cannot remove a queued head and a
    /// newer reset semantically subsumes an older reset receipt.
    pub(super) fn publication_receipt(&self) -> Option<EffectReceipt> {
        let queued = self.queued.front();
        let reset = self.latest_generation_reset.as_ref();
        let (source, record) = match (queued, reset) {
            (Some(queued), Some(reset)) if reset.sequence < queued.record.sequence => {
                (EffectLeaseSource::GenerationReset, reset)
            }
            (Some(queued), _) => (EffectLeaseSource::Queued, &queued.record),
            (None, Some(reset)) => (EffectLeaseSource::GenerationReset, reset),
            (None, None) => return None,
        };
        Some(EffectReceipt {
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
                let after_usage = match disposition {
                    EffectDisposition::Published | EffectDisposition::CircuitDisposed => self
                        .usage
                        .checked_release(queued.class, queued.record.batch.charge_bytes()),
                    EffectDisposition::Retain => Some(self.usage),
                }
                .ok_or(EffectSettlementPlanError::Projection)?;
                Ok(EffectSettlementPlan::Apply(EffectDelta(
                    EffectMutation::Settle(SettlementTarget::Queued(SettlementMutation {
                        disposition,
                        processed: settlement.processed,
                        after_usage,
                    })),
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
                Ok(EffectSettlementPlan::Apply(EffectDelta(
                    EffectMutation::Settle(SettlementTarget::GenerationReset(SettlementMutation {
                        disposition,
                        processed: settlement.processed,
                        after_usage: self.usage,
                    })),
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
                self.usage = plan.usage;
                let sequence = plan.record.record.sequence;
                let batch = Arc::clone(&plan.record.record.batch);
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
                self.queued.push_back(plan.record);
                None
            }
            EffectMutation::Settle(SettlementTarget::Queued(plan)) => {
                self.apply_queued_settlement(plan)
            }
            EffectMutation::Settle(SettlementTarget::GenerationReset(plan)) => {
                self.apply_reset_settlement(plan)
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

    fn apply_queued_settlement(&mut self, plan: SettlementMutation) -> Option<Arc<EffectBatch>> {
        let mut queued = self.queued.pop_front()?;
        self.usage = plan.after_usage;
        match plan.disposition {
            EffectDisposition::Published | EffectDisposition::CircuitDisposed => {
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
                Some(queued.record.batch)
            }
            EffectDisposition::Retain => {
                queued.record.processed = plan.processed;
                self.queued.push_front(queued);
                None
            }
        }
    }

    fn apply_reset_settlement(&mut self, plan: SettlementMutation) -> Option<Arc<EffectBatch>> {
        let mut reset = self.latest_generation_reset.take()?;
        match plan.disposition {
            EffectDisposition::Published | EffectDisposition::CircuitDisposed => Some(reset.batch),
            EffectDisposition::Retain => {
                reset.processed = plan.processed;
                self.latest_generation_reset = Some(reset);
                None
            }
        }
    }

    pub(super) fn is_closed_and_drained(&self) -> bool {
        self.closed
            && self.queued.is_empty()
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
        let queued = self.queued.back().map(|queued| queued.record.sequence);
        let reset = self
            .latest_generation_reset
            .as_ref()
            .map(|reset| reset.sequence);
        let latest = queued.into_iter().chain(reset).max();
        if latest.is_some_and(|latest| latest >= sequence) {
            Err(EffectError::Projection)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
#[path = "tests/support/effect.rs"]
pub(in crate::authority) mod test_support;
