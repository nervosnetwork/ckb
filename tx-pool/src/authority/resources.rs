#[cfg(test)]
use super::state::RawTxHash;
use super::{
    shard::{ShardResourcePlan, ShardedOwnerMap, ShardedOwnerWriteCut},
    state::{ComputeAttribution, EntryVersion, PreAcceptedEntry, ValidatedAdmission, WorkPermit},
};
use ckb_network::PeerIndex;
use ckb_types::core::TransactionView;
use ckb_util::parking_lot::Mutex;
use std::{collections::HashMap, num::NonZeroUsize, sync::Arc};
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
    fn component_max(self, other: Self) -> Self {
        Self {
            entries: self.entries.max(other.entries),
            bytes: self.bytes.max(other.bytes),
            edges: self.edges.max(other.edges),
            active_work: self.active_work.max(other.active_work),
            compute_bytes: self.compute_bytes.max(other.compute_bytes),
            compute_edges: self.compute_edges.max(other.compute_edges),
        }
    }

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
    fn component_max(self, other: Self) -> Self {
        Self {
            entries: self.entries.max(other.entries),
            serialized_bytes: self.serialized_bytes.max(other.serialized_bytes),
            resident_bytes: self.resident_bytes.max(other.resident_bytes),
            cycles: self.cycles.max(other.cycles),
        }
    }

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
    pub(in crate::authority) fn validate(self) -> Result<(), ResourceError> {
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
    fn totals(self) -> ResourceTotals {
        match self.entries.resource_totals() {
            Some(totals) => totals,
            None => {
                self.ledger.capacity.mark_faulted();
                ResourceTotals {
                    preaccepted: ResourceVector::exhausted(),
                    remote: ResourceVector::exhausted(),
                    replacement_history: ResourceVector::exhausted(),
                    accepted: AcceptedResources::exhausted(),
                }
            }
        }
    }

    pub(super) fn preaccepted(self) -> ResourceVector {
        self.totals().preaccepted
    }

    #[cfg(test)]
    pub(super) fn remote(self) -> ResourceVector {
        self.totals().remote
    }

    #[cfg(test)]
    pub(super) fn replacement_history(self) -> ResourceVector {
        self.totals().replacement_history
    }

    pub(super) fn peer(self, peer: PeerIndex) -> ResourceVector {
        self.entries.peer_resource(peer)
    }

    pub(super) fn accepted(self) -> AcceptedResources {
        self.totals().accepted
    }

    pub(super) fn accepted_fits(self, projected: AcceptedResources) -> bool {
        projected.fits(self.ledger.limits.accepted)
    }
}

pub(super) struct ResourceBatchPlan {
    shards: ShardResourcePlan,
    capacity: ResourceCapacityReservation,
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

