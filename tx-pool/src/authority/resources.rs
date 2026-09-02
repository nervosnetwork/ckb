use super::{
    shard::{ShardResourcePlan, ShardedOwnerMap, ShardedOwnerReadCut, ShardedOwnerWriteCut},
    state::{
        ComputeAttribution, EntryVersion, OwnedTx, PreAcceptedEntry, RawTxHash, ValidatedAdmission,
        WorkPermit,
    },
};
use ckb_network::PeerIndex;
use ckb_types::core::TransactionView;
use ckb_util::parking_lot::Mutex;
use std::{
    collections::{HashMap, HashSet},
    num::NonZeroUsize,
    sync::Arc,
};
use tokio::sync::Notify;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct ResourceVector {
    pub(super) entries: usize,
    /// Long-lived authority-owned payload and index residency.
    pub(super) bytes: usize,
    pub(super) edges: usize,
    /// Number of move-only compute capabilities currently outside the
    /// authority guard.
    pub(super) active_work: usize,
    /// Bytes and edges reserved for those capabilities. These fields are
    /// private so callers cannot create an active owner without going through
    /// the compute-grant compiler below.
    compute_bytes: usize,
    compute_edges: usize,
}

/// The sole compiler from transaction/resolution evidence to retained-byte
/// accounting.
///
/// `ResourceVector::bytes` is a weighted physical ceiling, not merely payload
/// bytes. Keeping this policy beside the ledger prevents runtime callers from
/// charging payload, entry and edge limits as three independent copies of the
/// configured byte budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ResidencyPolicy {
    entry_metadata_bytes: usize,
    edge_metadata_bytes: usize,
}

/// One authority-issued compute envelope in the same physical unit used by
/// retained resource accounting. The base payload and metadata policy are
/// sealed into the grant, so worker and settlement code cannot compare a
/// payload-only measurement with a total-residency ceiling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ComputeGrant {
    max_total_retained_bytes: usize,
    max_edges: usize,
    payload_bytes: usize,
    encoded_edges: usize,
    residency: ResidencyPolicy,
}

impl ComputeGrant {
    fn new(
        max_total_retained_bytes: usize,
        max_edges: usize,
        entry: &PreAcceptedEntry,
        residency: ResidencyPolicy,
    ) -> Self {
        Self {
            max_total_retained_bytes,
            max_edges,
            payload_bytes: entry.basis.payload_bytes(),
            encoded_edges: entry.basis.encoded_edges(),
            residency,
        }
    }

    /// Compile the exact retained charge and enforce the grant in one step.
    /// Checked arithmetic failure is an ordinary exclusion: attacker-shaped
    /// evidence must never turn a representational limit into service failure.
    pub(super) fn retained_charge(
        self,
        retained_payload_bytes: usize,
        retained_edges: usize,
    ) -> Option<ResourceVector> {
        let charge = self.residency.charge(
            self.payload_bytes,
            self.encoded_edges,
            retained_payload_bytes,
            retained_edges,
        )?;
        (charge.bytes <= self.max_total_retained_bytes && charge.edges <= self.max_edges)
            .then_some(charge)
    }

    /// Compile a retained phase that keeps only the original transaction
    /// payload while its canonical dependency set may have grown.
    pub(super) fn retained_base_charge(self, retained_edges: usize) -> Option<ResourceVector> {
        self.retained_charge(self.payload_bytes, retained_edges)
    }

    pub(super) const fn max_total_retained_bytes(self) -> usize {
        self.max_total_retained_bytes
    }

    pub(super) const fn max_edges(self) -> usize {
        self.max_edges
    }
}

/// Admission evidence after the authority-owned residency policy has compiled
/// its exact initial charge. The inner charge cannot be supplied by ingress.
pub(super) struct ChargedAdmission {
    admission: ValidatedAdmission,
    charge: ResourceVector,
}

pub(super) struct ReplacementHistoryCharge {
    payload_bytes: usize,
    encoded_edges: usize,
    recovery: ResourceVector,
    retained: ResourceVector,
}

impl ReplacementHistoryCharge {
    pub(super) fn into_parts(self) -> (usize, usize, ResourceVector, ResourceVector) {
        (
            self.payload_bytes,
            self.encoded_edges,
            self.recovery,
            self.retained,
        )
    }
}

impl ChargedAdmission {
    pub(super) fn admission(&self) -> &ValidatedAdmission {
        &self.admission
    }

    pub(super) fn charge(&self) -> ResourceVector {
        self.charge
    }

    pub(super) fn into_parts(self) -> (ValidatedAdmission, ResourceVector) {
        (self.admission, self.charge)
    }
}

impl ResidencyPolicy {
    pub(super) const fn production(
        entry_metadata_bytes: NonZeroUsize,
        edge_metadata_bytes: NonZeroUsize,
    ) -> Self {
        Self {
            entry_metadata_bytes: entry_metadata_bytes.get(),
            edge_metadata_bytes: edge_metadata_bytes.get(),
        }
    }

    pub(super) fn charge(
        self,
        payload_bytes: usize,
        encoded_edges: usize,
        retained_payload_bytes: usize,
        retained_edges: usize,
    ) -> Option<ResourceVector> {
        let edges = encoded_edges.max(retained_edges);
        let metadata = edges
            .checked_mul(self.edge_metadata_bytes)?
            .checked_add(self.entry_metadata_bytes)?;
        Some(ResourceVector::new(
            1,
            payload_bytes
                .max(retained_payload_bytes)
                .checked_add(metadata)?,
            edges,
            0,
        ))
    }
}

impl ResourceVector {
    pub(super) const fn new(
        entries: usize,
        bytes: usize,
        edges: usize,
        active_work: usize,
    ) -> Self {
        Self {
            entries,
            bytes,
            edges,
            active_work,
            compute_bytes: 0,
            compute_edges: 0,
        }
    }

    /// Build one resource-domain limit from disjoint retained and compute
    /// partitions of the same configured physical budget.
    pub(super) fn with_compute_capacity(
        mut self,
        compute_bytes: usize,
        compute_edges: usize,
    ) -> Option<Self> {
        self.bytes.checked_add(compute_bytes)?;
        self.edges.checked_add(compute_edges)?;
        self.compute_bytes = compute_bytes;
        self.compute_edges = compute_edges;
        Some(self)
    }

    pub(super) fn reserve_compute(mut self, grant: ComputeGrant) -> Option<Self> {
        if self.active_work != 0 || self.compute_bytes != 0 || self.compute_edges != 0 {
            return None;
        }
        self.active_work = 1;
        self.compute_bytes = grant.max_total_retained_bytes();
        self.compute_edges = grant.max_edges();
        Some(self)
    }

    pub(super) fn without_compute(mut self) -> Self {
        self.active_work = 0;
        self.compute_bytes = 0;
        self.compute_edges = 0;
        self
    }

    fn has_compute_reservation(self) -> bool {
        self.active_work != 0 || self.compute_bytes != 0 || self.compute_edges != 0
    }

    pub(super) fn total_bytes(self) -> Option<usize> {
        self.bytes.checked_add(self.compute_bytes)
    }

    pub(super) fn total_edges(self) -> Option<usize> {
        self.edges.checked_add(self.compute_edges)
    }

