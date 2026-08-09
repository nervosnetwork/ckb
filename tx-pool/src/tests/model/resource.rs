#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct PayloadBytes(pub(super) u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ResolvedResidentBytes(pub(super) u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct EntryMetadataBytes(pub(super) u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct EdgeMetadataBytes(pub(super) u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct TotalRetainedBytes(pub(super) u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RetainedChargeInputs {
    pub(super) payload: PayloadBytes,
    pub(super) resolved: ResolvedResidentBytes,
    pub(super) entry_metadata: EntryMetadataBytes,
    pub(super) edge_metadata: EdgeMetadataBytes,
}

impl RetainedChargeInputs {
    pub(super) fn compile(self) -> Option<TotalRetainedBytes> {
        self.payload
            .0
            .max(self.resolved.0)
            .checked_add(self.entry_metadata.0)?
            .checked_add(self.edge_metadata.0)
            .map(TotalRetainedBytes)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ComputeGrant {
    pub(super) max_total_retained: TotalRetainedBytes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ComputeAdmission {
    Granted(TotalRetainedBytes),
    ResourceExcluded,
    ArithmeticExcluded,
}

impl ComputeGrant {
    pub(super) fn admit(self, inputs: RetainedChargeInputs) -> ComputeAdmission {
        let Some(total) = inputs.compile() else {
            return ComputeAdmission::ArithmeticExcluded;
        };
        if total <= self.max_total_retained {
            ComputeAdmission::Granted(total)
        } else {
            ComputeAdmission::ResourceExcluded
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ScratchDisposition {
    Prepared,
    OrdinaryUnavailable,
}

pub(super) fn prepare_bounded_scratch(
    requested_items: u16,
    item_bound: u16,
    allocation_available: bool,
) -> ScratchDisposition {
    if requested_items > item_bound || !allocation_available {
        ScratchDisposition::OrdinaryUnavailable
    } else {
        ScratchDisposition::Prepared
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct QueryCostInputs {
    pub(super) concurrent_queries: u32,
    pub(super) owner_rows: u32,
    pub(super) accepted_order_rows: u32,
    pub(super) output_items: u32,
    pub(super) output_item_bytes: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct QueryCostUpperBound {
    pub(super) authority_row_visits: u64,
    pub(super) sort_comparisons: u64,
    pub(super) output_resident_bytes: u64,
}

impl QueryCostInputs {
    /// Compile the static adversarial upper-bound terms that remain visible
    /// even before profiling. `sort_comparisons` uses `n * ceil(log2(n))`;
    /// it is a comparison-count bound, not a wall-time prediction.
    pub(super) fn compile(self) -> Option<QueryCostUpperBound> {
        let concurrency = u64::from(self.concurrent_queries);
        let owners = u64::from(self.owner_rows);
        let accepted = u64::from(self.accepted_order_rows);
        let authority_row_visits = concurrency.checked_mul(owners.checked_add(accepted)?)?;
        let sort_levels = if self.accepted_order_rows <= 1 {
            0
        } else {
            u64::from(u32::BITS - (self.accepted_order_rows - 1).leading_zeros())
        };
        let sort_comparisons = concurrency
            .checked_mul(accepted)?
            .checked_mul(sort_levels)?;
        let output_resident_bytes = concurrency
            .checked_mul(u64::from(self.output_items))?
            .checked_mul(u64::from(self.output_item_bytes))?;
        Some(QueryCostUpperBound {
            authority_row_visits,
            sort_comparisons,
            output_resident_bytes,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ServiceIngressResidencyInputs {
    pub(super) verify_workers: u32,
    pub(super) handler_multiplier: u32,
    pub(super) ordinary_queue: u32,
    pub(super) ordered_queue: u32,
    /// The chain actor's lossless producer-owned payload suspended before
    /// channel admission. Production has exactly one chain publisher.
    pub(super) trusted_reorg_waiting_senders: u32,
    /// Public administrative payloads owning the unique admission capability
    /// while suspended before channel admission.
    pub(super) admitted_admin_waiting_senders: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ServiceIngressResidencyBound {
    pub(super) ordinary_handlers: u32,
    pub(super) ordinary_owned_requests: u32,
    pub(super) ordered_owned_requests: u32,
}

impl ServiceIngressResidencyInputs {
    /// Compile the exact count-only service terms. Payload bytes remain a
    /// separate protocol term. Reliable send alone does not bound producer-
    /// owned payloads, so the typed producer protocol is part of this bound.
    pub(super) fn compile(self) -> Option<ServiceIngressResidencyBound> {
        if self.handler_multiplier == 0
            || self.trusted_reorg_waiting_senders > 1
            || self.admitted_admin_waiting_senders > 1
        {
            return None;
        }
        let ordinary_handlers = self
            .verify_workers
            .max(1)
            .checked_mul(self.handler_multiplier)?;
        let ordinary_owned_requests = self.ordinary_queue.checked_add(ordinary_handlers)?;
        let ordered_owned_requests = self
            .ordered_queue
            .checked_add(1)?
            .checked_add(self.trusted_reorg_waiting_senders)?
            .checked_add(self.admitted_admin_waiting_senders)?;
        Some(ServiceIngressResidencyBound {
            ordinary_handlers,
            ordinary_owned_requests,
            ordered_owned_requests,
        })
    }
}

// Continuous resource transition algebra. These types deliberately use a
// small checked domain so properties can exhaust invalid partial reservations;
// production refinement converts only representable usize/u64 inputs.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct ContinuousResourceVector {
    pub(super) entries: u16,
    pub(super) bytes: u16,
    pub(super) edges: u16,
    pub(super) active_work: u16,
    pub(super) compute_bytes: u16,
    pub(super) compute_edges: u16,
}

impl ContinuousResourceVector {
    pub(super) const fn retained(entries: u16, bytes: u16, edges: u16) -> Self {
        Self {
            entries,
            bytes,
            edges,
            active_work: 0,
            compute_bytes: 0,
            compute_edges: 0,
        }
    }

    pub(super) fn reserve_compute(
        self,
        grant: ModelComputeGrant,
    ) -> Option<ContinuousResourceVector> {
        if self.has_compute_reservation() {
            return None;
        }
        Some(Self {
            active_work: 1,
            compute_bytes: grant.total_retained_bytes,
            compute_edges: grant.edges,
            ..self
        })
    }

    pub(super) const fn without_compute(self) -> Self {
        Self {
            active_work: 0,
            compute_bytes: 0,
            compute_edges: 0,
            ..self
        }
    }

    pub(super) const fn has_compute_reservation(self) -> bool {
        self.active_work != 0 || self.compute_bytes != 0 || self.compute_edges != 0
    }

    pub(super) fn total_bytes(self) -> Option<u16> {
        self.bytes.checked_add(self.compute_bytes)
    }

    pub(super) fn total_edges(self) -> Option<u16> {
        self.edges.checked_add(self.compute_edges)
    }

    fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_add(other.entries)?,
            bytes: self.bytes.checked_add(other.bytes)?,
            edges: self.edges.checked_add(other.edges)?,
            active_work: self.active_work.checked_add(other.active_work)?,
            compute_bytes: self.compute_bytes.checked_add(other.compute_bytes)?,
            compute_edges: self.compute_edges.checked_add(other.compute_edges)?,
        })
    }

    fn fits(self, limit: Self) -> bool {
        self.entries <= limit.entries
            && self.bytes <= limit.bytes
            && self.edges <= limit.edges
            && self.active_work <= limit.active_work
            && self.compute_bytes <= limit.compute_bytes
            && self.compute_edges <= limit.compute_edges
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ModelComputeGrant {
    pub(super) total_retained_bytes: u16,
    pub(super) edges: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ContinuousComputeLimits {
    pub(super) resolved_total_retained_bytes: u16,
    pub(super) accepted_total_retained_bytes: u16,
    pub(super) expanded_edges: u16,
}

impl ContinuousComputeLimits {
    pub(super) const fn max_total_retained_bytes(self) -> u16 {
        if self.resolved_total_retained_bytes > self.accepted_total_retained_bytes {
            self.resolved_total_retained_bytes
        } else {
            self.accepted_total_retained_bytes
        }
    }

    pub(super) const fn admits(self, resources: ContinuousResourceVector) -> bool {
        resources.entries == 1
            && !resources.has_compute_reservation()
            && resources.bytes <= self.resolved_total_retained_bytes
            && resources.bytes <= self.accepted_total_retained_bytes
            && resources.edges <= self.expanded_edges
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct ContinuousAcceptedResources {
    pub(super) entries: u16,
    pub(super) serialized_bytes: u16,
    pub(super) resident_bytes: u16,
    pub(super) cycles: u16,
}

impl ContinuousAcceptedResources {
    fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_add(other.entries)?,
            serialized_bytes: self.serialized_bytes.checked_add(other.serialized_bytes)?,
            resident_bytes: self.resident_bytes.checked_add(other.resident_bytes)?,
            cycles: self.cycles.checked_add(other.cycles)?,
        })
    }

    fn fits(self, limit: Self) -> bool {
        self.entries <= limit.entries
            && self.serialized_bytes <= limit.serialized_bytes
            && self.resident_bytes <= limit.resident_bytes
            && self.cycles <= limit.cycles
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ContinuousResourceLimits {
    pub(super) preaccepted: ContinuousResourceVector,
    pub(super) remote: ContinuousResourceVector,
    pub(super) per_peer: ContinuousResourceVector,
    pub(super) replacement_history: ContinuousResourceVector,
    pub(super) accepted: ContinuousAcceptedResources,
    pub(super) compute: ContinuousComputeLimits,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ContinuousResourceConfigError {
    LimitHierarchy,
    MissingComputeCapacity,
    NonMonotonicComputeEnvelope,
    TransientComputeOverflow,
}

impl ContinuousResourceLimits {
    pub(super) fn validate(
        preaccepted: ContinuousResourceVector,
        remote: ContinuousResourceVector,
        per_peer: ContinuousResourceVector,
        replacement_history: ContinuousResourceVector,
        accepted: ContinuousAcceptedResources,
        compute: ContinuousComputeLimits,
    ) -> Result<Self, ContinuousResourceConfigError> {
        if !remote.fits(preaccepted)
            || !per_peer.fits(remote)
            || !replacement_history.fits(preaccepted)
            || replacement_history.has_compute_reservation()
        {
            return Err(ContinuousResourceConfigError::LimitHierarchy);
        }
        if compute.resolved_total_retained_bytes == 0
            || compute.accepted_total_retained_bytes == 0
            || (preaccepted.entries != 0 && preaccepted.active_work == 0)
            || (remote.entries != 0 && remote.active_work == 0)
            || (per_peer.entries != 0 && per_peer.active_work == 0)
        {
            return Err(ContinuousResourceConfigError::MissingComputeCapacity);
        }
        if compute.accepted_total_retained_bytes < compute.resolved_total_retained_bytes {
            return Err(ContinuousResourceConfigError::NonMonotonicComputeEnvelope);
        }
        for limit in [preaccepted, remote, per_peer] {
            limit
                .total_bytes()
                .and_then(|_| limit.total_edges())
                .ok_or(ContinuousResourceConfigError::TransientComputeOverflow)?;
            if limit.active_work != 0
                && (limit.compute_bytes < compute.max_total_retained_bytes()
                    || limit.compute_edges < compute.expanded_edges)
            {
                return Err(ContinuousResourceConfigError::MissingComputeCapacity);
            }
        }
        Ok(Self {
            preaccepted,
            remote,
            per_peer,
            replacement_history,
            accepted,
            compute,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ContinuousChargeRecord {
    PreAccepted {
        resources: ContinuousResourceVector,
        residency_peer: Option<u8>,
        compute_peer: Option<u8>,
    },
    ReplacementHistory(ContinuousResourceVector),
    Accepted(ContinuousAcceptedResources),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ContinuousChargeError {
    ComputeEnvelope,
    AttributionMismatch,
    Arithmetic,
    ExistingChargeMismatch,
    DuplicateChange,
    ResourceLimit,
    InvalidComputeRelease,
}

impl ContinuousChargeRecord {
    pub(super) fn validate(self) -> Result<Self, ContinuousChargeError> {
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
                    return Err(ContinuousChargeError::ComputeEnvelope);
                }
            }
            Self::ReplacementHistory(resources)
                if resources.entries != 1 || resources.has_compute_reservation() =>
            {
                return Err(ContinuousChargeError::ComputeEnvelope);
            }
            Self::ReplacementHistory(_) | Self::Accepted(_) => {}
        }
        Ok(self)
    }

    fn preaccepted(self) -> Option<ContinuousResourceVector> {
        match self {
            Self::PreAccepted { resources, .. } | Self::ReplacementHistory(resources) => {
                Some(resources)
            }
            Self::Accepted(_) => None,
        }
    }

    fn replacement_history(self) -> Option<ContinuousResourceVector> {
        match self {
            Self::ReplacementHistory(resources) => Some(resources),
            Self::PreAccepted { .. } | Self::Accepted(_) => None,
        }
    }

    fn remote(self) -> Result<Option<(u8, ContinuousResourceVector)>, ContinuousChargeError> {
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
                Err(ContinuousChargeError::AttributionMismatch)
            };
        };
        if compute_peer.is_some_and(|compute_peer| compute_peer != peer) {
            return Err(ContinuousChargeError::AttributionMismatch);
        }
        Ok(Some((
            peer,
            if compute_peer == Some(peer) {
                resources
            } else {
                resources.without_compute()
            },
        )))
    }

    fn accepted(self) -> Option<ContinuousAcceptedResources> {
        match self {
            Self::Accepted(resources) => Some(resources),
            Self::PreAccepted { .. } | Self::ReplacementHistory(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ContinuousResourceChange {
    pub(super) key: u8,
    pub(super) expected: Option<ContinuousChargeRecord>,
    pub(super) after: Option<ContinuousChargeRecord>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ContinuousResourceUsage {
    pub(super) preaccepted: ContinuousResourceVector,
    pub(super) remote: ContinuousResourceVector,
    pub(super) per_peer: std::collections::BTreeMap<u8, ContinuousResourceVector>,
    pub(super) replacement_history: ContinuousResourceVector,
    pub(super) accepted: ContinuousAcceptedResources,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ContinuousResourceLedger {
    limits: ContinuousResourceLimits,
    charges: std::collections::BTreeMap<u8, ContinuousChargeRecord>,
}

impl ContinuousResourceLedger {
    pub(super) fn new(
        limits: ContinuousResourceLimits,
        charges: std::collections::BTreeMap<u8, ContinuousChargeRecord>,
    ) -> Result<Self, ContinuousChargeError> {
        let ledger = Self { limits, charges };
        ledger.usage()?;
        Ok(ledger)
    }

    pub(super) fn charges(&self) -> &std::collections::BTreeMap<u8, ContinuousChargeRecord> {
        &self.charges
    }

    pub(super) fn usage(&self) -> Result<ContinuousResourceUsage, ContinuousChargeError> {
        let mut usage = ContinuousResourceUsage::default();
        for charge in self.charges.values().copied() {
            let charge = charge.validate()?;
            if let Some(resources) = charge.preaccepted() {
                usage.preaccepted = usage
                    .preaccepted
                    .checked_add(resources)
                    .ok_or(ContinuousChargeError::Arithmetic)?;
            }
            if let Some((peer, resources)) = charge.remote()? {
                usage.remote = usage
                    .remote
                    .checked_add(resources)
                    .ok_or(ContinuousChargeError::Arithmetic)?;
                let peer_usage = usage.per_peer.entry(peer).or_default();
                *peer_usage = peer_usage
                    .checked_add(resources)
                    .ok_or(ContinuousChargeError::Arithmetic)?;
            }
            if let Some(resources) = charge.replacement_history() {
                usage.replacement_history = usage
                    .replacement_history
                    .checked_add(resources)
                    .ok_or(ContinuousChargeError::Arithmetic)?;
            }
            if let Some(resources) = charge.accepted() {
                usage.accepted = usage
                    .accepted
                    .checked_add(resources)
                    .ok_or(ContinuousChargeError::Arithmetic)?;
            }
        }
        if !usage.preaccepted.fits(self.limits.preaccepted)
            || !usage.remote.fits(self.limits.remote)
            || !usage
                .per_peer
                .values()
                .all(|resources| resources.fits(self.limits.per_peer))
            || !usage
                .replacement_history
                .fits(self.limits.replacement_history)
            || !usage.accepted.fits(self.limits.accepted)
        {
            return Err(ContinuousChargeError::ResourceLimit);
        }
        Ok(usage)
    }

    pub(super) fn plan_changes(
        &self,
        changes: &[ContinuousResourceChange],
    ) -> Result<Self, ContinuousChargeError> {
        let mut keys = std::collections::BTreeSet::new();
        for change in changes {
            if !keys.insert(change.key) {
                return Err(ContinuousChargeError::DuplicateChange);
            }
            change
                .expected
                .map(ContinuousChargeRecord::validate)
                .transpose()?;
            change
                .after
                .map(ContinuousChargeRecord::validate)
                .transpose()?;
            if self.charges.get(&change.key).copied() != change.expected {
                return Err(ContinuousChargeError::ExistingChargeMismatch);
            }
        }
        let mut charges = self.charges.clone();
        for change in changes {
            match change.after {
                Some(after) => {
                    charges.insert(change.key, after);
                }
                None => {
                    charges.remove(&change.key);
                }
            }
        }
        Self::new(self.limits, charges)
    }

    pub(super) fn plan_compute_release(
        &self,
        key: u8,
        expected: ContinuousChargeRecord,
        after: ContinuousChargeRecord,
    ) -> Result<Self, ContinuousChargeError> {
        let (
            ContinuousChargeRecord::PreAccepted {
                resources: old_resources,
                residency_peer: old_residency_peer,
                ..
            },
            ContinuousChargeRecord::PreAccepted {
                resources: new_resources,
                residency_peer: new_residency_peer,
                compute_peer: new_compute_peer,
            },
        ) = (expected.validate()?, after.validate()?)
        else {
            return Err(ContinuousChargeError::InvalidComputeRelease);
        };
        if old_resources.active_work != 1
            || new_resources.active_work != 0
            || old_residency_peer != new_residency_peer
            || new_compute_peer.is_some()
            || new_resources != old_resources.without_compute()
        {
            return Err(ContinuousChargeError::InvalidComputeRelease);
        }
        self.plan_changes(&[ContinuousResourceChange {
            key,
            expected: Some(expected),
            after: Some(after),
        }])
    }
}