    pub(super) fn seals_active_work_revision(&self) -> bool {
        self.0.active_work_revision.is_some()
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

    fn between(before: ResourceTotals, after: ResourceTotals) -> (Self, Self) {
        let (positive_preaccepted, release_preaccepted) =
            ResourceVector::split_transition(before.preaccepted, after.preaccepted);
        let (positive_remote, release_remote) =
            ResourceVector::split_transition(before.remote, after.remote);
        let (positive_replacement, release_replacement) =
            ResourceVector::split_transition(before.replacement_history, after.replacement_history);
        let (positive_accepted, release_accepted) =
            AcceptedResources::split_transition(before.accepted, after.accepted);
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
    #[cfg(test)]
    fn committed_projection(&self) -> Result<ResourceTotals, ResourceError> {
        let state = self.state.lock();
        if state.faulted {
            return Err(ResourceError::CapacityBankFault);
        }
        Ok(ResourceTotals {
            preaccepted: state.committed.preaccepted,
            remote: state.committed.remote,
            replacement_history: state.committed.replacement_history,
            accepted: state.committed.accepted,
        })
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

    fn accepted_totals(accepted: AcceptedResources) -> ResourceTotals {
        ResourceTotals {
            accepted,
            ..ResourceTotals::default()
        }
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
    fn prepared_release_remains_unavailable_until_the_owner_cut_finishes() {
        let bank = Arc::new(ResourceCapacityBank::default());
        commit_healthy(
            bank.reserve(one_accepted(), Default::default(), limits())
                .expect("initial committed use fits"),
        );

        let permit = bank
            .reserve(Default::default(), one_accepted(), limits())
            .expect("the exact release can be prepared")
            .begin_commit()
            .expect("capacity preparation is valid before owner mutation");
        assert_eq!(
            bank.committed_projection(),
            Ok(accepted_totals(one_accepted().accepted)),
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
                one_accepted(),
                two_accepted_limits(),
            )
            .expect("the first exact owner release reserves")
            .begin_commit()
            .expect("the first release enters its owner cut");
        let sibling = bank
            .reserve(
                ResourceCapacityDelta::default(),
                one_accepted(),
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
            assert_eq!(state.in_flight_release, one_accepted());
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
            .reserve(Default::default(), one_accepted(), limits())
            .expect("the first release plans")
            .begin_commit()
            .expect("the first release is covered by committed capacity");
        let second = bank
            .reserve(Default::default(), one_accepted(), limits())
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
        assert_eq!(bank.committed_projection(), Ok(ResourceTotals::default()));

        drop(
            bank.reserve(one_accepted(), ResourceCapacityDelta::default(), limits())
                .expect("one dropped insertion reserves capacity"),
        );
        assert_eq!(
            bank.committed_projection(),
            Ok(ResourceTotals::default()),
            "dropping an insertion returns only its outstanding reservation"
        );

        let insertion = bank
            .reserve(one_accepted(), ResourceCapacityDelta::default(), limits())
            .expect("one bounded insertion reserves capacity");
        assert_eq!(
            bank.committed_projection(),
            Ok(ResourceTotals::default()),
            "an uncommitted reservation is never reported as owner state"
        );
        commit_healthy(insertion);
        assert_eq!(
            bank.committed_projection(),
            Ok(accepted_totals(one_accepted().accepted))
        );

        drop(
            bank.reserve(ResourceCapacityDelta::default(), one_accepted(), limits())
                .expect("one dropped removal carries no reusable release"),
        );
        assert_eq!(
            bank.committed_projection(),
            Ok(accepted_totals(one_accepted().accepted)),
            "dropping a removal cannot publish uncommitted capacity"
        );

        let removal = bank
            .reserve(ResourceCapacityDelta::default(), one_accepted(), limits())
            .expect("one bounded removal carries its release until commit");
        assert_eq!(
            bank.committed_projection(),
            Ok(accepted_totals(one_accepted().accepted)),
            "a planned release cannot become reusable before semantic commit"
        );
        commit_healthy(removal);
        assert_eq!(bank.committed_projection(), Ok(ResourceTotals::default()));
    }

    #[test]
    fn ordered_batch_reserves_peak_headroom_not_only_its_net_delta() {
        let ledger = ResourceLedger::new(two_accepted_limits());
        commit_healthy(
            ledger
                .capacity
                .reserve(
                    one_accepted(),
                    ResourceCapacityDelta::default(),
                    two_accepted_limits(),
                )
                .expect("the initial committed owner fits"),
        );
        let one = accepted_totals(one_accepted().accepted);
        let two = accepted_totals(AcceptedResources::new(2, 2, 2, 2));
        let envelope = || OrderedResourceEnvelope {
            initial: one,
            peak: two,
            target: one,
        };

        let first = ledger
            .reserve_ordered_plan(ShardResourcePlan::empty(), one, one, envelope())
            .expect("the first insert-then-release prefix owns the only free headroom");
        assert!(matches!(
            ledger.reserve_ordered_plan(ShardResourcePlan::empty(), one, one, envelope()),
            Err(ResourceError::AcceptedLimit)
        ));
        drop(first);
        ledger
            .reserve_ordered_plan(ShardResourcePlan::empty(), one, one, envelope())
            .expect("dropping the first plan returns its uncommitted peak headroom");
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
        let observation = ResourceCapacityObservation::default();
        assert!(observation.explains_limit(observation, &ResourceError::PeerLimit(peer),));
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
    pub(in crate::authority) accepted: AcceptedResources,
}

impl ResourceTotals {
    fn component_max(self, other: Self) -> Self {
        Self {
            preaccepted: self.preaccepted.component_max(other.preaccepted),
            remote: self.remote.component_max(other.remote),
            replacement_history: self
                .replacement_history
                .component_max(other.replacement_history),
            accepted: self.accepted.component_max(other.accepted),
        }
    }

    pub(in crate::authority) fn checked_add_aggregate(self, added: Self) -> Option<Self> {
        Some(Self {
            preaccepted: self.preaccepted.checked_add(added.preaccepted)?,
            remote: self.remote.checked_add(added.remote)?,
            replacement_history: self
                .replacement_history
                .checked_add(added.replacement_history)?,
            accepted: self.accepted.checked_add(added.accepted)?,
        })
    }

    pub(in crate::authority) fn checked_sub_aggregate(self, removed: Self) -> Option<Self> {
        Some(Self {
            preaccepted: self.preaccepted.checked_sub(removed.preaccepted)?,
            remote: self.remote.checked_sub(removed.remote)?,
            replacement_history: self
                .replacement_history
                .checked_sub(removed.replacement_history)?,
            accepted: self.accepted.checked_sub(removed.accepted)?,
        })
    }

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
        if let Some(resources) = charge.accepted {
            self.accepted = self
                .accepted
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
        if let Some(resources) = charge.accepted {
            self.accepted = self
                .accepted
                .checked_add(resources)
                .ok_or(ResourceError::Arithmetic)?;
        }
        Ok(self)
    }
}

/// Stack-owned aggregate projection for a canonical ordered batch.
///
/// [`ResourceBatchPlan`] deliberately validates a set transition: every old
/// charge is removed before any new charge is installed. Retained ingress has
/// a stronger rule because each item must observe the resource result of every
/// earlier item in controller order. This projection evaluates that ordered
/// fold without cloning the owner map or mutating the authoritative ledger;
/// the final base-to-final changes are compiled by the sealed resource planner.
pub(super) struct OrderedResourceProjection {
    initial: ResourceTotals,
    peak: ResourceTotals,
    preaccepted: ResourceVector,
    remote: ResourceVector,
    peers: HashMap<PeerIndex, ResourceVector>,
    replacement_history: ResourceVector,
    accepted: AcceptedResources,
    limits: ResourceLimits,
    capacity_observation: ResourceCapacityObservation,
}

pub(in crate::authority) struct OrderedResourceEnvelope {
    initial: ResourceTotals,
    peak: ResourceTotals,
    target: ResourceTotals,
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

    pub(in crate::authority) fn operational_totals(
        &self,
        entries: &ShardedOwnerMap,
    ) -> ResourceTotals {
        self.read(entries).totals()
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
        let totals = current.totals();
        if self.capacity_observation().faulted {
            return Err(ResourceError::CapacityBankFault);
        }
        Ok(self.ordered_projection_from(totals, maximum_peers, capacity_observation))
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
        let totals = ResourceTotals {
            preaccepted: capacity_observation.committed.preaccepted,
            remote: capacity_observation.committed.remote,
            replacement_history: capacity_observation.committed.replacement_history,
            accepted: capacity_observation.committed.accepted,
        };
        Ok(self.ordered_projection_from(totals, maximum_peers, capacity_observation))
    }

    fn ordered_projection_from(
        &self,
        totals: ResourceTotals,
        maximum_peers: usize,
        capacity_observation: ResourceCapacityObservation,
    ) -> OrderedResourceProjection {
        let peers = HashMap::with_capacity(maximum_peers);
        OrderedResourceProjection {
            initial: totals,
            peak: totals,
            preaccepted: totals.preaccepted,
            remote: totals.remote,
            peers,
            replacement_history: totals.replacement_history,
            accepted: totals.accepted,
            limits: self.limits,
            capacity_observation,
        }
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

    pub(super) fn reserve_plan(
        &self,
        shards: ShardResourcePlan,
        before: ResourceTotals,
        after: ResourceTotals,
    ) -> Result<ResourceBatchPlan, ResourceError> {
        shards.validate_peer_targets(self.limits.per_peer)?;
        let (positive, release) = ResourceCapacityDelta::between(before, after);
        let capacity = self.capacity.reserve(positive, release, self.limits)?;
        Ok(ResourceBatchPlan { shards, capacity })
    }

    pub(in crate::authority) fn reserve_ordered_plan(
        &self,
        shards: ShardResourcePlan,
        changed_before: ResourceTotals,
        changed_after: ResourceTotals,
        envelope: OrderedResourceEnvelope,
    ) -> Result<ResourceBatchPlan, ResourceError> {
        shards.validate_peer_targets(self.limits.per_peer)?;
        if ResourceCapacityDelta::between(envelope.initial, envelope.target)
            != ResourceCapacityDelta::between(changed_before, changed_after)
        {
            return Err(ResourceError::ExistingChargeMismatch);
        }
        let (positive, premature_release) =
            ResourceCapacityDelta::between(envelope.initial, envelope.peak);
        let (late_positive, release) =
            ResourceCapacityDelta::between(envelope.peak, envelope.target);
        if premature_release != ResourceCapacityDelta::default()
            || late_positive != ResourceCapacityDelta::default()
        {
            return Err(ResourceError::Arithmetic);
        }
        let capacity = self.capacity.reserve(positive, release, self.limits)?;
        Ok(ResourceBatchPlan { shards, capacity })
    }

    pub(super) fn reserve_direct_accepted_plan(
        &self,
        shards: ShardResourcePlan,
        before: ResourceTotals,
        after: ResourceTotals,
    ) -> Result<ResourceBatchPlan, DirectAcceptedInsertionError> {
        shards.validate_peer_targets(self.limits.per_peer)?;
        let (positive, release) = ResourceCapacityDelta::between(before, after);
        if release != ResourceCapacityDelta::default() {
            return Err(ResourceError::CapacityBankFault.into());
        }
        let capacity = self
            .capacity
            .reserve_direct_accepted(positive, self.limits)?;
        Ok(ResourceBatchPlan { shards, capacity })
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
    pub(in crate::authority) fn into_envelope(self) -> OrderedResourceEnvelope {
        OrderedResourceEnvelope {
            initial: self.initial,
            peak: self.peak,
            target: ResourceTotals {
                preaccepted: self.preaccepted,
                remote: self.remote,
                replacement_history: self.replacement_history,
                accepted: self.accepted,
            },
        }
    }

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
            accepted: self.accepted,
        }
        .checked_remove(old_charge)?
        .checked_add(new_charge)?;
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
        if !totals.accepted.fits(self.limits.accepted) {
            return Err(ResourceError::AcceptedLimit);
        }

        self.preaccepted = totals.preaccepted;
        self.remote = totals.remote;
        self.replacement_history = totals.replacement_history;
        self.accepted = totals.accepted;
        self.peak = self.peak.component_max(totals);
        if let Some((peer, usage)) = old_peer_after {
            self.peers.insert(peer, usage);
        }
        if let Some((peer, usage)) = new_peer_after {
            self.peers.insert(peer, usage);
        }
        Ok(())
    }
}