    pub(super) fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_add(other.entries)?,
            bytes: self.bytes.checked_add(other.bytes)?,
            edges: self.edges.checked_add(other.edges)?,
            active_work: self.active_work.checked_add(other.active_work)?,
            compute_bytes: self.compute_bytes.checked_add(other.compute_bytes)?,
            compute_edges: self.compute_edges.checked_add(other.compute_edges)?,
        })
    }

    pub(super) fn checked_sub(self, other: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_sub(other.entries)?,
            bytes: self.bytes.checked_sub(other.bytes)?,
            edges: self.edges.checked_sub(other.edges)?,
            active_work: self.active_work.checked_sub(other.active_work)?,
            compute_bytes: self.compute_bytes.checked_sub(other.compute_bytes)?,
            compute_edges: self.compute_edges.checked_sub(other.compute_edges)?,
        })
    }

    pub(super) fn fits(self, limit: Self) -> bool {
        self.entries <= limit.entries
            && self.bytes <= limit.bytes
            && self.edges <= limit.edges
            && self.active_work <= limit.active_work
            && self.compute_bytes <= limit.compute_bytes
            && self.compute_edges <= limit.compute_edges
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct AcceptedResources {
    pub(super) entries: usize,
    pub(super) serialized_bytes: usize,
    pub(super) resident_bytes: usize,
    pub(super) cycles: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct AcceptedCost {
    pub(super) serialized_bytes: usize,
    pub(super) resident_bytes: usize,
    pub(super) cycles: u64,
}

impl AcceptedCost {
    pub(super) const fn new(serialized_bytes: usize, resident_bytes: usize, cycles: u64) -> Self {
        Self {
            serialized_bytes,
            resident_bytes,
            cycles,
        }
    }
}

impl AcceptedResources {
    pub(super) const fn new(
        entries: usize,
        serialized_bytes: usize,
        resident_bytes: usize,
        cycles: u64,
    ) -> Self {
        Self {
            entries,
            serialized_bytes,
            resident_bytes,
            cycles,
        }
    }

    pub(super) const fn one(cost: AcceptedCost) -> Self {
        Self {
            entries: 1,
            serialized_bytes: cost.serialized_bytes,
            resident_bytes: cost.resident_bytes,
            cycles: cost.cycles,
        }
    }

    pub(super) fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_add(other.entries)?,
            serialized_bytes: self.serialized_bytes.checked_add(other.serialized_bytes)?,
            resident_bytes: self.resident_bytes.checked_add(other.resident_bytes)?,
            cycles: self.cycles.checked_add(other.cycles)?,
        })
    }

    pub(super) fn checked_sub(self, other: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_sub(other.entries)?,
            serialized_bytes: self.serialized_bytes.checked_sub(other.serialized_bytes)?,
            resident_bytes: self.resident_bytes.checked_sub(other.resident_bytes)?,
            cycles: self.cycles.checked_sub(other.cycles)?,
        })
    }

    pub(super) fn fits(self, limit: Self) -> bool {
        self.entries <= limit.entries
            && self.serialized_bytes <= limit.serialized_bytes
            && self.resident_bytes <= limit.resident_bytes
            && self.cycles <= limit.cycles
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ResourceLimits {
    preaccepted: ResourceVector,
    remote: ResourceVector,
    per_peer: ResourceVector,
    replacement_history: ResourceVector,
    accepted: AcceptedResources,
    compute: ComputeLimits,
    residency: ResidencyPolicy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResourceConfigError {
    LimitHierarchy,
    MissingComputeCapacity,
    NonMonotonicComputeEnvelope,
    TransientComputeOverflow,
}

impl ResourceLimits {
    pub(super) fn with_residency_policy(
        preaccepted: ResourceVector,
        remote: ResourceVector,
        per_peer: ResourceVector,
        accepted: AcceptedResources,
        compute: ComputeLimits,
        residency: ResidencyPolicy,
    ) -> Result<Self, ResourceConfigError> {
        if !remote.fits(preaccepted) || !per_peer.fits(remote) {
            return Err(ResourceConfigError::LimitHierarchy);
        }
        if compute.resolved_total_retained_bytes == 0
            || compute.accepted_total_retained_bytes == 0
            || (preaccepted.entries != 0 && preaccepted.active_work == 0)
            || (remote.entries != 0 && remote.active_work == 0)
            || (per_peer.entries != 0 && per_peer.active_work == 0)
        {
            return Err(ResourceConfigError::MissingComputeCapacity);
        }
        if compute.accepted_total_retained_bytes < compute.resolved_total_retained_bytes {
            return Err(ResourceConfigError::NonMonotonicComputeEnvelope);
        }
        for limit in [preaccepted, remote, per_peer] {
            limit
                .total_bytes()
                .and_then(|_| limit.total_edges())
                .ok_or(ResourceConfigError::TransientComputeOverflow)?;
            if limit.active_work != 0
                && (limit.compute_bytes < compute.max_total_retained_bytes()
                    || limit.compute_edges < compute.expanded_edges())
            {
                return Err(ResourceConfigError::MissingComputeCapacity);
            }
        }
        Ok(Self {
            preaccepted,
            remote,
            per_peer,
            // Replacement history is optional and secure-by-default. The
            // explicit builder below enables its bounded subpartition.
            replacement_history: ResourceVector::default(),
            accepted,
            compute,
            residency,
        })
    }

    pub(super) fn with_replacement_history_limit(
        mut self,
        replacement_history: ResourceVector,
    ) -> Result<Self, ResourceConfigError> {
        if !replacement_history.fits(self.preaccepted)
            || replacement_history.has_compute_reservation()
        {
            return Err(ResourceConfigError::LimitHierarchy);
        }
        self.replacement_history = replacement_history;
        Ok(self)
    }

    /// Hard upper bound for one reusable full-query row set. Replacement
    /// history is already charged inside `preaccepted`, so adding a third
    /// partition here would double-count the only optional owner class.
    pub(super) fn max_owner_entries(self) -> Option<usize> {
        self.preaccepted.entries.checked_add(self.accepted.entries)
    }

    /// Immutable hard ceiling for Accepted membership. Administrative
    /// descendant closure uses this configured bound while planning under a
    /// shared generation guard; a live per-shard count could splice
    /// concurrent owner commits and is therefore not a valid traversal bound.
    pub(super) const fn accepted_entry_limit(self) -> usize {
        self.accepted.entries
    }

    /// Maximum number of simultaneously checked-out retained capabilities.
    /// This is also the configured cardinality of the compute-worker topology;
    /// it is not the unrelated membership/RBF mutation-component bound.
    pub(super) const fn active_work_limit(self) -> usize {
        self.preaccepted.active_work
    }

    /// Conservative hard ceiling for all dependency stage permits retained by
    /// one generation.
    ///
    /// Each logical edge can occupy consumer plus waiter rows, and a staged
    /// transition can hold the current and prospective universes at once.
    /// Accepted edge cardinality is conservatively bounded by its resident
    /// bytes (every physical edge costs at least one charged byte). Control-
    /// A published predecessor may remain alive until cleanup while a
    /// successor takes over the same physical row, so the physical-row bound
    /// is multiplied by the configured maximum live capability population:
    /// owner reservations, checked-out work, and the sole maintenance
    /// successor. This deliberately over-approximates scheduler timing while
    /// remaining a hard configuration-derived ceiling.
    pub(super) fn max_dependency_stage_units(self) -> usize {
        let preaccepted_edges = self.preaccepted.total_edges().unwrap_or(usize::MAX);
        let accepted_edge_bound = self.accepted.resident_bytes;
        // One edge can own the before/after consumer and waiter relation
        // cells plus the level and dirty control cells. The stage bank
        // charges all six physical cells because they can coexist until the
        // publishing capability is normalized.
        let relation_and_control_rows_per_stage = preaccepted_edges
            .saturating_add(accepted_edge_bound)
            .saturating_mul(6);
        let live_capabilities = self
            .max_owner_entries()
            .unwrap_or(usize::MAX)
            .saturating_add(self.active_work_limit())
            .saturating_add(1);
        relation_and_control_rows_per_stage
            .saturating_add(1)
            .saturating_mul(live_capabilities)
            .max(1)
    }
}

/// Per-lease upper bounds reserved before attacker-shaped resolve/verify
/// facts can become retained authority state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ComputeLimits {
    resolved_total_retained_bytes: usize,
    accepted_total_retained_bytes: usize,
    expanded_edges: usize,
}

impl ComputeLimits {
    pub(super) const fn new(
        resolved_total_retained_bytes: usize,
        accepted_total_retained_bytes: usize,
        expanded_edges: usize,
    ) -> Self {
        Self {
            resolved_total_retained_bytes,
            accepted_total_retained_bytes,
            expanded_edges,
        }
    }

    fn grant_for(
        self,
        permit: WorkPermit,
        entry: &PreAcceptedEntry,
        residency: ResidencyPolicy,
    ) -> ComputeGrant {
        let max_total_retained_bytes = match permit {
            WorkPermit::ResolveOnly => self.resolved_total_retained_bytes,
            WorkPermit::VerifyOnly(_) => self.accepted_total_retained_bytes,
            WorkPermit::ResolveThenVerify(_) => self
                .resolved_total_retained_bytes
                .max(self.accepted_total_retained_bytes),
        };
        ComputeGrant::new(
            max_total_retained_bytes,
            self.expanded_edges,
            entry,
            residency,
        )
    }

    fn max_total_retained_bytes(self) -> usize {
        self.resolved_total_retained_bytes
            .max(self.accepted_total_retained_bytes)
    }

    fn expanded_edges(self) -> usize {
        self.expanded_edges
    }

    fn admits(self, resources: ResourceVector) -> bool {
        resources.entries == 1
            && !resources.has_compute_reservation()
            && resources.bytes <= self.resolved_total_retained_bytes
            && resources.bytes <= self.accepted_total_retained_bytes
            && resources.edges <= self.expanded_edges
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChargeRecord {
    PreAccepted {
        resources: ResourceVector,
        residency_peer: Option<PeerIndex>,
        compute_peer: Option<PeerIndex>,
    },
    /// Trusted transient ownership of an Accepted replacement victim. It is
    /// charged to both total preacceptance and the dedicated history
    /// partition, and can never carry peer or active-work attribution.
    ReplacementHistory(ResourceVector),
    Accepted(AcceptedResources),
}

impl ChargeRecord {
    fn validate(self) -> Result<(), ResourceError> {
        match self {
            Self::PreAccepted {
                resources,
                residency_peer,
                compute_peer,
            } => {
                if resources.entries != 1
                    || resources.active_work > 1
                    || (resources.active_work == 0 && resources.has_compute_reservation())
                    || (resources.active_work == 1 && resources.compute_bytes == 0)
                    || compute_peer.is_some_and(|peer| Some(peer) != residency_peer)
                    || (resources.active_work == 0 && compute_peer.is_some())
                {
                    return Err(ResourceError::ComputeEnvelope);
                }
                Ok(())
            }
            Self::ReplacementHistory(resources) => {
                if resources.entries != 1 || resources.has_compute_reservation() {
                    Err(ResourceError::ComputeEnvelope)
                } else {
                    Ok(())
                }
            }
            Self::Accepted(_) => Ok(()),
        }
    }

    fn preaccepted(self) -> Option<ResourceVector> {
        match self {
            Self::PreAccepted { resources, .. } | Self::ReplacementHistory(resources) => {
                Some(resources)
            }
            Self::Accepted(_) => None,
        }
    }

    fn replacement_history(self) -> Option<ResourceVector> {
        match self {
            Self::ReplacementHistory(resources) => Some(resources),
            Self::PreAccepted { .. } | Self::Accepted(_) => None,
        }
    }

    fn peer_preaccepted(self) -> Result<Option<(PeerIndex, ResourceVector)>, ResourceError> {
        let Self::PreAccepted {
            resources,
            residency_peer,
            compute_peer,
        } = self
        else {
            return Ok(None);
        };
        let Some(peer) = residency_peer else {
            return if compute_peer.is_none() {
                Ok(None)
            } else {
                Err(ResourceError::AttributionMismatch)
            };
        };
        if compute_peer.is_some_and(|compute_peer| compute_peer != peer) {
            return Err(ResourceError::AttributionMismatch);
        }
        let resources = if compute_peer == Some(peer) {
            resources
        } else {
            resources.without_compute()
        };
        Ok(Some((peer, resources)))
    }

    fn accepted(self) -> Option<AcceptedResources> {
        match self {
            Self::PreAccepted { .. } | Self::ReplacementHistory(_) => None,
            Self::Accepted(resources) => Some(resources),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ResourceError {
    Arithmetic,
    PreAcceptedLimit,
    RemoteLimit,
    PeerLimit(PeerIndex),
    ReplacementHistoryLimit,
    AcceptedLimit,
    ExistingChargeMismatch,
    DuplicateChange,
    ComputeEnvelope,
    AttributionMismatch,
    CapacityBankFault,
    Allocation,
}

pub(super) enum DirectAcceptedInsertionError {
    Resource(ResourceError),
    Contended(ResourceCapacityWaitIdentity),
}

impl From<ResourceError> for DirectAcceptedInsertionError {
    fn from(error: ResourceError) -> Self {
        Self::Resource(error)
    }
}

#[cfg(test)]
#[path = "tests/support/resources.rs"]
pub(in crate::authority) mod test_support;

/// Whether one more checked-out worker can consume the single active-work
/// slot charged by every compute grant. This is a projection of the existing
/// ledger, not another scheduler state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ActiveWorkAvailability {
    Available,
    PreAcceptedExhausted,
    RemoteExhausted,
    PeerExhausted(PeerIndex),
}

/// The owner operation that last changed active-work availability.
///
/// [`EntryVersion`] values are allocated globally and never reused. Pairing
/// that identity with the operation therefore gives the capacity bank an
/// ABA-safe revision without adding another counter or lifecycle authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ActiveWorkOperation {
    Acquire,
    Release,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct ActiveWorkRevision(Option<(EntryVersion, ActiveWorkOperation)>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ActiveWorkRevisionSeal {
    expected: ActiveWorkRevision,
    target: ActiveWorkRevision,
}

impl ActiveWorkRevision {
    /// Seal the revision captured with availability planning to the final
    /// active-work transition in the canonical owner batch. A mixed batch may
    /// have zero net active work, so the target must come from an actual owner
    /// transition rather than from the aggregate capacity delta.
    pub(super) const fn seal(
        self,
        target: EntryVersion,
        operation: ActiveWorkOperation,
    ) -> ActiveWorkRevisionSeal {
        ActiveWorkRevisionSeal {
            expected: self,
            target: Self(Some((target, operation))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ActiveWorkRevisionSealError {
    AlreadySealed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ResourceCapacityBeginError {
    StaleActiveWorkRevision,
    Capacity(ResourceError),
}

impl From<ResourceError> for ResourceCapacityBeginError {
    fn from(error: ResourceError) -> Self {
        Self::Capacity(error)
    }
}

#[derive(Debug)]
pub(super) struct ResourceLedger {
    limits: ResourceLimits,
    capacity: Arc<ResourceCapacityBank>,
}

#[derive(Clone, Copy)]
pub(in crate::authority) struct ResourceRead<'state> {
    entries: &'state ShardedOwnerMap,
    ledger: &'state ResourceLedger,
}

impl ResourceRead<'_> {
    fn totals(self) -> (ResourceTotals, AcceptedResources) {
        match self.entries.resource_totals() {
            Some(totals) => totals,
            None => {
                self.ledger.capacity.mark_faulted();
                (
                    ResourceTotals {
                        preaccepted: ResourceVector::exhausted(),
                        remote: ResourceVector::exhausted(),
                        replacement_history: ResourceVector::exhausted(),
                    },
                    AcceptedResources::exhausted(),
                )
            }
        }
    }

    pub(super) fn preaccepted(self) -> ResourceVector {
        self.totals().0.preaccepted
    }

    #[cfg(test)]
    pub(super) fn remote(self) -> ResourceVector {
        self.totals().0.remote
    }

    #[cfg(test)]
    pub(super) fn replacement_history(self) -> ResourceVector {
        self.totals().0.replacement_history
    }

    pub(super) fn peer(self, peer: PeerIndex) -> ResourceVector {
        self.entries.peer_resource(peer)
    }

    pub(super) fn accepted(self) -> AcceptedResources {
        self.totals().1
    }

    pub(super) fn accepted_fits(self, projected: AcceptedResources) -> bool {
        projected.fits(self.ledger.limits.accepted)
    }

    #[cfg(test)]
    pub(super) fn limits(self) -> ResourceLimits {
        self.ledger.limits
    }
}

pub(super) struct ResourcePlan {
    shards: ShardResourcePlan,
    capacity: ResourceCapacityReservation,
}

pub(super) struct ResourceBatchPlan {
    shards: ShardResourcePlan,
    capacity: ResourceCapacityReservation,
}

/// Resource transition sealed by the only owner-to-Nowhere compiler. Shared
/// Apply may rebase its per-shard subtraction on the exact current owners;
/// exclusive Apply may consume the already-compiled absolute shard targets.
/// No insertion/replacement plan can construct this carrier.
pub(super) struct OwnerRemovalResourcePlan {
    plan: ResourceBatchPlan,
    owners: Vec<(RawTxHash, ChargeRecord)>,
}

pub(super) struct ResourceCapacityCommit(ResourceCapacityReservation);

#[must_use = "a prepared capacity transition must finish after its matching owner mutation"]
pub(super) struct ResourceCapacityCommitPermit {
    bank: Arc<ResourceCapacityBank>,
    positive: Option<ResourceCapacityDelta>,
    release: Option<ResourceCapacityDelta>,
}

#[must_use = "post-owner capacity health must reach the commit finalizer or supervision"]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResourceCommitHealth {
    Healthy,
    Faulted,
}

impl ResourceCapacityCommit {
    pub(super) fn commit(self) -> ResourceCommitHealth {
        self.0.commit()
    }

    /// Prove under the capacity-bank lock that the already-reserved positive
    /// and exact release form a total commit. The bank remains conservative
    /// until [`ResourceCapacityCommitPermit::finish`] after owner mutation;
    /// no global lock spans the physical shard work.
    pub(super) fn begin(self) -> Result<ResourceCapacityCommitPermit, ResourceCapacityBeginError> {
        self.0.begin_commit()
    }
}

impl ResourceCapacityCommitPermit {
    pub(super) fn finish(mut self) -> ResourceCommitHealth {
        let positive = self.positive.take().unwrap_or_default();
        let release = self.release.take().unwrap_or_default();
        if self.bank.finish_commit(positive, release) {
            ResourceCommitHealth::Healthy
        } else {
            ResourceCommitHealth::Faulted
        }
    }
}

impl Drop for ResourceCapacityCommitPermit {
    fn drop(&mut self) {
        let positive = self.positive.take();
        let release = self.release.take();
        if positive.is_some() || release.is_some() {
            // `begin_commit` already moved the positive charge into the
            // conservative committed total. An abandoned permit cannot prove
            // whether its owner cut started, so keep both that charge and its
            // deferred release stranded and fault the generation in one bank
            // cut. No sibling permit may mistake this for reusable capacity.
            self.bank.abandon_commit();
        }
    }
}

impl ResourcePlan {
    pub(super) fn releases_preaccepted_active_work(&self) -> bool {
        self.capacity.releases_preaccepted_active_work()
    }

    pub(super) fn shard_plan(&self) -> &ShardResourcePlan {
        &self.shards
    }

    pub(super) fn apply_shards(
        self,
        owners: &mut ShardedOwnerWriteCut<'_>,
    ) -> ResourceCapacityCommit {
        owners.apply_resource_plan(self.shards);
        ResourceCapacityCommit(self.capacity)
    }

    /// Reinterpret one already-planned owner transition as the one-member
    /// batch consumed by the shared Apply engine. Both carriers own the same
    /// shard and capacity reservations, so this conversion is allocation-free
    /// and cannot change resource policy.
    pub(super) fn into_batch(self) -> ResourceBatchPlan {
        ResourceBatchPlan {
            shards: self.shards,
            capacity: self.capacity,
        }
    }
}

impl ResourceBatchPlan {
    pub(super) fn seal_active_work_revision(
        &mut self,
        seal: ActiveWorkRevisionSeal,
    ) -> Result<(), ActiveWorkRevisionSealError> {
        self.capacity.seal_active_work_revision(seal)
    }

    pub(super) fn releases_preaccepted_active_work(&self) -> bool {
        self.capacity.releases_preaccepted_active_work()
    }

    pub(super) fn shard_plan(&self) -> &ShardResourcePlan {
        &self.shards
    }

    pub(super) fn apply_shards(
        self,
        owners: &mut ShardedOwnerWriteCut<'_>,
    ) -> ResourceCapacityCommit {
        owners.apply_resource_plan(self.shards);
        ResourceCapacityCommit(self.capacity)
    }

    pub(super) fn into_shared_commit_parts(self) -> (ShardResourcePlan, ResourceCapacityCommit) {
        (self.shards, ResourceCapacityCommit(self.capacity))
    }
}

impl OwnerRemovalResourcePlan {
    pub(super) fn releases_preaccepted_active_work(&self) -> bool {
        self.plan.releases_preaccepted_active_work()
    }

    pub(super) fn shard_plan(&self) -> &ShardResourcePlan {
        self.plan.shard_plan()
    }

    pub(super) fn into_exclusive_plan(self) -> ResourceBatchPlan {
        self.plan
    }

    pub(super) fn rebase_shared_removal(
        mut self,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
        hashes: &[RawTxHash],
    ) -> Result<(ShardResourcePlan, ResourceCapacityCommit), ResourceError> {
        if hashes.len() != self.owners.len()
            || hashes
                .iter()
                .zip(&self.owners)
                .any(|(hash, (expected_hash, expected_charge))| {
                    hash != expected_hash
                        || cut.owner(entries, hash).map(OwnedTx::charge_record)
                            != Some(*expected_charge)
                })
        {
            return Err(ResourceError::ExistingChargeMismatch);
        }
        cut.rebase_owner_removal_resource_plan(&mut self.plan.shards)?;
        Ok(self.plan.into_shared_commit_parts())
    }

    #[cfg(test)]
    pub(in crate::authority) fn swap_first_owner_witnesses_for_foundation(&mut self) -> bool {
        if self.owners.len() < 2 {
            return false;
        }
        self.owners.swap(0, 1);
        true
    }
}

#[derive(Debug, Default)]
struct ResourceCapacityBank {
    state: Mutex<ResourceCapacityState>,
    reservation_terminal: Arc<Notify>,
}

#[derive(Debug, Default)]
struct ResourceCapacityState {
    committed: ResourceCapacityDelta,
    reserved: ResourceCapacityDelta,
    in_flight_positive: ResourceCapacityDelta,
    in_flight_release: ResourceCapacityDelta,
    active_work_revision: ActiveWorkRevision,
    faulted: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct ResourceCapacityObservation {
    committed: ResourceCapacityDelta,
    reserved: ResourceCapacityDelta,
    in_flight_positive: ResourceCapacityDelta,
    in_flight_release: ResourceCapacityDelta,
    active_work_revision: ActiveWorkRevision,
    faulted: bool,
}

/// Exact identity of the capacity bank which observed a transient conflict.
///
/// Every `ResourceCapacityBank` instance owns one distinct terminal signal.
/// Authority replacement therefore changes this identity, while transitions
/// which deliberately retain the same bank safely retain its signal. Pointer
/// identity prevents an old-bank waiter from parking on behalf of a new bank;
/// it adds neither a second resource authority nor an independent counter.
#[derive(Clone)]
pub(super) struct ResourceCapacityWaitIdentity {
    terminal: Arc<Notify>,
}

impl std::fmt::Debug for ResourceCapacityWaitIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("ResourceCapacityWaitIdentity")
            .field(&Arc::as_ptr(&self.terminal))
            .finish()
    }
}

impl PartialEq for ResourceCapacityWaitIdentity {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.terminal, &other.terminal)
    }
}

impl Eq for ResourceCapacityWaitIdentity {}

impl ResourceCapacityWaitIdentity {
    pub(super) fn same_bank(&self, other: &Self) -> bool {
        self == other
    }

    pub(super) fn terminal_signal(&self) -> Arc<Notify> {
        Arc::clone(&self.terminal)
    }
}

impl ResourceCapacityObservation {
    #[cfg(test)]
    pub(in crate::authority) fn has_reserved_capacity_for_foundation(self) -> bool {
        self.reserved != ResourceCapacityDelta::default()
            || self.in_flight_positive != ResourceCapacityDelta::default()
            || self.in_flight_release != ResourceCapacityDelta::default()
    }

    pub(super) fn explains_limit(self, previous: Self, error: &ResourceError) -> bool {
        match error {
            ResourceError::PreAcceptedLimit => {
                self.committed.preaccepted != previous.committed.preaccepted
                    || self.reserved.preaccepted != previous.reserved.preaccepted
                    || self.in_flight_positive.preaccepted
                        != previous.in_flight_positive.preaccepted
                    || self.in_flight_release.preaccepted != previous.in_flight_release.preaccepted
                    || self.reserved.preaccepted != ResourceVector::default()
                    || self.in_flight_positive.preaccepted != ResourceVector::default()
                    || self.in_flight_release.preaccepted != ResourceVector::default()
            }
            ResourceError::RemoteLimit => {
                self.committed.remote != previous.committed.remote
                    || self.reserved.remote != previous.reserved.remote
                    || self.in_flight_positive.remote != previous.in_flight_positive.remote
                    || self.in_flight_release.remote != previous.in_flight_release.remote
                    || self.reserved.remote != ResourceVector::default()
                    || self.in_flight_positive.remote != ResourceVector::default()
                    || self.in_flight_release.remote != ResourceVector::default()
            }
            ResourceError::AcceptedLimit => {
                self.committed.accepted != previous.committed.accepted
                    || self.reserved.accepted != previous.reserved.accepted
                    || self.in_flight_positive.accepted != previous.in_flight_positive.accepted
                    || self.in_flight_release.accepted != previous.in_flight_release.accepted
                    || self.reserved.accepted != AcceptedResources::default()
                    || self.in_flight_positive.accepted != AcceptedResources::default()
                    || self.in_flight_release.accepted != AcceptedResources::default()
            }
            // The caller uses this only after the ordered owner/resource
            // projection has accepted the complete set transition and after
            // selected-owner freshness still holds.  A final peer failure can
            // therefore only come from an unrelated peer-row transition,
            // including an aggregate-zero redistribution which is invisible
            // to this bank-wide observation.
            ResourceError::PeerLimit(_) => true,
            ResourceError::ReplacementHistoryLimit
            | ResourceError::ComputeEnvelope
            | ResourceError::Allocation
            | ResourceError::Arithmetic
            | ResourceError::ExistingChargeMismatch
            | ResourceError::AttributionMismatch
            | ResourceError::CapacityBankFault
            | ResourceError::DuplicateChange => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ResourceCapacityDelta {
    preaccepted: ResourceVector,
    remote: ResourceVector,
    replacement_history: ResourceVector,
    accepted: AcceptedResources,
}

struct ResourceCapacityReservation {
    bank: Arc<ResourceCapacityBank>,
    positive: Option<ResourceCapacityDelta>,
    release: Option<ResourceCapacityDelta>,
    active_work_revision: Option<ActiveWorkRevisionSeal>,
}

#[cfg(test)]
#[must_use = "the fixture reservation must remain held until its explicit terminal"]
pub(in crate::authority) struct HeldResourceCapacityReservation {
    reservation: Option<ResourceCapacityReservation>,
}

#[cfg(test)]
impl HeldResourceCapacityReservation {
    pub(in crate::authority) fn release(mut self) {
        drop(self.reservation.take());
    }
}

impl ResourceVector {
    const fn exhausted() -> Self {
        Self {
            entries: usize::MAX,
            bytes: usize::MAX,
            edges: usize::MAX,
            active_work: usize::MAX,
            compute_bytes: usize::MAX,
            compute_edges: usize::MAX,
        }
    }

    fn split_transition(before: Self, after: Self) -> (Self, Self) {
        let positive = Self {
            entries: after.entries.saturating_sub(before.entries),
            bytes: after.bytes.saturating_sub(before.bytes),
            edges: after.edges.saturating_sub(before.edges),
            active_work: after.active_work.saturating_sub(before.active_work),
            compute_bytes: after.compute_bytes.saturating_sub(before.compute_bytes),
            compute_edges: after.compute_edges.saturating_sub(before.compute_edges),
        };
        let release = Self {
            entries: before.entries.saturating_sub(after.entries),
            bytes: before.bytes.saturating_sub(after.bytes),
            edges: before.edges.saturating_sub(after.edges),
            active_work: before.active_work.saturating_sub(after.active_work),
            compute_bytes: before.compute_bytes.saturating_sub(after.compute_bytes),
            compute_edges: before.compute_edges.saturating_sub(after.compute_edges),
        };
        (positive, release)
    }
}

impl AcceptedResources {
    const fn exhausted() -> Self {
        Self {
            entries: usize::MAX,
            serialized_bytes: usize::MAX,
            resident_bytes: usize::MAX,
            cycles: u64::MAX,
        }
    }

    fn split_transition(before: Self, after: Self) -> (Self, Self) {
        let positive = Self {
            entries: after.entries.saturating_sub(before.entries),
            serialized_bytes: after
                .serialized_bytes
                .saturating_sub(before.serialized_bytes),
            resident_bytes: after.resident_bytes.saturating_sub(before.resident_bytes),
            cycles: after.cycles.saturating_sub(before.cycles),
        };
        let release = Self {
            entries: before.entries.saturating_sub(after.entries),
            serialized_bytes: before
                .serialized_bytes
                .saturating_sub(after.serialized_bytes),
            resident_bytes: before.resident_bytes.saturating_sub(after.resident_bytes),
            cycles: before.cycles.saturating_sub(after.cycles),
        };
        (positive, release)
    }
}

impl ResourceCapacityDelta {
    fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            preaccepted: self.preaccepted.checked_add(other.preaccepted)?,
            remote: self.remote.checked_add(other.remote)?,
            replacement_history: self
                .replacement_history
                .checked_add(other.replacement_history)?,
            accepted: self.accepted.checked_add(other.accepted)?,
        })
    }

    fn checked_sub(self, other: Self) -> Option<Self> {
        Some(Self {
            preaccepted: self.preaccepted.checked_sub(other.preaccepted)?,
            remote: self.remote.checked_sub(other.remote)?,
            replacement_history: self
                .replacement_history
                .checked_sub(other.replacement_history)?,
            accepted: self.accepted.checked_sub(other.accepted)?,
        })
    }

    #[cfg(test)]
    fn fits(self, limits: ResourceLimits) -> bool {
        self.preaccepted.fits(limits.preaccepted)
            && self.remote.fits(limits.remote)
            && self.replacement_history.fits(limits.replacement_history)
            && self.accepted.fits(limits.accepted)
    }

    fn between(
        current_totals: ResourceTotals,
        current_accepted: AcceptedResources,
        preaccepted: ResourceVector,
        remote: ResourceVector,
        replacement_history: ResourceVector,
        accepted: AcceptedResources,
    ) -> (Self, Self) {
        let (positive_preaccepted, release_preaccepted) =
            ResourceVector::split_transition(current_totals.preaccepted, preaccepted);
        let (positive_remote, release_remote) =
            ResourceVector::split_transition(current_totals.remote, remote);
        let (positive_replacement, release_replacement) = ResourceVector::split_transition(
            current_totals.replacement_history,
            replacement_history,
        );
        let (positive_accepted, release_accepted) =
            AcceptedResources::split_transition(current_accepted, accepted);
        (
            Self {
                preaccepted: positive_preaccepted,
                remote: positive_remote,
                replacement_history: positive_replacement,
                accepted: positive_accepted,
            },
            Self {
                preaccepted: release_preaccepted,
                remote: release_remote,
                replacement_history: release_replacement,
                accepted: release_accepted,
            },
        )
    }
}

impl ResourceCapacityBank {
    fn reservation_terminal_signal(&self) -> Arc<Notify> {
        Arc::clone(&self.reservation_terminal)
    }

    fn wait_identity(&self) -> ResourceCapacityWaitIdentity {
        ResourceCapacityWaitIdentity {
            terminal: self.reservation_terminal_signal(),
        }
    }

    fn notify_reservation_terminal(&self) {
        // The coordinator enables a waiter from the current generation before
        // planning. Broadcasting to registered waiters coalesces concurrent
        // terminals without retaining an old-generation/stale permit that
        // could cause an ungrounded extra reclassification later.
        self.reservation_terminal.notify_waiters();
    }

    fn observation(&self) -> ResourceCapacityObservation {
        let state = self.state.lock();
        ResourceCapacityObservation {
            committed: state.committed,
            reserved: state.reserved,
            in_flight_positive: state.in_flight_positive,
            in_flight_release: state.in_flight_release,
            active_work_revision: state.active_work_revision,
            faulted: state.faulted,
        }
    }

    #[cfg(test)]
    fn active_work_revision(&self) -> Result<ActiveWorkRevision, ResourceError> {
        let observation = self.observation();
        if observation.faulted {
            Err(ResourceError::CapacityBankFault)
        } else {
            Ok(observation.active_work_revision)
        }
    }

    /// Return the capacity bank's conservative committed projection. While a
    /// begun permit is crossing its owner cut, its positive charge is already
    /// included and its release remains deferred. The value is therefore
    /// gross rather than owner-exact while any begun permit may exist, but it
    /// never exposes capacity that an unfinished owner transition might still
    /// consume.
    fn committed_projection(&self) -> Result<(ResourceTotals, AcceptedResources), ResourceError> {
        let state = self.state.lock();
        if state.faulted {
            return Err(ResourceError::CapacityBankFault);
        }
        Ok((
            ResourceTotals {
                preaccepted: state.committed.preaccepted,
                remote: state.committed.remote,
                replacement_history: state.committed.replacement_history,
            },
            state.committed.accepted,
        ))
    }

    /// Answer the canonical membership capacity question from the same bank
    /// that later owns the transition reservation. Begun positive commits are
    /// already charged and begun releases remain charged, so this probe is
    /// conservative without scanning or materializing all owner shards.
    fn accepted_transition_fits(
        &self,
        released: AcceptedResources,
        added: AcceptedResources,
        limit: AcceptedResources,
    ) -> Result<bool, ResourceError> {
        let state = self.state.lock();
        if state.faulted {
            return Err(ResourceError::CapacityBankFault);
        }
        Ok(state
            .committed
            .accepted
            .checked_sub(released)
            .and_then(|current| current.checked_add(added))
            .is_some_and(|projected| projected.fits(limit)))
    }

    fn mark_faulted(&self) {
        self.state.lock().faulted = true;
        self.notify_reservation_terminal();
    }

    fn reserve(
        self: &Arc<Self>,
        positive: ResourceCapacityDelta,
        release: ResourceCapacityDelta,
        limits: ResourceLimits,
    ) -> Result<ResourceCapacityReservation, ResourceError> {
        let mut state = self.state.lock();
        if state.faulted {
            return Err(ResourceError::CapacityBankFault);
        }
        let reserved = ResourceCapacityDelta {
            preaccepted: state
                .reserved
                .preaccepted
                .checked_add(positive.preaccepted)
                .ok_or(ResourceError::PreAcceptedLimit)?,
            remote: state
                .reserved
                .remote
                .checked_add(positive.remote)
                .ok_or(ResourceError::RemoteLimit)?,
            replacement_history: state
                .reserved
                .replacement_history
                .checked_add(positive.replacement_history)
                .ok_or(ResourceError::ReplacementHistoryLimit)?,
            accepted: state
                .reserved
                .accepted
                .checked_add(positive.accepted)
                .ok_or(ResourceError::AcceptedLimit)?,
        };
        let _occupied = ResourceCapacityDelta {
            preaccepted: state
                .committed
                .preaccepted
                .checked_add(reserved.preaccepted)
                .filter(|usage| usage.fits(limits.preaccepted))
                .ok_or(ResourceError::PreAcceptedLimit)?,
            remote: state
                .committed
                .remote
                .checked_add(reserved.remote)
                .filter(|usage| usage.fits(limits.remote))
                .ok_or(ResourceError::RemoteLimit)?,
            replacement_history: state
                .committed
                .replacement_history
                .checked_add(reserved.replacement_history)
                .filter(|usage| usage.fits(limits.replacement_history))
                .ok_or(ResourceError::ReplacementHistoryLimit)?,
            accepted: state
                .committed
                .accepted
                .checked_add(reserved.accepted)
                .filter(|usage| usage.fits(limits.accepted))
                .ok_or(ResourceError::AcceptedLimit)?,
        };
        state.reserved = reserved;
        drop(state);
        Ok(ResourceCapacityReservation {
            bank: Arc::clone(self),
            positive: Some(positive),
            release: Some(release),
            active_work_revision: None,
        })
    }

    fn reserve_direct_accepted(
        self: &Arc<Self>,
        positive: ResourceCapacityDelta,
        limits: ResourceLimits,
    ) -> Result<ResourceCapacityReservation, DirectAcceptedInsertionError> {
        if positive.preaccepted != ResourceVector::default()
            || positive.remote != ResourceVector::default()
            || positive.replacement_history != ResourceVector::default()
        {
            return Err(ResourceError::CapacityBankFault.into());
        }
        let mut state = self.state.lock();
        if state.faulted {
            return Err(ResourceError::CapacityBankFault.into());
        }
        let contention = state.reserved.accepted != AcceptedResources::default()
            || state.in_flight_positive.accepted != AcceptedResources::default()
            || state.in_flight_release.accepted != AcceptedResources::default();
        let Some(reserved) = state.reserved.accepted.checked_add(positive.accepted) else {
            return Err(if contention {
                DirectAcceptedInsertionError::Contended(self.wait_identity())
            } else {
                DirectAcceptedInsertionError::Resource(ResourceError::AcceptedLimit)
            });
        };
        if state
            .committed
            .accepted
            .checked_add(reserved)
            .is_none_or(|usage| !usage.fits(limits.accepted))
        {
            return Err(if contention {
                DirectAcceptedInsertionError::Contended(self.wait_identity())
            } else {
                DirectAcceptedInsertionError::Resource(ResourceError::AcceptedLimit)
            });
        }
        state.reserved.accepted = reserved;
        drop(state);
        Ok(ResourceCapacityReservation {
            bank: Arc::clone(self),
            positive: Some(positive),
            release: Some(ResourceCapacityDelta::default()),
            active_work_revision: None,
        })
    }

    fn begin_commit(
        self: &Arc<Self>,
        positive: ResourceCapacityDelta,
        release: ResourceCapacityDelta,
        active_work_revision: Option<ActiveWorkRevisionSeal>,
    ) -> Result<ResourceCapacityCommitPermit, ResourceCapacityBeginError> {
        let mut state = self.state.lock();
        if state.faulted {
            return Err(ResourceError::CapacityBankFault.into());
        }
        if active_work_revision.is_some_and(|seal| seal.expected != state.active_work_revision) {
            return Err(ResourceCapacityBeginError::StaleActiveWorkRevision);
        }
        let Some(reserved) = state.reserved.checked_sub(positive) else {
            state.faulted = true;
            return Err(ResourceError::CapacityBankFault.into());
        };
        let Some(committed) = state.committed.checked_add(positive) else {
            state.faulted = true;
            return Err(ResourceError::CapacityBankFault.into());
        };
        let Some(in_flight_release) = state.in_flight_release.checked_add(release) else {
            state.faulted = true;
            return Err(ResourceError::CapacityBankFault.into());
        };
        let Some(in_flight_positive) = state.in_flight_positive.checked_add(positive) else {
            state.faulted = true;
            return Err(ResourceError::CapacityBankFault.into());
        };
        if committed.checked_sub(in_flight_release).is_none() {
            state.faulted = true;
            return Err(ResourceError::CapacityBankFault.into());
        }
        state.reserved = reserved;
        state.committed = committed;
        state.in_flight_positive = in_flight_positive;
        state.in_flight_release = in_flight_release;
        if let Some(seal) = active_work_revision {
            state.active_work_revision = seal.target;
        }
        Ok(ResourceCapacityCommitPermit {
            bank: Arc::clone(self),
            positive: Some(positive),
            release: Some(release),
        })
    }

    fn finish_commit(
        &self,
        positive: ResourceCapacityDelta,
        release: ResourceCapacityDelta,
    ) -> bool {
        let mut state = self.state.lock();
        let healthy = !state.faulted;
        let Some(in_flight_positive) = state.in_flight_positive.checked_sub(positive) else {
            state.faulted = true;
            drop(state);
            self.notify_reservation_terminal();
            return false;
        };
        let Some(in_flight_release) = state.in_flight_release.checked_sub(release) else {
            state.faulted = true;
            drop(state);
            self.notify_reservation_terminal();
            return false;
        };
        let Some(committed) = state.committed.checked_sub(release) else {
            state.faulted = true;
            drop(state);
            self.notify_reservation_terminal();
            return false;
        };
        state.in_flight_positive = in_flight_positive;
        state.in_flight_release = in_flight_release;
        state.committed = committed;
        drop(state);
        self.notify_reservation_terminal();
        healthy
    }

    fn abandon_commit(&self) {
        self.state.lock().faulted = true;
        self.notify_reservation_terminal();
    }

    fn release_reservation(&self, positive: ResourceCapacityDelta) -> bool {
        let mut state = self.state.lock();
        if state.faulted {
            drop(state);
            self.notify_reservation_terminal();
            return false;
        }
        let Some(reserved) = state.reserved.checked_sub(positive) else {
            state.faulted = true;
            drop(state);
            self.notify_reservation_terminal();
            return false;
        };
        state.reserved = reserved;
        drop(state);
        self.notify_reservation_terminal();
        true
    }
}

impl ResourceCapacityReservation {
    fn seal_active_work_revision(
        &mut self,
        seal: ActiveWorkRevisionSeal,
    ) -> Result<(), ActiveWorkRevisionSealError> {
        if self.active_work_revision.is_some() {
            return Err(ActiveWorkRevisionSealError::AlreadySealed);
        }
        self.active_work_revision = Some(seal);
        Ok(())
    }

    fn releases_preaccepted_active_work(&self) -> bool {
        self.release
            .as_ref()
            .is_some_and(|release| release.preaccepted.active_work != 0)
    }

    fn commit(mut self) -> ResourceCommitHealth {
        let Some(positive) = self.positive.take() else {
            return ResourceCommitHealth::Healthy;
        };
        let release = self.release.take().unwrap_or_default();
        let active_work_revision = self.active_work_revision.take();
        let permit = self
            .bank
            .begin_commit(positive, release, active_work_revision);
        match permit {
            Ok(permit) => permit.finish(),
            Err(_) => {
                let _closed_or_faulted = self.bank.release_reservation(positive);
                ResourceCommitHealth::Faulted
            }
        }
    }

    fn begin_commit(mut self) -> Result<ResourceCapacityCommitPermit, ResourceCapacityBeginError> {
        let positive = self.positive.unwrap_or_default();
        let release = self.release.unwrap_or_default();
        let active_work_revision = self.active_work_revision;
        let permit = self
            .bank
            .begin_commit(positive, release, active_work_revision)?;
        self.positive = None;
        self.release = None;
        self.active_work_revision = None;
        Ok(permit)
    }
}

impl Drop for ResourceCapacityReservation {
    fn drop(&mut self) {
        if let Some(positive) = self.positive.take() {
            let _closed_or_faulted = self.bank.release_reservation(positive);
        }
    }
}

#[cfg(test)]
mod capacity_bank_tests {
    use super::*;

    fn limits() -> ResourceLimits {
        ResourceLimits {
            preaccepted: ResourceVector::new(1, 1, 1, 1)
                .with_compute_capacity(1, 1)
                .expect("small test capacity is representable"),
            remote: ResourceVector::new(1, 1, 1, 1)
                .with_compute_capacity(1, 1)
                .expect("small test capacity is representable"),
            per_peer: ResourceVector::new(1, 1, 1, 1)
                .with_compute_capacity(1, 1)
                .expect("small test capacity is representable"),
            replacement_history: ResourceVector::new(1, 1, 1, 0),
            accepted: AcceptedResources::new(1, 1, 1, 1),
            compute: ComputeLimits::new(1, 1, 1),
            residency: ResidencyPolicy::foundation(),
        }
    }

    fn one_accepted() -> ResourceCapacityDelta {
        ResourceCapacityDelta {
            accepted: AcceptedResources::new(1, 1, 1, 1),
            ..Default::default()
        }
    }

    fn one_accepted_release() -> ResourceCapacityDelta {
        one_accepted()
    }

    fn one_active_work() -> ResourceCapacityDelta {
        ResourceCapacityDelta {
            preaccepted: ResourceVector::new(0, 0, 0, 1)
                .with_compute_capacity(1, 1)
                .expect("one active-work charge is representable"),
            ..Default::default()
        }
    }

    fn two_accepted_limits() -> ResourceLimits {
        let mut limits = limits();
        limits.accepted = AcceptedResources::new(2, 2, 2, 2);
        limits
    }

    fn two_active_work_limits() -> ResourceLimits {
        let mut limits = limits();
        limits.preaccepted = ResourceVector::new(1, 1, 1, 2)
            .with_compute_capacity(2, 2)
            .expect("two active-work slots are representable");
        limits
    }

    fn commit_healthy(reservation: ResourceCapacityReservation) {
        assert_eq!(reservation.commit(), ResourceCommitHealth::Healthy);
    }

    #[test]
    fn final_peer_limit_is_contention_even_when_aggregate_observation_is_unchanged() {
        let observation = ResourceCapacityObservation::default();
        assert!(observation.explains_limit(
            observation,
            &ResourceError::PeerLimit(PeerIndex::from(7usize)),
        ));
    }

    #[tokio::test]
    async fn aggregate_zero_reservation_still_publishes_its_terminal() {
        let bank = Arc::new(ResourceCapacityBank::default());
        let observation = bank.observation();
        let signal = bank.wait_identity().terminal_signal();
        let terminal = signal.notified();
        tokio::pin!(terminal);
        let _ = terminal.as_mut().enable();

        let reservation = bank
            .reserve(Default::default(), Default::default(), limits())
            .expect("an aggregate-zero peer transition still owns one reservation lifetime");
        assert_eq!(bank.observation(), observation);
        drop(reservation);
        terminal.as_mut().await;
        assert_eq!(bank.observation(), observation);
    }

    #[test]
    fn committed_capacity_is_not_returned_as_if_it_were_only_reserved() {
        let bank = Arc::new(ResourceCapacityBank::default());
        commit_healthy(
            bank.reserve(one_accepted(), Default::default(), limits())
                .expect("first reservation fits"),
        );

        assert!(matches!(
            bank.reserve(one_accepted(), Default::default(), limits()),
            Err(ResourceError::AcceptedLimit)
        ));
    }

    #[test]
    fn dropped_plan_returns_only_its_outstanding_positive_reservation() {
        let bank = Arc::new(ResourceCapacityBank::default());
        drop(
            bank.reserve(one_accepted(), Default::default(), limits())
                .expect("outstanding reservation fits"),
        );

        commit_healthy(
            bank.reserve(one_accepted(), Default::default(), limits())
                .expect("dropped reservation was returned"),
        );
    }

    #[test]
    fn dropped_removal_plan_does_not_publish_uncommitted_capacity() {
        let bank = Arc::new(ResourceCapacityBank::default());
        commit_healthy(
            bank.reserve(one_accepted(), Default::default(), limits())
                .expect("initial committed use fits"),
        );
        drop(
            bank.reserve(Default::default(), one_accepted_release(), limits())
                .expect("removal release is carried without being published"),
        );

        assert!(matches!(
            bank.reserve(one_accepted(), Default::default(), limits()),
            Err(ResourceError::AcceptedLimit)
        ));
    }

    #[test]
    fn committed_removal_releases_capacity_after_the_semantic_commit() {
        let bank = Arc::new(ResourceCapacityBank::default());
        commit_healthy(
            bank.reserve(one_accepted(), Default::default(), limits())
                .expect("initial committed use fits"),
        );
        commit_healthy(
            bank.reserve(Default::default(), one_accepted_release(), limits())
                .expect("removal release is carried"),
        );

        commit_healthy(
            bank.reserve(one_accepted(), Default::default(), limits())
                .expect("committed removal made the capacity reusable"),
        );
    }

    #[test]
    fn prepared_release_remains_unavailable_until_the_owner_cut_finishes() {
        let bank = Arc::new(ResourceCapacityBank::default());
        commit_healthy(
            bank.reserve(one_accepted(), Default::default(), limits())
                .expect("initial committed use fits"),
        );

        let permit = bank
            .reserve(Default::default(), one_accepted_release(), limits())
            .expect("the exact release can be prepared")
            .begin_commit()
            .expect("capacity preparation is valid before owner mutation");
        assert_eq!(
            bank.committed_projection(),
            Ok((ResourceTotals::default(), one_accepted().accepted)),
            "a prepared release is not reusable before the owner cut finishes"
        );
        assert!(matches!(
            bank.reserve(one_accepted(), Default::default(), limits()),
            Err(ResourceError::AcceptedLimit)
        ));

        assert_eq!(permit.finish(), ResourceCommitHealth::Healthy);
        commit_healthy(
            bank.reserve(one_accepted(), Default::default(), limits())
                .expect("the finished owner cut releases the exact capacity"),
        );
    }

    #[test]
    fn abandoned_commit_permit_faults_the_capacity_generation() {
        let bank = Arc::new(ResourceCapacityBank::default());
        let permit = bank
            .reserve(one_accepted(), Default::default(), limits())
            .expect("one positive reservation fits")
            .begin_commit()
            .expect("the positive capacity can enter its conservative commit phase");
        assert_eq!(
            bank.committed_projection(),
            Ok((ResourceTotals::default(), one_accepted().accepted)),
            "a begun positive-only permit is conservatively charged before owner mutation"
        );
        drop(permit);
        assert_eq!(
            bank.committed_projection(),
            Err(ResourceError::CapacityBankFault)
        );
    }

    #[test]
    fn begun_sibling_finishes_exactly_after_other_permit_abandons() {
        let bank = Arc::new(ResourceCapacityBank::default());
        for _ in 0..2 {
            commit_healthy(
                bank.reserve(one_accepted(), Default::default(), two_accepted_limits())
                    .expect("both accepted owners fit the bounded fixture"),
            );
        }
        let abandoned = bank
            .reserve(
                ResourceCapacityDelta::default(),
                one_accepted_release(),
                two_accepted_limits(),
            )
            .expect("the first exact owner release reserves")
            .begin_commit()
            .expect("the first release enters its owner cut");
        let sibling = bank
            .reserve(
                ResourceCapacityDelta::default(),
                one_accepted_release(),
                two_accepted_limits(),
            )
            .expect("the sibling exact owner release reserves")
            .begin_commit()
            .expect("the sibling release enters its owner cut");

        drop(abandoned);
        assert_eq!(
            sibling.finish(),
            ResourceCommitHealth::Faulted,
            "the sibling must finish its already-proven release and report the absorbing fault"
        );
        {
            let state = bank.state.lock();
            assert!(state.faulted);
            assert_eq!(state.reserved, ResourceCapacityDelta::default());
            assert_eq!(state.committed, one_accepted());
            assert_eq!(state.in_flight_release, one_accepted_release());
        }
        assert_eq!(
            bank.reserve(
                ResourceCapacityDelta::default(),
                ResourceCapacityDelta::default(),
                two_accepted_limits(),
            )
            .err(),
            Some(ResourceError::CapacityBankFault)
        );
    }

    #[test]
    fn aggregate_begun_releases_cannot_double_spend_committed_capacity() {
        let bank = Arc::new(ResourceCapacityBank::default());
        commit_healthy(
            bank.reserve(one_accepted(), Default::default(), limits())
                .expect("one accepted owner fits"),
        );
        let first = bank
            .reserve(Default::default(), one_accepted_release(), limits())
            .expect("the first release plans")
            .begin_commit()
            .expect("the first release is covered by committed capacity");
        let second = bank
            .reserve(Default::default(), one_accepted_release(), limits())
            .expect("release intent itself consumes no positive capacity")
            .begin_commit();
        assert!(matches!(
            second,
            Err(ResourceCapacityBeginError::Capacity(
                ResourceError::CapacityBankFault
            ))
        ));
        assert_eq!(first.finish(), ResourceCommitHealth::Faulted);
        let state = bank.state.lock();
        assert!(state.faulted);
        assert_eq!(state.committed, ResourceCapacityDelta::default());
        assert_eq!(state.in_flight_release, ResourceCapacityDelta::default());
    }

    #[test]
    fn committed_projection_excludes_reservations_and_tracks_only_linear_commit() {
        let bank = Arc::new(ResourceCapacityBank::default());
        assert_eq!(
            bank.committed_projection(),
            Ok((ResourceTotals::default(), AcceptedResources::default()))
        );

        let insertion = bank
            .reserve(one_accepted(), ResourceCapacityDelta::default(), limits())
            .expect("one bounded insertion reserves capacity");
        assert_eq!(
            bank.committed_projection(),
            Ok((ResourceTotals::default(), AcceptedResources::default())),
            "an uncommitted reservation is never reported as owner state"
        );
        commit_healthy(insertion);
        assert_eq!(
            bank.committed_projection(),
            Ok((ResourceTotals::default(), one_accepted().accepted))
        );

        let removal = bank
            .reserve(
                ResourceCapacityDelta::default(),
                one_accepted_release(),
                limits(),
            )
            .expect("one bounded removal carries its release until commit");
        assert_eq!(
            bank.committed_projection(),
            Ok((ResourceTotals::default(), one_accepted().accepted)),
            "a planned release cannot become reusable before semantic commit"
        );
        commit_healthy(removal);
        assert_eq!(
            bank.committed_projection(),
            Ok((ResourceTotals::default(), AcceptedResources::default()))
        );
    }

    #[test]
    fn active_work_release_without_queue_change_invalidates_old_checkout_selection() {
        let bank = Arc::new(ResourceCapacityBank::default());
        let initial = bank
            .active_work_revision()
            .expect("the initial capacity generation is healthy");
        let acquired = initial.seal(EntryVersion(11), ActiveWorkOperation::Acquire);
        let mut incumbent = bank
            .reserve(
                one_active_work(),
                ResourceCapacityDelta::default(),
                two_active_work_limits(),
            )
            .expect("the incumbent active-work owner fits");
        incumbent
            .seal_active_work_revision(acquired)
            .expect("the incumbent publishes one transition");
        commit_healthy(incumbent);

        let selected = bank
            .active_work_revision()
            .expect("checkout captures the incumbent availability identity");
        assert_eq!(selected, acquired.target);
        let mut old_checkout = bank
            .reserve(
                one_active_work(),
                ResourceCapacityDelta::default(),
                two_active_work_limits(),
            )
            .expect("the old selection reserves the remaining slot");
        old_checkout
            .seal_active_work_revision(
                selected.seal(EntryVersion(13), ActiveWorkOperation::Acquire),
            )
            .expect("the checkout selection carries one revision seal");

        let released = selected.seal(EntryVersion(12), ActiveWorkOperation::Release);
        let mut release = bank
            .reserve(
                ResourceCapacityDelta::default(),
                one_active_work(),
                two_active_work_limits(),
            )
            .expect("the incumbent release plans without scheduler mutation");
        release
            .seal_active_work_revision(released)
            .expect("the release carries its target identity");
        let release_permit = release
            .begin_commit()
            .expect("the release installs its identity before owner mutation");
        assert_eq!(
            bank.active_work_revision(),
            Ok(released.target),
            "the target is visible from capacity begin, not delayed until finish"
        );

        assert!(matches!(
            old_checkout.begin_commit(),
            Err(ResourceCapacityBeginError::StaleActiveWorkRevision)
        ));
        assert_eq!(release_permit.finish(), ResourceCommitHealth::Healthy);
        assert_eq!(
            bank.active_work_revision(),
            Ok(released.target),
            "the failed stale begin cannot roll back the committed release identity"
        );
        let state = bank.state.lock();
        assert!(!state.faulted, "ordinary OCC staleness is not a bank fault");
        assert_eq!(state.reserved, ResourceCapacityDelta::default());
        assert_eq!(state.committed, ResourceCapacityDelta::default());
    }

    #[test]
    fn unrelated_non_active_resource_change_preserves_checkout_selection_revision() {
        let bank = Arc::new(ResourceCapacityBank::default());
        let selected = bank
            .active_work_revision()
            .expect("checkout captures the initial availability identity");
        let acquired = selected.seal(EntryVersion(21), ActiveWorkOperation::Acquire);
        let mut checkout = bank
            .reserve(
                one_active_work(),
                ResourceCapacityDelta::default(),
                limits(),
            )
            .expect("one checkout slot reserves");
        checkout
            .seal_active_work_revision(acquired)
            .expect("the checkout carries its expected and target identity");

        commit_healthy(
            bank.reserve(one_accepted(), ResourceCapacityDelta::default(), limits())
                .expect("the unrelated accepted charge reserves independently"),
        );
        assert_eq!(
            bank.active_work_revision(),
            Ok(selected),
            "non-active resource commits do not invalidate availability"
        );

        let permit = checkout
            .begin_commit()
            .expect("the unchanged availability identity admits checkout begin");
        assert_eq!(bank.active_work_revision(), Ok(acquired.target));
        assert_eq!(permit.finish(), ResourceCommitHealth::Healthy);
    }

    #[test]
    fn peer_limit_uses_the_same_aba_stable_target_as_final_apply_freshness() {
        let mut entries = ShardedOwnerMap::new(super::super::shard::AuthorityShardRouter::new());
        let peer = PeerIndex::from(7);
        let incumbent = RawTxHash(ckb_types::packed::Byte32::new([7; 32]));
        let candidate = RawTxHash(ckb_types::packed::Byte32::new([8; 32]));
        let high = ResourceVector::new(1, 8, 8, 0);
        let low = ResourceVector::new(1, 2, 2, 0);
        let added = ResourceVector::new(1, 4, 4, 0);
        let limit = ResourceVector::new(2, 10, 10, 0);
        let charge = |resources| {
            let record = ChargeRecord::PreAccepted {
                resources,
                residency_peer: Some(peer),
                compute_peer: None,
            };
            record
                .validate()
                .expect("the peer charge is reachable from one production owner");
            ChargeProjection::from_validated(Some(record)).expect("the finite peer charge is valid")
        };
        let empty = ChargeProjection::from_validated(None).expect("absence is valid");
        let high_charge = charge(high);
        let low_charge = charge(low);
        let added_charge = charge(added);

        let establish = entries
            .plan_resource_transitions(std::iter::once((&incumbent, empty, high_charge)))
            .expect("the high peer row plans");
        entries.apply_resource_plan(establish);
        let insertion = entries
            .plan_resource_transitions(std::iter::once((&candidate, empty, added_charge)))
            .expect("the candidate captures high expected and over-limit target");

        let lower = entries
            .plan_resource_transitions(std::iter::once((&incumbent, high_charge, low_charge)))
            .expect("the peer row temporarily lowers");
        entries.apply_resource_plan(lower);
        assert!(
            entries
                .peer_resource(peer)
                .checked_add(added)
                .is_some_and(|target| target.fits(limit)),
            "the second live read used by the rejected implementation would pass at the low ABA point"
        );

        let restore = entries
            .plan_resource_transitions(std::iter::once((&incumbent, low_charge, high_charge)))
            .expect("the peer row returns to the captured expected value");
        entries.apply_resource_plan(restore);
        let proposed = super::super::shard::ShardProposedCountPlan::default();
        let support = entries.owner_resource_write_support(
            std::iter::once(&candidate),
            &proposed,
            &insertion,
        );
        let cut = entries.write_cut(support);
        assert!(
            cut.resource_plan_is_fresh(&insertion),
            "the high-low-high ABA restores the exact final Apply prestate"
        );
        drop(cut);
        assert!(matches!(
            insertion.validate_peer_targets(limit),
            Err(ResourceError::PeerLimit(actual)) if actual == peer
        ));
    }
}

#[cfg(test)]
impl ResourcePlan {
    pub(in crate::authority) fn extend_shard_support(
        &self,
        support: &mut super::shard_support::AuthorityShardSupport,
        exclusive: &mut super::shard_support::ExclusiveSupport,
    ) {
        self.shards.extend_shard_support(support);
        let _ = exclusive;
    }
}

#[derive(Clone, Copy)]
pub(in crate::authority) struct ChargeProjection {
    pub(in crate::authority) preaccepted: Option<ResourceVector>,
    pub(in crate::authority) peer: Option<(PeerIndex, ResourceVector)>,
    pub(in crate::authority) replacement_history: Option<ResourceVector>,
    pub(in crate::authority) accepted: Option<AcceptedResources>,
}

impl ChargeProjection {
    pub(in crate::authority) fn from_validated(
        charge: Option<ChargeRecord>,
    ) -> Result<Self, ResourceError> {
        Ok(Self {
            preaccepted: charge.and_then(ChargeRecord::preaccepted),
            peer: charge
                .map(ChargeRecord::peer_preaccepted)
                .transpose()?
                .flatten(),
            replacement_history: charge.and_then(ChargeRecord::replacement_history),
            accepted: charge.and_then(ChargeRecord::accepted),
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::authority) struct ResourceTotals {
    pub(in crate::authority) preaccepted: ResourceVector,
    pub(in crate::authority) remote: ResourceVector,
    pub(in crate::authority) replacement_history: ResourceVector,
}

impl ResourceTotals {
    pub(in crate::authority) fn checked_remove(
        mut self,
        charge: ChargeProjection,
    ) -> Result<Self, ResourceError> {
        if let Some(resources) = charge.preaccepted {
            self.preaccepted = self
                .preaccepted
                .checked_sub(resources)
                .ok_or(ResourceError::Arithmetic)?;
        }
        if let Some((_, resources)) = charge.peer {
            self.remote = self
                .remote
                .checked_sub(resources)
                .ok_or(ResourceError::Arithmetic)?;
        }
        if let Some(resources) = charge.replacement_history {
            self.replacement_history = self
                .replacement_history
                .checked_sub(resources)
                .ok_or(ResourceError::Arithmetic)?;
        }
        Ok(self)
    }

    pub(in crate::authority) fn checked_add(
        mut self,
        charge: ChargeProjection,
    ) -> Result<Self, ResourceError> {
        if let Some(resources) = charge.preaccepted {
            self.preaccepted = self
                .preaccepted
                .checked_add(resources)
                .ok_or(ResourceError::Arithmetic)?;
        }
        if let Some((_, resources)) = charge.peer {
            self.remote = self
                .remote
                .checked_add(resources)
                .ok_or(ResourceError::Arithmetic)?;
        }
        if let Some(resources) = charge.replacement_history {
            self.replacement_history = self
                .replacement_history
                .checked_add(resources)
                .ok_or(ResourceError::Arithmetic)?;
        }
        Ok(self)
    }
}

fn checked_remove_accepted(
    current: AcceptedResources,
    removed: Option<AcceptedResources>,
) -> Result<AcceptedResources, ResourceError> {
    removed.map_or(Ok(current), |resources| {
        current
            .checked_sub(resources)
            .ok_or(ResourceError::Arithmetic)
    })
}

fn checked_add_accepted(
    current: AcceptedResources,
    added: Option<AcceptedResources>,
) -> Result<AcceptedResources, ResourceError> {
    added.map_or(Ok(current), |resources| {
        current
            .checked_add(resources)
            .ok_or(ResourceError::Arithmetic)
    })
}

/// Stack-owned aggregate projection for a canonical ordered batch.
///
/// [`ResourceBatchPlan`] deliberately validates a set transition: every old
/// charge is removed before any new charge is installed. Retained ingress has
/// a stronger rule because each item must observe the resource result of every
/// earlier item in controller order. This projection evaluates that ordered
/// fold without cloning the owner map or mutating the authoritative ledger;
/// the final base-to-final changes are still compiled by [`ResourceLedger::plan_batch`].
pub(super) struct OrderedResourceProjection {
    preaccepted: ResourceVector,
    remote: ResourceVector,
    peers: HashMap<PeerIndex, ResourceVector>,
    replacement_history: ResourceVector,
    accepted: AcceptedResources,
    limits: ResourceLimits,
    capacity_observation: ResourceCapacityObservation,
}

impl ResourceLedger {
    pub(super) fn capacity_wait_identity(&self) -> ResourceCapacityWaitIdentity {
        self.capacity.wait_identity()
    }

    pub(super) fn membership_accepted_transition_fits(
        &self,
        released: AcceptedResources,
        added: AcceptedResources,
    ) -> Result<bool, ResourceError> {
        self.capacity
            .accepted_transition_fits(released, added, self.limits.accepted)
    }

    pub(super) fn capacity_observation(&self) -> ResourceCapacityObservation {
        self.capacity.observation()
    }

    #[cfg(test)]
    pub(in crate::authority) fn hold_positive_compute_reservation_for_foundation(
        &self,
    ) -> Result<HeldResourceCapacityReservation, ResourceError> {
        let active = ResourceVector::new(0, 0, 0, self.limits.preaccepted.active_work)
            .with_compute_capacity(
                self.limits.compute.resolved_total_retained_bytes,
                self.limits.compute.expanded_edges,
            )
            .ok_or(ResourceError::Arithmetic)?;
        let reservation = self.capacity.reserve(
            ResourceCapacityDelta {
                preaccepted: active,
                ..ResourceCapacityDelta::default()
            },
            ResourceCapacityDelta::default(),
            self.limits,
        )?;
        Ok(HeldResourceCapacityReservation {
            reservation: Some(reservation),
        })
    }

    #[cfg(test)]
    pub(in crate::authority) fn hold_positive_accepted_reservation_for_foundation(
        &self,
    ) -> Result<HeldResourceCapacityReservation, ResourceError> {
        let reservation = self.capacity.reserve(
            ResourceCapacityDelta {
                accepted: self.limits.accepted,
                ..ResourceCapacityDelta::default()
            },
            ResourceCapacityDelta::default(),
            self.limits,
        )?;
        Ok(HeldResourceCapacityReservation {
            reservation: Some(reservation),
        })
    }

    pub(super) fn plan_removal_batch(
        &self,
        entries: &ShardedOwnerMap,
        changes: Vec<(RawTxHash, ChargeRecord)>,
        shards: ShardResourcePlan,
    ) -> Result<OwnerRemovalResourcePlan, ResourceError> {
        let mut release = ResourceCapacityDelta::default();
        for (key, expected) in &changes {
            expected.validate()?;
            if entries.get(key).as_deref().map(OwnedTx::charge_record) != Some(*expected) {
                return Err(ResourceError::ExistingChargeMismatch);
            }
            let charge = ChargeProjection::from_validated(Some(*expected))?;
            if let Some(resources) = charge.preaccepted {
                release.preaccepted = release
                    .preaccepted
                    .checked_add(resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
            if let Some((_, resources)) = charge.peer {
                release.remote = release
                    .remote
                    .checked_add(resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
            if let Some(resources) = charge.replacement_history {
                release.replacement_history = release
                    .replacement_history
                    .checked_add(resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
            if let Some(resources) = charge.accepted {
                release.accepted = release
                    .accepted
                    .checked_add(resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
        }
        let capacity =
            self.capacity
                .reserve(ResourceCapacityDelta::default(), release, self.limits)?;
        Ok(OwnerRemovalResourcePlan {
            plan: ResourceBatchPlan { shards, capacity },
            owners: changes,
        })
    }

    pub(super) fn new(limits: ResourceLimits) -> Self {
        Self {
            limits,
            capacity: Arc::new(ResourceCapacityBank::default()),
        }
    }

    pub(in crate::authority) fn read<'state>(
        &'state self,
        entries: &'state ShardedOwnerMap,
    ) -> ResourceRead<'state> {
        ResourceRead {
            entries,
            ledger: self,
        }
    }

    pub(in crate::authority) fn coherent_totals(
        &self,
        owners: &ShardedOwnerReadCut<'_>,
    ) -> (ResourceTotals, AcceptedResources) {
        owners.resource_totals().unwrap_or_else(|| {
            self.capacity.mark_faulted();
            (
                ResourceTotals {
                    preaccepted: ResourceVector::exhausted(),
                    remote: ResourceVector::exhausted(),
                    replacement_history: ResourceVector::exhausted(),
                },
                AcceptedResources::exhausted(),
            )
        })
    }

    pub(super) fn ordered_projection(
        &self,
        entries: &ShardedOwnerMap,
        maximum_peers: usize,
    ) -> Result<OrderedResourceProjection, ResourceError> {
        let capacity_observation = self.capacity_observation();
        if capacity_observation.faulted {
            return Err(ResourceError::CapacityBankFault);
        }
        let current = self.read(entries);
        let (totals, accepted) = current.totals();
        if self.capacity_observation().faulted {
            return Err(ResourceError::CapacityBankFault);
        }
        self.ordered_projection_from(totals, accepted, maximum_peers, capacity_observation)
    }

    pub(super) fn ordered_committed_projection(
        &self,
        maximum_peers: usize,
    ) -> Result<OrderedResourceProjection, ResourceError> {
        // Only closed shared-ingress compilers use this cut. The capacity
        // bank owns the exact committed aggregate; routed owner and per-peer
        // rows remain point observations and are revalidated by the final
        // transition receipt. No population-wide shard scan is needed.
        let capacity_observation = self.capacity_observation();
        if capacity_observation.faulted {
            return Err(ResourceError::CapacityBankFault);
        }
        let (totals, accepted) = self.capacity.committed_projection()?;
        self.ordered_projection_from(totals, accepted, maximum_peers, capacity_observation)
    }

    fn ordered_projection_from(
        &self,
        totals: ResourceTotals,
        accepted: AcceptedResources,
        maximum_peers: usize,
        capacity_observation: ResourceCapacityObservation,
    ) -> Result<OrderedResourceProjection, ResourceError> {
        let mut peers = HashMap::new();
        peers
            .try_reserve(maximum_peers)
            .map_err(|_| ResourceError::Allocation)?;
        Ok(OrderedResourceProjection {
            preaccepted: totals.preaccepted,
            remote: totals.remote,
            peers,
            replacement_history: totals.replacement_history,
            accepted,
            limits: self.limits,
            capacity_observation,
        })
    }

    pub(super) fn compute_grant(
        &self,
        entry: &PreAcceptedEntry,
        permit: WorkPermit,
    ) -> ComputeGrant {
        self.limits
            .compute
            .grant_for(permit, entry, self.limits.residency)
    }

    pub(super) fn admission_charge(
        &self,
        payload_bytes: usize,
        encoded_edges: usize,
    ) -> Result<ResourceVector, ResourceError> {
        self.retained_charge(payload_bytes, encoded_edges, payload_bytes, encoded_edges)
    }

    pub(super) fn charge_admission(
        &self,
        admission: ValidatedAdmission,
    ) -> Result<ChargedAdmission, ResourceError> {
        let charge = self.admission_charge(admission.payload_bytes, admission.encoded_edges)?;
        self.validate_admission(charge)?;
        Ok(ChargedAdmission { admission, charge })
    }

    pub(super) fn retained_charge(
        &self,
        payload_bytes: usize,
        encoded_edges: usize,
        retained_payload_bytes: usize,
        retained_edges: usize,
    ) -> Result<ResourceVector, ResourceError> {
        self.limits
            .residency
            .charge(
                payload_bytes,
                encoded_edges,
                retained_payload_bytes,
                retained_edges,
            )
            .ok_or(ResourceError::Arithmetic)
    }

    pub(super) fn retained_entry_charge(
        &self,
        entry: &PreAcceptedEntry,
        retained_payload_bytes: usize,
        retained_edges: usize,
    ) -> Result<ResourceVector, ResourceError> {
        self.retained_charge(
            entry.basis.payload_bytes(),
            entry.basis.encoded_edges(),
            retained_payload_bytes,
            retained_edges,
        )
    }

    pub(super) fn replacement_history_charge(
        &self,
        tx: &TransactionView,
        retained_edges: usize,
    ) -> Result<ReplacementHistoryCharge, ResourceError> {
        let payload_bytes = tx.data().total_size();
        let encoded_edges = tx
            .inputs()
            .len()
            .checked_add(tx.cell_deps().len())
            .and_then(|count| count.checked_add(tx.header_deps().len()))
            .ok_or(ResourceError::Arithmetic)?;
        let recovery = self.admission_charge(payload_bytes, encoded_edges)?;
        let retained =
            self.retained_charge(payload_bytes, encoded_edges, payload_bytes, retained_edges)?;
        Ok(ReplacementHistoryCharge {
            payload_bytes,
            encoded_edges,
            recovery,
            retained,
        })
    }

    pub(super) fn limits(&self) -> ResourceLimits {
        self.limits
    }

    pub(super) fn validate_admission(
        &self,
        resources: ResourceVector,
    ) -> Result<(), ResourceError> {
        if self.limits.compute.admits(resources) {
            Ok(())
        } else {
            Err(ResourceError::ComputeEnvelope)
        }
    }

    pub(super) fn plan_replace<F>(
        &self,
        entries: &ShardedOwnerMap,
        expected: Option<ChargeRecord>,
        after: Option<ChargeRecord>,
        shards: ShardResourcePlan,
        current_charge: F,
    ) -> Result<ResourcePlan, ResourceError>
    where
        F: FnOnce() -> Option<ChargeRecord>,
    {
        expected.map(ChargeRecord::validate).transpose()?;
        after.map(ChargeRecord::validate).transpose()?;
        if current_charge() != expected {
            return Err(ResourceError::ExistingChargeMismatch);
        }

        let old_charge = ChargeProjection::from_validated(expected)?;
        let new_charge = ChargeProjection::from_validated(after)?;
        let current = self.read(entries);
        let (current_totals, current_accepted) = current.totals();
        let ResourceTotals {
            preaccepted,
            remote,
            replacement_history,
        } = current_totals
            .checked_remove(old_charge)?
            .checked_add(new_charge)?;
        if !preaccepted.fits(self.limits.preaccepted) {
            return Err(ResourceError::PreAcceptedLimit);
        }
        if !remote.fits(self.limits.remote) {
            return Err(ResourceError::RemoteLimit);
        }
        if !replacement_history.fits(self.limits.replacement_history) {
            return Err(ResourceError::ReplacementHistoryLimit);
        }

        let old_peer_charge = old_charge.peer;
        let new_peer_charge = new_charge.peer;
        let old_peer = old_peer_charge.map(|(peer, _)| peer);
        let new_peer = new_peer_charge.map(|(peer, _)| peer);
        let project_peer = |peer: PeerIndex| {
            let mut usage = current.peer(peer);
            if old_peer == Some(peer) {
                let resources = old_peer_charge
                    .map(|(_, resources)| resources)
                    .ok_or(ResourceError::Arithmetic)?;
                usage = usage
                    .checked_sub(resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
            if new_peer == Some(peer) {
                let resources = new_peer_charge
                    .map(|(_, resources)| resources)
                    .ok_or(ResourceError::Arithmetic)?;
                usage = usage
                    .checked_add(resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
            if !usage.fits(self.limits.per_peer) {
                return Err(ResourceError::PeerLimit(peer));
            }
            Ok(usage)
        };
        if let Some(peer) = old_peer {
            let _validated_peer_target = project_peer(peer)?;
        }
        if let Some(peer) = new_peer.filter(|peer| Some(*peer) != old_peer) {
            let _validated_peer_target = project_peer(peer)?;
        }

        let accepted = checked_add_accepted(
            checked_remove_accepted(current_accepted, old_charge.accepted)?,
            new_charge.accepted,
        )?;
        if !accepted.fits(self.limits.accepted) {
            return Err(ResourceError::AcceptedLimit);
        }

        let (positive, release) = ResourceCapacityDelta::between(
            current_totals,
            current_accepted,
            preaccepted,
            remote,
            replacement_history,
            accepted,
        );
        let capacity = self.capacity.reserve(positive, release, self.limits)?;
        Ok(ResourcePlan { shards, capacity })
    }

    pub(super) fn plan_batch<F>(
        &self,
        entries: &ShardedOwnerMap,
        changes: Vec<(RawTxHash, Option<ChargeRecord>, Option<ChargeRecord>)>,
        shards: ShardResourcePlan,
        mut current_charge: F,
    ) -> Result<ResourceBatchPlan, ResourceError>
    where
        F: FnMut(&RawTxHash) -> Option<ChargeRecord>,
    {
        let mut keys = HashSet::new();
        keys.try_reserve(changes.len())
            .map_err(|_| ResourceError::Allocation)?;
        let mut peer_updates = HashMap::new();
        let peer_capacity = changes
            .len()
            .checked_mul(2)
            .ok_or(ResourceError::Arithmetic)?;
        peer_updates
            .try_reserve(peer_capacity)
            .map_err(|_| ResourceError::Allocation)?;
        let current = self.read(entries);
        let (current_totals, current_accepted) = current.totals();
        let mut totals = current_totals;
        let mut accepted = current_accepted;

        for (key, expected, after) in &changes {
            expected.map(ChargeRecord::validate).transpose()?;
            after.map(ChargeRecord::validate).transpose()?;
            if !keys.insert(key.clone()) {
                return Err(ResourceError::DuplicateChange);
            }
            if current_charge(key) != *expected {
                return Err(ResourceError::ExistingChargeMismatch);
            }
        }

        // A batch is a set transition, not a caller-ordered sequence. Remove
        // every old charge before adding any new charge so a valid net change
        // cannot overflow only because its freeing member appeared later in
        // the input vector.
        for (_, expected, _) in &changes {
            let charge = ChargeProjection::from_validated(*expected)?;
            totals = totals.checked_remove(charge)?;
            if let Some((peer, resources)) = charge.peer {
                let usage = peer_updates
                    .entry(peer)
                    .or_insert_with(|| current.peer(peer));
                *usage = usage
                    .checked_sub(resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
            accepted = checked_remove_accepted(accepted, charge.accepted)?;
        }
        for (_, _, after) in &changes {
            let charge = ChargeProjection::from_validated(*after)?;
            totals = totals.checked_add(charge)?;
            if let Some((peer, resources)) = charge.peer {
                let usage = peer_updates
                    .entry(peer)
                    .or_insert_with(|| current.peer(peer));
                *usage = usage
                    .checked_add(resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
            accepted = checked_add_accepted(accepted, charge.accepted)?;
        }

        if !totals.preaccepted.fits(self.limits.preaccepted) {
            return Err(ResourceError::PreAcceptedLimit);
        }
        if !totals.remote.fits(self.limits.remote) {
            return Err(ResourceError::RemoteLimit);
        }
        if let Some(peer) = peer_updates
            .iter()
            .filter_map(|(peer, usage)| (!usage.fits(self.limits.per_peer)).then_some(*peer))
            .min()
        {
            return Err(ResourceError::PeerLimit(peer));
        }
        if !totals
            .replacement_history
            .fits(self.limits.replacement_history)
        {
            return Err(ResourceError::ReplacementHistoryLimit);
        }
        if !accepted.fits(self.limits.accepted) {
            return Err(ResourceError::AcceptedLimit);
        }

        let (positive, release) = ResourceCapacityDelta::between(
            current_totals,
            current_accepted,
            totals.preaccepted,
            totals.remote,
            totals.replacement_history,
            accepted,
        );
        let capacity = self.capacity.reserve(positive, release, self.limits)?;
        Ok(ResourceBatchPlan { shards, capacity })
    }

    /// Reserve the exact aggregate transition of one closed shared owner
    /// batch from its point-validated before/after charges alone.
    ///
    /// The capacity bank owns aggregate admission. The routed shard plan owns
    /// per-key and per-peer state. Summing the selected before and after rows
    /// therefore yields the exact aggregate delta without reading every owner
    /// shard, while the final owner cut revalidates every selected premise.
    pub(super) fn plan_shared_transition_batch(
        &self,
        entries: &ShardedOwnerMap,
        changes: Vec<(RawTxHash, Option<ChargeRecord>, Option<ChargeRecord>)>,
        shards: ShardResourcePlan,
    ) -> Result<ResourceBatchPlan, ResourceError> {
        let mut keys = HashSet::new();
        keys.try_reserve(changes.len())
            .map_err(|_| ResourceError::Allocation)?;
        let mut before = ResourceCapacityDelta::default();
        let mut after = ResourceCapacityDelta::default();

        for (key, expected, replacement) in changes {
            expected.map(ChargeRecord::validate).transpose()?;
            replacement.map(ChargeRecord::validate).transpose()?;
            if !keys.insert(key.clone()) {
                return Err(ResourceError::DuplicateChange);
            }
            if entries.get(&key).as_deref().map(OwnedTx::charge_record) != expected {
                return Err(ResourceError::ExistingChargeMismatch);
            }
            before = before
                .checked_add(Self::charge_capacity_delta(expected)?)
                .ok_or(ResourceError::Arithmetic)?;
            after = after
                .checked_add(Self::charge_capacity_delta(replacement)?)
                .ok_or(ResourceError::Arithmetic)?;
        }

        let (positive_preaccepted, release_preaccepted) =
            ResourceVector::split_transition(before.preaccepted, after.preaccepted);
        let (positive_remote, release_remote) =
            ResourceVector::split_transition(before.remote, after.remote);
        let (positive_replacement, release_replacement) =
            ResourceVector::split_transition(before.replacement_history, after.replacement_history);
        let (positive_accepted, release_accepted) =
            AcceptedResources::split_transition(before.accepted, after.accepted);
        let positive = ResourceCapacityDelta {
            preaccepted: positive_preaccepted,
            remote: positive_remote,
            replacement_history: positive_replacement,
            accepted: positive_accepted,
        };
        let release = ResourceCapacityDelta {
            preaccepted: release_preaccepted,
            remote: release_remote,
            replacement_history: release_replacement,
            accepted: release_accepted,
        };

        shards.validate_peer_targets(self.limits.per_peer)?;
        let capacity = self.capacity.reserve(positive, release, self.limits)?;
        Ok(ResourceBatchPlan { shards, capacity })
    }

    fn charge_capacity_delta(
        charge: Option<ChargeRecord>,
    ) -> Result<ResourceCapacityDelta, ResourceError> {
        let charge = ChargeProjection::from_validated(charge)?;
        Ok(ResourceCapacityDelta {
            preaccepted: charge.preaccepted.unwrap_or_default(),
            remote: charge
                .peer
                .map(|(_, resources)| resources)
                .unwrap_or_default(),
            replacement_history: charge.replacement_history.unwrap_or_default(),
            accepted: charge.accepted.unwrap_or_default(),
        })
    }

    pub(super) fn plan_direct_accepted_insertion_batch(
        &self,
        entries: &ShardedOwnerMap,
        changes: Vec<(RawTxHash, ChargeRecord)>,
        shards: ShardResourcePlan,
    ) -> Result<ResourceBatchPlan, DirectAcceptedInsertionError> {
        let positive = self.insertion_positive(entries, changes, &shards)?;
        let capacity = self
            .capacity
            .reserve_direct_accepted(positive, self.limits)?;
        Ok(ResourceBatchPlan { shards, capacity })
    }

    fn insertion_positive(
        &self,
        entries: &ShardedOwnerMap,
        changes: Vec<(RawTxHash, ChargeRecord)>,
        shards: &ShardResourcePlan,
    ) -> Result<ResourceCapacityDelta, ResourceError> {
        let mut keys = HashSet::new();
        keys.try_reserve(changes.len())
            .map_err(|_| ResourceError::Allocation)?;
        let mut positive = ResourceCapacityDelta::default();

        for (key, after) in changes {
            after.validate()?;
            if !keys.insert(key.clone()) {
                return Err(ResourceError::DuplicateChange);
            }
            if entries.get(&key).is_some() {
                return Err(ResourceError::ExistingChargeMismatch);
            }
            let charge = ChargeProjection::from_validated(Some(after))?;
            if let Some(resources) = charge.preaccepted {
                positive.preaccepted = positive
                    .preaccepted
                    .checked_add(resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
            if let Some((_peer, resources)) = charge.peer {
                positive.remote = positive
                    .remote
                    .checked_add(resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
            if let Some(resources) = charge.replacement_history {
                positive.replacement_history = positive
                    .replacement_history
                    .checked_add(resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
            if let Some(resources) = charge.accepted {
                positive.accepted = positive
                    .accepted
                    .checked_add(resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
        }

        shards.validate_peer_targets(self.limits.per_peer)?;
        Ok(positive)
    }
}

fn active_work_availability(
    preaccepted: ResourceVector,
    remote: ResourceVector,
    peer: Option<(PeerIndex, ResourceVector)>,
    limits: ResourceLimits,
) -> Result<ActiveWorkAvailability, ResourceError> {
    let available = |used: usize, limit: usize| match used.cmp(&limit) {
        std::cmp::Ordering::Less => Ok(true),
        std::cmp::Ordering::Equal => Ok(false),
        std::cmp::Ordering::Greater => Err(ResourceError::Arithmetic),
    };
    if !available(preaccepted.active_work, limits.preaccepted.active_work)? {
        return Ok(ActiveWorkAvailability::PreAcceptedExhausted);
    }
    let Some((peer, usage)) = peer else {
        return Ok(ActiveWorkAvailability::Available);
    };
    if !available(remote.active_work, limits.remote.active_work)? {
        return Ok(ActiveWorkAvailability::RemoteExhausted);
    }
    if !available(usage.active_work, limits.per_peer.active_work)? {
        return Ok(ActiveWorkAvailability::PeerExhausted(peer));
    }
    Ok(ActiveWorkAvailability::Available)
}

impl OrderedResourceProjection {
    pub(super) const fn active_work_revision(&self) -> ActiveWorkRevision {
        self.capacity_observation.active_work_revision
    }

    pub(super) const fn capacity_observation(&self) -> ResourceCapacityObservation {
        self.capacity_observation
    }

    pub(super) fn active_work_availability(
        &self,
        current: ResourceRead<'_>,
        attribution: ComputeAttribution,
    ) -> Result<ActiveWorkAvailability, ResourceError> {
        active_work_availability(
            self.preaccepted,
            self.remote,
            attribution.peer().map(|peer| {
                (
                    peer,
                    self.peers
                        .get(&peer)
                        .copied()
                        .unwrap_or_else(|| current.peer(peer)),
                )
            }),
            self.limits,
        )
    }

    /// Evaluate one canonical owner replacement against the virtual result of
    /// all prior replacements. The caller owns raw-hash identity; this method
    /// owns only aggregate accounting and therefore cannot become a second
    /// lifecycle authority.
    pub(super) fn replace(
        &mut self,
        current: ResourceRead<'_>,
        expected: Option<ChargeRecord>,
        after: Option<ChargeRecord>,
    ) -> Result<(), ResourceError> {
        self.replace_with_peer(expected, after, |peer| current.peer(peer))
    }

    pub(super) fn replace_with_peer(
        &mut self,
        expected: Option<ChargeRecord>,
        after: Option<ChargeRecord>,
        mut base_peer: impl FnMut(PeerIndex) -> ResourceVector,
    ) -> Result<(), ResourceError> {
        expected.map(ChargeRecord::validate).transpose()?;
        after.map(ChargeRecord::validate).transpose()?;

        let old_charge = ChargeProjection::from_validated(expected)?;
        let new_charge = ChargeProjection::from_validated(after)?;
        let totals = ResourceTotals {
            preaccepted: self.preaccepted,
            remote: self.remote,
            replacement_history: self.replacement_history,
        }
        .checked_remove(old_charge)?
        .checked_add(new_charge)?;
        let accepted = checked_add_accepted(
            checked_remove_accepted(self.accepted, old_charge.accepted)?,
            new_charge.accepted,
        )?;
        let old_peer = old_charge.peer;
        let new_peer = new_charge.peer;

        let mut current_peer = |peer: PeerIndex| {
            self.peers
                .get(&peer)
                .copied()
                .unwrap_or_else(|| base_peer(peer))
        };
        let old_peer_after = old_peer
            .map(|(peer, resources)| {
                current_peer(peer)
                    .checked_sub(resources)
                    .map(|usage| (peer, usage))
                    .ok_or(ResourceError::Arithmetic)
            })
            .transpose()?;
        let new_peer_after = new_peer
            .map(|(peer, resources)| {
                let usage = old_peer_after
                    .filter(|(old_peer, _)| *old_peer == peer)
                    .map_or_else(|| current_peer(peer), |(_, usage)| usage);
                usage
                    .checked_add(resources)
                    .map(|usage| (peer, usage))
                    .ok_or(ResourceError::Arithmetic)
            })
            .transpose()?;
        if !totals.preaccepted.fits(self.limits.preaccepted) {
            return Err(ResourceError::PreAcceptedLimit);
        }
        if !totals.remote.fits(self.limits.remote) {
            return Err(ResourceError::RemoteLimit);
        }
        if let Some(peer) = old_peer_after
            .into_iter()
            .chain(new_peer_after)
            .filter_map(|(peer, usage)| (!usage.fits(self.limits.per_peer)).then_some(peer))
            .min()
        {
            return Err(ResourceError::PeerLimit(peer));
        }
        if !totals
            .replacement_history
            .fits(self.limits.replacement_history)
        {
            return Err(ResourceError::ReplacementHistoryLimit);
        }
        if !accepted.fits(self.limits.accepted) {
            return Err(ResourceError::AcceptedLimit);
        }

        self.preaccepted = totals.preaccepted;
        self.remote = totals.remote;
        self.replacement_history = totals.replacement_history;
        self.accepted = accepted;
        if let Some((peer, usage)) = old_peer_after {
            self.peers.insert(peer, usage);
        }
        if let Some((peer, usage)) = new_peer_after {
            self.peers.insert(peer, usage);
        }
        Ok(())
    }

    /// Evaluate one ordered component as an atomic set transition.
    ///
    /// A leaf-RBF component replaces both the Ready candidate and its
    /// Accepted victim. Removing either owner first can create a false
    /// transient limit (Accepted in candidate-first order, replacement
    /// history in victim-first order), although the canonical single-member
    /// compiler removes both old charges before installing either new one.
    /// This scratch fold preserves that exact rule for every strongest-first
    /// component while the final authority transition remains owned by
    /// [`ResourceLedger::plan_batch`]. An error leaves this projection
    /// unchanged, so optional history can be terminalized and retried once.
    pub(super) fn replace_set(
        &mut self,
        current: ResourceRead<'_>,
        changes: &[(Option<ChargeRecord>, Option<ChargeRecord>)],
    ) -> Result<(), ResourceError> {
        let peer_capacity = changes
            .len()
            .checked_mul(2)
            .ok_or(ResourceError::Arithmetic)?;
        let mut peer_updates = HashMap::new();
        peer_updates
            .try_reserve(peer_capacity)
            .map_err(|_| ResourceError::Allocation)?;
        let mut totals = ResourceTotals {
            preaccepted: self.preaccepted,
            remote: self.remote,
            replacement_history: self.replacement_history,
        };
        let mut accepted = self.accepted;

        for (expected, after) in changes {
            expected.map(ChargeRecord::validate).transpose()?;
            after.map(ChargeRecord::validate).transpose()?;
        }
        for (expected, _) in changes {
            let charge = ChargeProjection::from_validated(*expected)?;
            totals = totals.checked_remove(charge)?;
            accepted = checked_remove_accepted(accepted, charge.accepted)?;
            if let Some((peer, resources)) = charge.peer {
                let usage = peer_updates.entry(peer).or_insert_with(|| {
                    self.peers
                        .get(&peer)
                        .copied()
                        .unwrap_or_else(|| current.peer(peer))
                });
                *usage = usage
                    .checked_sub(resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
        }
        for (_, after) in changes {
            let charge = ChargeProjection::from_validated(*after)?;
            totals = totals.checked_add(charge)?;
            accepted = checked_add_accepted(accepted, charge.accepted)?;
            if let Some((peer, resources)) = charge.peer {
                let usage = peer_updates.entry(peer).or_insert_with(|| {
                    self.peers
                        .get(&peer)
                        .copied()
                        .unwrap_or_else(|| current.peer(peer))
                });
                *usage = usage
                    .checked_add(resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
        }
        if !totals.preaccepted.fits(self.limits.preaccepted) {
            return Err(ResourceError::PreAcceptedLimit);
        }
        if !totals.remote.fits(self.limits.remote) {
            return Err(ResourceError::RemoteLimit);
        }
        if let Some(peer) = peer_updates
            .iter()
            .filter_map(|(peer, usage)| (!usage.fits(self.limits.per_peer)).then_some(*peer))
            .min()
        {
            return Err(ResourceError::PeerLimit(peer));
        }
        if !totals
            .replacement_history
            .fits(self.limits.replacement_history)
        {
            return Err(ResourceError::ReplacementHistoryLimit);
        }
        if !accepted.fits(self.limits.accepted) {
            return Err(ResourceError::AcceptedLimit);
        }

        self.preaccepted = totals.preaccepted;
        self.remote = totals.remote;
        self.replacement_history = totals.replacement_history;
        self.accepted = accepted;
        for (peer, usage) in peer_updates {
            self.peers.insert(peer, usage);
        }
        Ok(())
    }
}
