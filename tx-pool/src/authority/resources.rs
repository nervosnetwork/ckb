use super::state::{
    ComputeAttribution, PreAcceptedEntry, RawTxHash, ValidatedAdmission, WorkPermit,
};
use ckb_network::PeerIndex;
use ckb_types::core::TransactionView;
use std::{
    collections::{HashMap, HashSet},
    num::NonZeroUsize,
};

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
    Allocation,
}

#[cfg(test)]
#[path = "tests/support/resources.rs"]
pub(in crate::authority) mod test_support;

/// Closed error surface for releasing one existing compute reservation.
/// This transition neither inserts a charge nor creates a peer row, so
/// allocator backpressure is not a legal result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ComputeReleaseError {
    Arithmetic,
    Projection,
}

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

#[derive(Debug)]
pub(super) struct ResourceLedger {
    charges: HashMap<RawTxHash, ChargeRecord>,
    preaccepted: ResourceVector,
    remote: ResourceVector,
    peers: HashMap<PeerIndex, ResourceVector>,
    replacement_history: ResourceVector,
    accepted: AcceptedResources,
    limits: ResourceLimits,
}

pub(super) struct ResourcePlan {
    key: RawTxHash,
    after: Option<ChargeRecord>,
    preaccepted: ResourceVector,
    remote: ResourceVector,
    peer_updates: [Option<(PeerIndex, ResourceVector)>; 2],
    replacement_history: ResourceVector,
    accepted: AcceptedResources,
}

pub(super) struct ResourceBatchPlan {
    changes: Vec<(RawTxHash, Option<ChargeRecord>, Option<ChargeRecord>)>,
    preaccepted: ResourceVector,
    remote: ResourceVector,
    peer_updates: HashMap<PeerIndex, ResourceVector>,
    replacement_history: ResourceVector,
    accepted: AcceptedResources,
}

/// Stack-owned aggregate projection for a canonical ordered batch.
///
/// [`ResourceBatchPlan`] deliberately validates a set transition: every old
/// charge is removed before any new charge is installed. Retained ingress has
/// a stronger rule because each item must observe the resource result of every
/// earlier item in controller order. This projection evaluates that ordered
/// fold without cloning the charge map or mutating the authoritative ledger;
/// the final base-to-final changes are still compiled by [`ResourceLedger::plan_batch`].
pub(super) struct OrderedResourceProjection {
    preaccepted: ResourceVector,
    remote: ResourceVector,
    peers: HashMap<PeerIndex, ResourceVector>,
    replacement_history: ResourceVector,
    accepted: AcceptedResources,
    limits: ResourceLimits,
}

impl ResourceLedger {
    pub(super) fn new(limits: ResourceLimits) -> Self {
        Self {
            charges: HashMap::new(),
            preaccepted: ResourceVector::default(),
            remote: ResourceVector::default(),
            peers: HashMap::new(),
            replacement_history: ResourceVector::default(),
            accepted: AcceptedResources::default(),
            limits,
        }
    }

    pub(super) fn charge(&self, key: &RawTxHash) -> Option<ChargeRecord> {
        self.charges.get(key).copied()
    }

    pub(super) fn preaccepted(&self) -> ResourceVector {
        self.preaccepted
    }

    pub(super) fn remote(&self) -> ResourceVector {
        self.remote
    }

    pub(super) fn replacement_history(&self) -> ResourceVector {
        self.replacement_history
    }

    pub(super) fn peer(&self, peer: PeerIndex) -> ResourceVector {
        self.peers.get(&peer).copied().unwrap_or_default()
    }

    pub(super) fn accepted(&self) -> AcceptedResources {
        self.accepted
    }

    pub(super) fn accepted_fits(&self, projected: AcceptedResources) -> bool {
        projected.fits(self.limits.accepted)
    }

