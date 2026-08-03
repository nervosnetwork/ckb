use super::ban::PeerBanLease;
#[cfg(test)]
use super::state::RejectionKind;
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
/// envelope, not the legacy service journal's `TxEntrySnapshot`. Keeping the
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

    #[cfg(test)]
    fn for_foundation() -> Self {
        Self {
            regions: EffectRegions::new(
                EffectCapacity::new(8, 64 * 1024),
                EffectCapacity::new(12, 128 * 1024),
                EffectCapacity::new(14, 192 * 1024),
            ),
            bounds: EffectBatchBounds::new(
                EffectBatchBound::new(16, 32 * 1024),
                EffectBatchBound::new(16, 64 * 1024),
                EffectBatchBound::new(64, 128 * 1024),
            ),
        }
    }

    fn batch_bound(self, class: EffectClass) -> EffectBatchBound {
        self.bounds.for_class(class)
    }

    #[cfg(test)]
    pub(super) fn max_batch_bytes_for_foundation(self, policy: EffectPolicy) -> usize {
        self.batch_bound(policy.class()).max_bytes
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
    /// Foundation-only bounded effect fixture.
    #[cfg(test)]
    Foundation {
        tx: Arc<TransactionView>,
        audience: RejectionAudience,
        reason: RejectionKind,
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
            #[cfg(test)]
            Self::Foundation { tx, .. } => tx.hash(),
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
            #[cfg(test)]
            Self::Foundation { reason, .. } => (*reason).into(),
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
            #[cfg(test)]
            Self::Foundation { .. } => true,
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
    pub(super) const fn administrative(lease: PeerBanLease) -> Self {
        Self {
            lease,
            culprit: None,
        }
    }

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
    pub(super) fn malformed_for_foundation(
        peer: PeerIndex,
        tx_hash: RawTxHash,
        reason: CommittedPublicReject,
    ) -> Option<Self> {
        Self::malformed(PeerBanLease::for_foundation(peer), tx_hash, reason)
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
    pub(super) const fn duplicate_remote_submission(
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

    #[cfg(test)]
    pub(super) const fn ingress_peer(self) -> Option<PeerIndex> {
        self.ingress_peer
    }

    #[cfg(test)]
    pub(super) const fn foundation() -> Self {
        Self { ingress_peer: None }
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
    /// A duplicate Remote submission was not retained, or a remote-attributed
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
                    #[cfg(test)]
                    CommittedRejection::Foundation { tx, .. } => {
                        EFFECT_ENVELOPE_BYTES.checked_add(tx.data().total_size())
                    }
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

    #[cfg(test)]
    pub(super) fn new_for_foundation(
        policy: EffectPolicy,
        effects: Vec<CommittedEffect>,
        limits: EffectLimits,
    ) -> Result<Self, EffectBuildError> {
        Self::new(policy, effects, limits)
    }

    #[cfg(test)]
    pub(super) fn charge_bytes_for_foundation(&self) -> usize {
        self.batch.charge_bytes()
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
struct EffectEnvelope {
    sequence: ApplySequence,
    class: Option<EffectClass>,
    batch: Arc<EffectBatch>,
    processed: EffectProgress,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct EffectSnapshot {
    queued: VecDeque<EffectEnvelope>,
    active: Option<EffectEnvelope>,
    latest_generation_reset: Option<EffectEnvelope>,
    usage: EffectRegionUsage,
    closed: bool,
}

#[cfg(test)]
impl EffectSnapshot {
    /// Compares the externally committed effect stream while deliberately
    /// ignoring journal batch boundaries and their accounting envelope.
    ///
    /// A commuting authority Apply may publish several effects in one batch,
    /// while its canonical one-at-a-time reference publishes the same effects
    /// in adjacent batches.  Batch shape is a delivery optimization, not a
    /// transaction outcome.  Order, trust class, active/reset position, and
    /// closure state remain observable and therefore stay in the comparison.
    pub(super) fn equivalent_stream(&self, other: &Self) -> bool {
        fn flatten(
            envelopes: impl IntoIterator<Item = EffectEnvelope>,
        ) -> Vec<(Option<EffectClass>, CommittedEffect)> {
            envelopes
                .into_iter()
                .flat_map(|envelope| {
                    envelope
                        .batch
                        .effects()
                        .iter()
                        .cloned()
                        .map(move |effect| (envelope.class, effect))
                        .collect::<Vec<_>>()
                })
                .collect()
        }

        let active = self.active.clone().into_iter();
        let other_active = other.active.clone().into_iter();
        let reset = self.latest_generation_reset.clone().into_iter();
        let other_reset = other.latest_generation_reset.clone().into_iter();

        flatten(self.queued.clone()) == flatten(other.queued.clone())
            && flatten(active) == flatten(other_active)
            && flatten(reset) == flatten(other_reset)
            && self.closed == other.closed
    }
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct EffectObservation {
    pub(super) queued: Vec<ApplySequence>,
    pub(super) active: Option<ApplySequence>,
    pub(super) active_processed_steps: Option<usize>,
    pub(super) latest_generation_reset: Option<ApplySequence>,
    pub(super) remote_usage: EffectUsage,
    pub(super) ordinary_usage: EffectUsage,
    pub(super) total_usage: EffectUsage,
    pub(super) pending_recent_rejects: usize,
    pub(super) closed: bool,
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
    envelope: EffectEnvelope,
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

#[derive(Clone, Copy)]
enum CheckoutSource {
    Queued,
    GenerationReset,
}

struct CheckoutPlan {
    source: CheckoutSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EffectDisposition {
    Published,
    CircuitDisposed,
    Retain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EffectToken {
    sequence: ApplySequence,
    processed: EffectProgress,
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

    fn is_pending(self, batch: &EffectBatch) -> bool {
        self.0 < batch.publication_steps()
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
pub(super) struct EffectLease {
    token: EffectToken,
    batch: Arc<EffectBatch>,
    processed: EffectProgress,
}

impl EffectLease {
    pub(super) fn sequence(&self) -> ApplySequence {
        self.token.sequence
    }

    pub(super) fn effects(&self) -> &[CommittedEffect] {
        self.batch.effects()
    }

    /// The first not-yet-processed endpoint in this exclusive batch lease.
    /// Progress is local to the move-only capability while endpoint I/O runs;
    /// only Retain commits it back into the authority.
    pub(super) fn current(&self) -> Option<EffectWork<'_>> {
        self.processed.current(&self.batch)
    }

    pub(super) fn mark_current_processed(&mut self) -> Result<bool, EffectProgressError> {
        let (processed, complete) = self.processed.advance(&self.batch)?;
        self.processed = processed;
        Ok(complete)
    }

    pub(super) fn charge_bytes(&self) -> usize {
        self.batch.charge_bytes()
    }

    pub(super) fn retain(self) -> EffectSettlement {
        EffectSettlement {
            token: self.token,
            batch: self.batch,
            processed: self.processed,
            disposition: EffectDisposition::Retain,
        }
    }

    pub(super) fn into_complete(self) -> Result<CompletedEffectLease, EffectCompletionFailure> {
        if self.processed.is_complete(&self.batch) {
            Ok(CompletedEffectLease {
                token: self.token,
                batch: self.batch,
            })
        } else {
            Err(EffectCompletionFailure {
                error: EffectProgressError::Incomplete,
                lease: self,
            })
        }
    }

    #[cfg(test)]
    pub(super) fn complete_for_foundation(mut self) -> CompletedEffectLease {
        self.processed = EffectProgress(self.batch.publication_steps());
        CompletedEffectLease {
            token: self.token,
            batch: self.batch,
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
#[must_use = "an incomplete effect lease still owns the active capability"]
pub(super) struct EffectCompletionFailure {
    error: EffectProgressError,
    lease: EffectLease,
}

impl EffectCompletionFailure {
    pub(super) fn into_parts(self) -> (EffectProgressError, EffectLease) {
        (self.error, self.lease)
    }
}

#[derive(Debug)]
#[must_use = "a completed effect lease must settle the authority charge"]
pub(super) struct CompletedEffectLease {
    token: EffectToken,
    batch: Arc<EffectBatch>,
}

impl CompletedEffectLease {
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

struct SettlementPlan {
    disposition: EffectDisposition,
    processed: EffectProgress,
    after_usage: EffectRegionUsage,
}

struct ResetPlan {
    envelope: EffectEnvelope,
}

#[derive(Default)]
enum EffectMutation {
    #[default]
    None,
    Append(AppendPlan),
    Checkout(CheckoutPlan),
    Settle(SettlementPlan),
    Reset(ResetPlan),
    Close,
}

#[derive(Default)]
pub(super) struct EffectDelta(EffectMutation);

#[derive(Debug)]
pub(super) struct EffectLog {
    limits: EffectLimits,
    queued: VecDeque<EffectEnvelope>,
    active: Option<EffectEnvelope>,
    latest_generation_reset: Option<EffectEnvelope>,
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
            active: None,
            latest_generation_reset: None,
            pending_recent_rejects: HashMap::new(),
            usage: EffectRegionUsage::default(),
            closed: false,
            generation_reset_batch: EffectBatch::reset(),
        })
    }

    #[cfg(test)]
    pub(super) fn for_foundation() -> Self {
        let limits = EffectLimits::for_foundation();
        Self {
            limits,
            queued: VecDeque::with_capacity(limits.regions.total.batches),
            active: None,
            latest_generation_reset: None,
            pending_recent_rejects: HashMap::new(),
            usage: EffectRegionUsage::default(),
            closed: false,
            generation_reset_batch: EffectBatch::reset(),
        }
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

    pub(super) fn snapshot(&self) -> EffectSnapshot {
        EffectSnapshot {
            queued: self.queued.clone(),
            active: self.active.clone(),
            latest_generation_reset: self.latest_generation_reset.clone(),
            usage: self.usage,
            closed: self.closed,
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
                envelope: EffectEnvelope {
                    sequence,
                    class: Some(class),
                    batch: Arc::clone(&publication.batch),
                    processed: EffectProgress::default(),
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
            envelope: EffectEnvelope {
                sequence,
                class: None,
                batch: Arc::clone(&self.generation_reset_batch),
                processed: EffectProgress::default(),
            },
        }))
    }

    pub(super) fn plan_checkout(&self) -> Option<(EffectDelta, EffectLease)> {
        if self.active.is_some() {
            return None;
        }
        let queued = self.queued.front();
        let reset = self.latest_generation_reset.as_ref();
        let (source, envelope) = match (queued, reset) {
            (Some(queued), Some(reset)) if reset.sequence < queued.sequence => {
                (CheckoutSource::GenerationReset, reset)
            }
            (Some(queued), _) => (CheckoutSource::Queued, queued),
            (None, Some(reset)) => (CheckoutSource::GenerationReset, reset),
            (None, None) => return None,
        };
        Some((
            EffectDelta(EffectMutation::Checkout(CheckoutPlan { source })),
            EffectLease {
                token: EffectToken {
                    sequence: envelope.sequence,
                    processed: envelope.processed,
                },
                batch: Arc::clone(&envelope.batch),
                processed: envelope.processed,
            },
        ))
    }

    pub(super) fn plan_settlement(
        &self,
        settlement: &EffectSettlement,
    ) -> Result<EffectDelta, EffectSettlementPlanError> {
        let active = self
            .active
            .as_ref()
            .ok_or(EffectSettlementPlanError::StaleLease)?;
        if active.sequence != settlement.token.sequence
            || !Arc::ptr_eq(&active.batch, &settlement.batch)
            || active.processed != settlement.token.processed
        {
            return Err(EffectSettlementPlanError::StaleLease);
        }
        if settlement.processed < active.processed
            || settlement.processed.0 > active.batch.publication_steps()
        {
            return Err(EffectSettlementPlanError::Projection);
        }
        let disposition = match settlement.disposition {
            EffectDisposition::Published | EffectDisposition::CircuitDisposed
                if !settlement.processed.is_complete(&active.batch) =>
            {
                return Err(EffectSettlementPlanError::Projection);
            }
            EffectDisposition::Retain if settlement.processed.is_complete(&active.batch) => {
                EffectDisposition::Published
            }
            disposition => disposition,
        };
        let after_usage = match disposition {
            EffectDisposition::Published | EffectDisposition::CircuitDisposed => {
                active.class.map_or(Some(self.usage), |class| {
                    self.usage
                        .checked_release(class, active.batch.charge_bytes())
                })
            }
            EffectDisposition::Retain => Some(self.usage),
        }
        .ok_or(EffectSettlementPlanError::Projection)?;
        Ok(EffectDelta(EffectMutation::Settle(SettlementPlan {
            disposition,
            processed: settlement.processed,
            after_usage,
        })))
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
                let sequence = plan.envelope.sequence;
                let batch = Arc::clone(&plan.envelope.batch);
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
                self.queued.push_back(plan.envelope);
                None
            }
            EffectMutation::Checkout(plan) => {
                let selected = match plan.source {
                    CheckoutSource::Queued => self.queued.pop_front(),
                    CheckoutSource::GenerationReset => self.latest_generation_reset.take(),
                };
                // The exclusive prepared plan proves this source is present.
                // Keeping the Option branch explicit avoids panic-based
                // invariant handling if future code violates that contract.
                if let Some(selected) = selected {
                    self.active = Some(selected);
                }
                None
            }
            EffectMutation::Settle(plan) => self.apply_settlement(plan),
            EffectMutation::Reset(plan) => self
                .latest_generation_reset
                .replace(plan.envelope)
                .map(|envelope| envelope.batch),
            EffectMutation::Close => {
                self.closed = true;
                None
            }
        }
    }

    fn apply_settlement(&mut self, plan: SettlementPlan) -> Option<Arc<EffectBatch>> {
        let active = self.active.take()?;
        self.usage = plan.after_usage;
        match plan.disposition {
            EffectDisposition::Published | EffectDisposition::CircuitDisposed => {
                for (effect_index, rejection) in active.batch.pending_recent_rejects() {
                    let hash = rejection.raw_hash();
                    if self
                        .pending_recent_rejects
                        .get(&hash)
                        .is_some_and(|pending| {
                            pending.sequence == active.sequence
                                && pending.effect_index == effect_index
                                && Arc::ptr_eq(&pending.batch, &active.batch)
                        })
                    {
                        self.pending_recent_rejects.remove(&hash);
                    }
                }
                Some(active.batch)
            }
            EffectDisposition::Retain => match active.class {
                Some(_) => {
                    let mut active = active;
                    active.processed = plan.processed;
                    self.queued.push_front(active);
                    None
                }
                None => {
                    let mut active = active;
                    active.processed = plan.processed;
                    if self
                        .latest_generation_reset
                        .as_ref()
                        .is_some_and(|latest| latest.sequence > active.sequence)
                    {
                        Some(active.batch)
                    } else {
                        self.latest_generation_reset = Some(active);
                        None
                    }
                }
            },
        }
    }

    pub(super) fn is_closed_and_drained(&self) -> bool {
        self.closed
            && self.queued.is_empty()
            && self.active.is_none()
            && self.latest_generation_reset.is_none()
            && self.pending_recent_rejects.is_empty()
            && self.usage == EffectRegionUsage::default()
    }

    #[cfg(test)]
    pub(super) fn observation(&self) -> EffectObservation {
        EffectObservation {
            queued: self
                .queued
                .iter()
                .map(|envelope| envelope.sequence)
                .collect(),
            active: self.active.as_ref().map(|envelope| envelope.sequence),
            active_processed_steps: self.active.as_ref().map(|envelope| envelope.processed.0),
            latest_generation_reset: self
                .latest_generation_reset
                .as_ref()
                .map(|envelope| envelope.sequence),
            remote_usage: self.usage.remote,
            ordinary_usage: self.usage.ordinary,
            total_usage: self.usage.total,
            pending_recent_rejects: self.pending_recent_rejects.len(),
            closed: self.closed,
        }
    }

    pub(super) fn pending_recent_reject(&self, hash: &RawTxHash) -> Option<PendingRecentReject> {
        let pending = self.pending_recent_rejects.get(hash)?;
        Some(PendingRecentReject {
            expected_hash: hash.clone(),
            batch: Arc::clone(&pending.batch),
            effect_index: pending.effect_index,
        })
    }

    pub(super) fn semantically_consistent(&self, next_sequence: ApplySequence) -> bool {
        let queued_ordered = self
            .queued
            .iter()
            .try_fold(None, |previous, envelope| {
                if envelope.class.is_none()
                    || previous.is_some_and(|previous| previous >= envelope.sequence)
                {
                    None
                } else {
                    Some(Some(envelope.sequence))
                }
            })
            .is_some();
        if !queued_ordered {
            return false;
        }
        let mut rebuilt = EffectRegionUsage::default();
        for envelope in self.queued.iter().chain(self.active.iter()) {
            let Some(class) = envelope.class else {
                if self.active.as_ref() != Some(envelope) {
                    return false;
                }
                continue;
            };
            let Some(next) = rebuilt.checked_charge(class, envelope.batch.charge_bytes()) else {
                return false;
            };
            rebuilt = next;
        }
        let all_sequences_before_clock = self
            .queued
            .iter()
            .chain(self.active.iter())
            .chain(self.latest_generation_reset.iter())
            .all(|envelope| envelope.sequence < next_sequence);
        let all_progress_incomplete = self
            .queued
            .iter()
            .chain(self.active.iter())
            .chain(self.latest_generation_reset.iter())
            .all(|envelope| envelope.processed.is_pending(&envelope.batch));
        let active_precedes_pending = self.active.as_ref().is_none_or(|active| {
            self.queued
                .front()
                .is_none_or(|queued| active.sequence < queued.sequence)
                && self
                    .latest_generation_reset
                    .as_ref()
                    .is_none_or(|reset| active.sequence < reset.sequence)
        });
        let resident = self.queued.iter().chain(self.active.iter());
        let pending_projection_complete =
            resident.clone().all(|envelope| {
                envelope
                    .batch
                    .pending_recent_rejects()
                    .all(|(effect_index, rejection)| {
                        self.pending_recent_rejects
                            .get(&rejection.raw_hash())
                            .is_some_and(|pending| {
                                pending.sequence > envelope.sequence
                                    || (pending.sequence == envelope.sequence
                                        && pending.effect_index >= effect_index
                                        && Arc::ptr_eq(&pending.batch, &envelope.batch))
                            })
                    })
            }) && self.pending_recent_rejects.iter().all(|(hash, pending)| {
                pending
                    .batch
                    .effects()
                    .get(pending.effect_index)
                    .and_then(CommittedEffect::recordable_rejection)
                    .is_some_and(|rejection| rejection.raw_hash() == *hash)
                    && self
                        .queued
                        .iter()
                        .chain(self.active.iter())
                        .any(|envelope| {
                            envelope.sequence == pending.sequence
                                && Arc::ptr_eq(&envelope.batch, &pending.batch)
                        })
            });
        rebuilt == self.usage
            && self.usage_within_limits()
            && all_sequences_before_clock
            && all_progress_incomplete
            && active_precedes_pending
            && self
                .latest_generation_reset
                .as_ref()
                .is_none_or(|reset| reset.class.is_none() && reset.batch.charge_bytes() == 0)
            && pending_projection_complete
    }

    fn usage_within_limits(&self) -> bool {
        self.usage.remote.batches <= self.limits.regions.remote.batches
            && self.usage.remote.bytes <= self.limits.regions.remote.bytes
            && self.usage.ordinary.batches <= self.limits.regions.ordinary.batches
            && self.usage.ordinary.bytes <= self.limits.regions.ordinary.bytes
            && self.usage.total.batches <= self.limits.regions.total.batches
            && self.usage.total.bytes <= self.limits.regions.total.bytes
            && self.usage.remote.batches <= self.usage.ordinary.batches
            && self.usage.ordinary.batches <= self.usage.total.batches
            && self.usage.remote.bytes <= self.usage.ordinary.bytes
            && self.usage.ordinary.bytes <= self.usage.total.bytes
    }

    fn validate_new_sequence(&self, sequence: ApplySequence) -> Result<(), EffectError> {
        let latest = self
            .queued
            .back()
            .into_iter()
            .chain(self.active.iter())
            .chain(self.latest_generation_reset.iter())
            .map(|envelope| envelope.sequence)
            .max();
        if latest.is_some_and(|latest| latest >= sequence) {
            Err(EffectError::Projection)
        } else {
            Ok(())
        }
    }
}