    pub(super) fn ordered_projection(
        &self,
        maximum_peers: usize,
    ) -> Result<OrderedResourceProjection, ResourceError> {
        let mut peers = HashMap::new();
        peers
            .try_reserve(maximum_peers)
            .map_err(|_| ResourceError::Allocation)?;
        Ok(OrderedResourceProjection {
            preaccepted: self.preaccepted,
            remote: self.remote,
            peers,
            replacement_history: self.replacement_history,
            accepted: self.accepted,
            limits: self.limits,
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

    pub(super) fn plan_replace(
        &mut self,
        key: RawTxHash,
        expected: Option<ChargeRecord>,
        after: Option<ChargeRecord>,
    ) -> Result<ResourcePlan, ResourceError> {
        expected.map(ChargeRecord::validate).transpose()?;
        after.map(ChargeRecord::validate).transpose()?;
        if self.charge(&key) != expected {
            return Err(ResourceError::ExistingChargeMismatch);
        }

        let old_preaccepted = expected.and_then(ChargeRecord::preaccepted);
        let new_preaccepted = after.and_then(ChargeRecord::preaccepted);
        let old_replacement_history = expected.and_then(ChargeRecord::replacement_history);
        let new_replacement_history = after.and_then(ChargeRecord::replacement_history);
        let old_peer_charge = expected
            .map(ChargeRecord::peer_preaccepted)
            .transpose()?
            .flatten();
        let new_peer_charge = after
            .map(ChargeRecord::peer_preaccepted)
            .transpose()?
            .flatten();
        let mut preaccepted = self.preaccepted;
        let mut remote = self.remote;
        let mut replacement_history = self.replacement_history;
        if let Some(resources) = old_preaccepted {
            preaccepted = preaccepted
                .checked_sub(resources)
                .ok_or(ResourceError::Arithmetic)?;
        }
        if let Some((_, resources)) = old_peer_charge {
            remote = remote
                .checked_sub(resources)
                .ok_or(ResourceError::Arithmetic)?;
        }
        if let Some(resources) = old_replacement_history {
            replacement_history = replacement_history
                .checked_sub(resources)
                .ok_or(ResourceError::Arithmetic)?;
        }
        if let Some(resources) = new_preaccepted {
            preaccepted = preaccepted
                .checked_add(resources)
                .ok_or(ResourceError::Arithmetic)?;
        }
        if let Some((_, resources)) = new_peer_charge {
            remote = remote
                .checked_add(resources)
                .ok_or(ResourceError::Arithmetic)?;
        }
        if let Some(resources) = new_replacement_history {
            replacement_history = replacement_history
                .checked_add(resources)
                .ok_or(ResourceError::Arithmetic)?;
        }
        if !preaccepted.fits(self.limits.preaccepted) {
            return Err(ResourceError::PreAcceptedLimit);
        }
        if !remote.fits(self.limits.remote) {
            return Err(ResourceError::RemoteLimit);
        }
        if !replacement_history.fits(self.limits.replacement_history) {
            return Err(ResourceError::ReplacementHistoryLimit);
        }

        let old_peer = old_peer_charge.map(|(peer, _)| peer);
        let new_peer = new_peer_charge.map(|(peer, _)| peer);
        let project_peer = |peer: PeerIndex| {
            let mut usage = self.peer(peer);
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
        let old_update = old_peer
            .map(|peer| project_peer(peer).map(|usage| (peer, usage)))
            .transpose()?;
        let new_update = new_peer
            .filter(|peer| Some(*peer) != old_peer)
            .map(|peer| project_peer(peer).map(|usage| (peer, usage)))
            .transpose()?;

        let old_accepted = expected.and_then(ChargeRecord::accepted);
        let new_accepted = after.and_then(ChargeRecord::accepted);
        let mut accepted = self.accepted;
        if let Some(resources) = old_accepted {
            accepted = accepted
                .checked_sub(resources)
                .ok_or(ResourceError::Arithmetic)?;
        }
        if let Some(resources) = new_accepted {
            accepted = accepted
                .checked_add(resources)
                .ok_or(ResourceError::Arithmetic)?;
        }
        if !accepted.fits(self.limits.accepted) {
            return Err(ResourceError::AcceptedLimit);
        }

        if expected.is_none() && after.is_some() {
            self.charges
                .try_reserve(1)
                .map_err(|_| ResourceError::Allocation)?;
        }
        if new_peer.is_some_and(|peer| !self.peers.contains_key(&peer)) {
            self.peers
                .try_reserve(1)
                .map_err(|_| ResourceError::Allocation)?;
        }

        Ok(ResourcePlan {
            key,
            after,
            preaccepted,
            remote,
            peer_updates: [old_update, new_update],
            replacement_history,
            accepted,
        })
    }

    /// Plan `Computing -> Queued(Resolve)` for the same primary owner.
    ///
    /// Cancellation is the emergency discharge path for the sole compute
    /// capability. It may only remove resource usage from an existing charge
    /// and therefore performs no reservation or insertion.
    pub(super) fn plan_compute_release(
        &self,
        key: RawTxHash,
        expected: ChargeRecord,
        after: ChargeRecord,
    ) -> Result<ResourcePlan, ComputeReleaseError> {
        expected
            .validate()
            .map_err(|_| ComputeReleaseError::Projection)?;
        after
            .validate()
            .map_err(|_| ComputeReleaseError::Projection)?;
        if self.charge(&key) != Some(expected) {
            return Err(ComputeReleaseError::Projection);
        }
        let (
            ChargeRecord::PreAccepted {
                resources: old_resources,
                residency_peer: old_residency_peer,
                ..
            },
            ChargeRecord::PreAccepted {
                resources: new_resources,
                residency_peer: new_residency_peer,
                compute_peer: new_compute_peer,
            },
        ) = (expected, after)
        else {
            return Err(ComputeReleaseError::Projection);
        };
        if old_resources.active_work != 1
            || new_resources.active_work != 0
            || old_residency_peer != new_residency_peer
            || new_compute_peer.is_some()
            || new_resources != old_resources.without_compute()
        {
            return Err(ComputeReleaseError::Projection);
        }

        let preaccepted = self
            .preaccepted
            .checked_sub(old_resources)
            .and_then(|usage| usage.checked_add(new_resources))
            .ok_or(ComputeReleaseError::Arithmetic)?;
        let old_peer_charge = expected
            .peer_preaccepted()
            .map_err(|_| ComputeReleaseError::Projection)?;
        let new_peer_charge = after
            .peer_preaccepted()
            .map_err(|_| ComputeReleaseError::Projection)?;
        let (remote, peer_update) = match (old_peer_charge, new_peer_charge) {
            (None, None) => (self.remote, None),
            (Some((old_peer, old_usage)), Some((new_peer, new_usage)))
                if old_peer == new_peer && self.peers.contains_key(&old_peer) =>
            {
                let remote = self
                    .remote
                    .checked_sub(old_usage)
                    .and_then(|usage| usage.checked_add(new_usage))
                    .ok_or(ComputeReleaseError::Arithmetic)?;
                let peer = self
                    .peer(old_peer)
                    .checked_sub(old_usage)
                    .and_then(|usage| usage.checked_add(new_usage))
                    .ok_or(ComputeReleaseError::Arithmetic)?;
                (remote, Some((old_peer, peer)))
            }
            _ => return Err(ComputeReleaseError::Projection),
        };
        if !preaccepted.fits(self.limits.preaccepted)
            || !remote.fits(self.limits.remote)
            || peer_update.is_some_and(|(_, usage)| !usage.fits(self.limits.per_peer))
        {
            return Err(ComputeReleaseError::Projection);
        }

        Ok(ResourcePlan {
            key,
            after: Some(after),
            preaccepted,
            remote,
            peer_updates: [peer_update, None],
            replacement_history: self.replacement_history,
            accepted: self.accepted,
        })
    }

    pub(super) fn plan_batch(
        &mut self,
        changes: Vec<(RawTxHash, Option<ChargeRecord>, Option<ChargeRecord>)>,
    ) -> Result<ResourceBatchPlan, ResourceError> {
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
        let mut preaccepted = self.preaccepted;
        let mut remote = self.remote;
        let mut replacement_history = self.replacement_history;
        let mut accepted = self.accepted;
        let mut new_charge_count = 0usize;

        for (key, expected, after) in &changes {
            expected.map(ChargeRecord::validate).transpose()?;
            after.map(ChargeRecord::validate).transpose()?;
            if !keys.insert(key.clone()) {
                return Err(ResourceError::DuplicateChange);
            }
            if self.charge(key) != *expected {
                return Err(ResourceError::ExistingChargeMismatch);
            }
            if expected.is_none() && after.is_some() {
                new_charge_count = new_charge_count
                    .checked_add(1)
                    .ok_or(ResourceError::Arithmetic)?;
            }
        }

        // A batch is a set transition, not a caller-ordered sequence. Remove
        // every old charge before adding any new charge so a valid net change
        // cannot overflow only because its freeing member appeared later in
        // the input vector.
        for (_, expected, _) in &changes {
            if let Some(resources) = expected.and_then(ChargeRecord::preaccepted) {
                preaccepted = preaccepted
                    .checked_sub(resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
            if let Some((peer, resources)) = expected
                .map(ChargeRecord::peer_preaccepted)
                .transpose()?
                .flatten()
            {
                remote = remote
                    .checked_sub(resources)
                    .ok_or(ResourceError::Arithmetic)?;
                let usage = peer_updates.entry(peer).or_insert_with(|| self.peer(peer));
                *usage = usage
                    .checked_sub(resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
            if let Some(resources) = expected.and_then(ChargeRecord::replacement_history) {
                replacement_history = replacement_history
                    .checked_sub(resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
            if let Some(resources) = expected.and_then(ChargeRecord::accepted) {
                accepted = accepted
                    .checked_sub(resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
        }
        for (_, _, after) in &changes {
            if let Some(resources) = after.and_then(ChargeRecord::preaccepted) {
                preaccepted = preaccepted
                    .checked_add(resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
            if let Some((peer, resources)) = after
                .map(ChargeRecord::peer_preaccepted)
                .transpose()?
                .flatten()
            {
                remote = remote
                    .checked_add(resources)
                    .ok_or(ResourceError::Arithmetic)?;
                let usage = peer_updates.entry(peer).or_insert_with(|| self.peer(peer));
                *usage = usage
                    .checked_add(resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
            if let Some(resources) = after.and_then(ChargeRecord::replacement_history) {
                replacement_history = replacement_history
                    .checked_add(resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
            if let Some(resources) = after.and_then(ChargeRecord::accepted) {
                accepted = accepted
                    .checked_add(resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
        }

        if !preaccepted.fits(self.limits.preaccepted) {
            return Err(ResourceError::PreAcceptedLimit);
        }
        if !remote.fits(self.limits.remote) {
            return Err(ResourceError::RemoteLimit);
        }
        if let Some(peer) = peer_updates
            .iter()
            .filter_map(|(peer, usage)| (!usage.fits(self.limits.per_peer)).then_some(*peer))
            .min()
        {
            return Err(ResourceError::PeerLimit(peer));
        }
        if !replacement_history.fits(self.limits.replacement_history) {
            return Err(ResourceError::ReplacementHistoryLimit);
        }
        if !accepted.fits(self.limits.accepted) {
            return Err(ResourceError::AcceptedLimit);
        }

        self.charges
            .try_reserve(new_charge_count)
            .map_err(|_| ResourceError::Allocation)?;
        let new_peer_count = peer_updates
            .keys()
            .filter(|peer| !self.peers.contains_key(peer))
            .count();
        self.peers
            .try_reserve(new_peer_count)
            .map_err(|_| ResourceError::Allocation)?;
        Ok(ResourceBatchPlan {
            changes,
            preaccepted,
            remote,
            peer_updates,
            replacement_history,
            accepted,
        })
    }

    pub(super) fn apply(&mut self, plan: ResourcePlan) {
        match plan.after {
            Some(charge) => {
                self.charges.insert(plan.key, charge);
            }
            None => {
                self.charges.remove(&plan.key);
            }
        }
        self.preaccepted = plan.preaccepted;
        self.remote = plan.remote;
        self.replacement_history = plan.replacement_history;
        for (peer, usage) in plan.peer_updates.into_iter().flatten() {
            if usage == ResourceVector::default() {
                self.peers.remove(&peer);
            } else {
                self.peers.insert(peer, usage);
            }
        }
        self.accepted = plan.accepted;
    }

    pub(super) fn apply_batch(&mut self, plan: ResourceBatchPlan) {
        for (key, _, after) in plan.changes {
            match after {
                Some(charge) => {
                    self.charges.insert(key, charge);
                }
                None => {
                    self.charges.remove(&key);
                }
            }
        }
        self.preaccepted = plan.preaccepted;
        self.remote = plan.remote;
        self.replacement_history = plan.replacement_history;
        for (peer, usage) in plan.peer_updates {
            if usage == ResourceVector::default() {
                self.peers.remove(&peer);
            } else {
                self.peers.insert(peer, usage);
            }
        }
        self.accepted = plan.accepted;
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
    pub(super) fn active_work_availability(
        &self,
        ledger: &ResourceLedger,
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
                        .unwrap_or_else(|| ledger.peer(peer)),
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
        ledger: &ResourceLedger,
        expected: Option<ChargeRecord>,
        after: Option<ChargeRecord>,
    ) -> Result<(), ResourceError> {
        expected.map(ChargeRecord::validate).transpose()?;
        after.map(ChargeRecord::validate).transpose()?;

        let old_preaccepted = expected.and_then(ChargeRecord::preaccepted);
        let new_preaccepted = after.and_then(ChargeRecord::preaccepted);
        let old_history = expected.and_then(ChargeRecord::replacement_history);
        let new_history = after.and_then(ChargeRecord::replacement_history);
        let old_accepted = expected.and_then(ChargeRecord::accepted);
        let new_accepted = after.and_then(ChargeRecord::accepted);
        let old_peer = expected
            .map(ChargeRecord::peer_preaccepted)
            .transpose()?
            .flatten();
        let new_peer = after
            .map(ChargeRecord::peer_preaccepted)
            .transpose()?
            .flatten();

        let mut preaccepted = self.preaccepted;
        let mut remote = self.remote;
        let mut replacement_history = self.replacement_history;
        let mut accepted = self.accepted;
        if let Some(resources) = old_preaccepted {
            preaccepted = preaccepted
                .checked_sub(resources)
                .ok_or(ResourceError::Arithmetic)?;
        }
        if let Some(resources) = new_preaccepted {
            preaccepted = preaccepted
                .checked_add(resources)
                .ok_or(ResourceError::Arithmetic)?;
        }
        if let Some(resources) = old_history {
            replacement_history = replacement_history
                .checked_sub(resources)
                .ok_or(ResourceError::Arithmetic)?;
        }
        if let Some(resources) = new_history {
            replacement_history = replacement_history
                .checked_add(resources)
                .ok_or(ResourceError::Arithmetic)?;
        }
        if let Some(resources) = old_accepted {
            accepted = accepted
                .checked_sub(resources)
                .ok_or(ResourceError::Arithmetic)?;
        }
        if let Some(resources) = new_accepted {
            accepted = accepted
                .checked_add(resources)
                .ok_or(ResourceError::Arithmetic)?;
        }

        let current_peer = |peer: PeerIndex| {
            self.peers
                .get(&peer)
                .copied()
                .unwrap_or_else(|| ledger.peer(peer))
        };
        let old_peer_after = old_peer
            .map(|(peer, resources)| {
                current_peer(peer)
                    .checked_sub(resources)
                    .map(|usage| (peer, usage))
                    .ok_or(ResourceError::Arithmetic)
            })
            .transpose()?;
        if let Some((_, resources)) = old_peer {
            remote = remote
                .checked_sub(resources)
                .ok_or(ResourceError::Arithmetic)?;
        }
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
        if let Some((_, resources)) = new_peer {
            remote = remote
                .checked_add(resources)
                .ok_or(ResourceError::Arithmetic)?;
        }

        if !preaccepted.fits(self.limits.preaccepted) {
            return Err(ResourceError::PreAcceptedLimit);
        }
        if !remote.fits(self.limits.remote) {
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
        if !replacement_history.fits(self.limits.replacement_history) {
            return Err(ResourceError::ReplacementHistoryLimit);
        }
        if !accepted.fits(self.limits.accepted) {
            return Err(ResourceError::AcceptedLimit);
        }

        self.preaccepted = preaccepted;
        self.remote = remote;
        self.replacement_history = replacement_history;
        self.accepted = accepted;
        if let Some((peer, usage)) = old_peer_after {
            self.peers.insert(peer, usage);
        }
        if let Some((peer, usage)) = new_peer_after {
            self.peers.insert(peer, usage);
        }
        Ok(())
    }
}
