//! Checked continuous resource algebra used by production ledger properties.

// Continuous resource transition algebra. These types deliberately use a
// small checked domain so properties can exhaust invalid partial reservations;
// production refinement converts only representable usize/u64 inputs.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ContinuousResourceVector {
    pub(crate) entries: u16,
    pub(crate) bytes: u16,
    pub(crate) edges: u16,
    pub(crate) active_work: u16,
    pub(crate) compute_bytes: u16,
    pub(crate) compute_edges: u16,
}

impl ContinuousResourceVector {
    pub(crate) const fn retained(entries: u16, bytes: u16, edges: u16) -> Self {
        Self {
            entries,
            bytes,
            edges,
            active_work: 0,
            compute_bytes: 0,
            compute_edges: 0,
        }
    }

    pub(crate) fn reserve_compute(
        self,
        grant: ClaimComputeGrant,
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

    pub(crate) const fn without_compute(self) -> Self {
        Self {
            active_work: 0,
            compute_bytes: 0,
            compute_edges: 0,
            ..self
        }
    }

    pub(crate) const fn has_compute_reservation(self) -> bool {
        self.active_work != 0 || self.compute_bytes != 0 || self.compute_edges != 0
    }

    pub(crate) fn total_bytes(self) -> Option<u16> {
        self.bytes.checked_add(self.compute_bytes)
    }

    pub(crate) fn total_edges(self) -> Option<u16> {
        self.edges.checked_add(self.compute_edges)
    }

    pub(crate) fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_add(other.entries)?,
            bytes: self.bytes.checked_add(other.bytes)?,
            edges: self.edges.checked_add(other.edges)?,
            active_work: self.active_work.checked_add(other.active_work)?,
            compute_bytes: self.compute_bytes.checked_add(other.compute_bytes)?,
            compute_edges: self.compute_edges.checked_add(other.compute_edges)?,
        })
    }

    pub(crate) fn checked_sub(self, other: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_sub(other.entries)?,
            bytes: self.bytes.checked_sub(other.bytes)?,
            edges: self.edges.checked_sub(other.edges)?,
            active_work: self.active_work.checked_sub(other.active_work)?,
            compute_bytes: self.compute_bytes.checked_sub(other.compute_bytes)?,
            compute_edges: self.compute_edges.checked_sub(other.compute_edges)?,
        })
    }

    pub(crate) fn fits(self, limit: Self) -> bool {
        self.entries <= limit.entries
            && self.bytes <= limit.bytes
            && self.edges <= limit.edges
            && self.active_work <= limit.active_work
            && self.compute_bytes <= limit.compute_bytes
            && self.compute_edges <= limit.compute_edges
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClaimComputeGrant {
    pub(crate) total_retained_bytes: u16,
    pub(crate) edges: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ContinuousComputeLimits {
    pub(crate) resolved_total_retained_bytes: u16,
    pub(crate) accepted_total_retained_bytes: u16,
    pub(crate) expanded_edges: u16,
}

impl ContinuousComputeLimits {
    pub(crate) const fn max_total_retained_bytes(self) -> u16 {
        if self.resolved_total_retained_bytes > self.accepted_total_retained_bytes {
            self.resolved_total_retained_bytes
        } else {
            self.accepted_total_retained_bytes
        }
    }

    pub(crate) const fn admits(self, resources: ContinuousResourceVector) -> bool {
        resources.entries == 1
            && !resources.has_compute_reservation()
            && resources.bytes <= self.resolved_total_retained_bytes
            && resources.bytes <= self.accepted_total_retained_bytes
            && resources.edges <= self.expanded_edges
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ContinuousAcceptedResources {
    pub(crate) entries: u16,
    pub(crate) serialized_bytes: u16,
    pub(crate) resident_bytes: u16,
    pub(crate) cycles: u16,
}

impl ContinuousAcceptedResources {
    pub(crate) fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_add(other.entries)?,
            serialized_bytes: self.serialized_bytes.checked_add(other.serialized_bytes)?,
            resident_bytes: self.resident_bytes.checked_add(other.resident_bytes)?,
            cycles: self.cycles.checked_add(other.cycles)?,
        })
    }

    pub(crate) fn checked_sub(self, other: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_sub(other.entries)?,
            serialized_bytes: self.serialized_bytes.checked_sub(other.serialized_bytes)?,
            resident_bytes: self.resident_bytes.checked_sub(other.resident_bytes)?,
            cycles: self.cycles.checked_sub(other.cycles)?,
        })
    }

    pub(crate) fn fits(self, limit: Self) -> bool {
        self.entries <= limit.entries
            && self.serialized_bytes <= limit.serialized_bytes
            && self.resident_bytes <= limit.resident_bytes
            && self.cycles <= limit.cycles
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ContinuousResourceLimits {
    pub(crate) preaccepted: ContinuousResourceVector,
    pub(crate) remote: ContinuousResourceVector,
    pub(crate) per_peer: ContinuousResourceVector,
    pub(crate) replacement_history: ContinuousResourceVector,
    pub(crate) accepted: ContinuousAcceptedResources,
    pub(crate) compute: ContinuousComputeLimits,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContinuousResourceConfigError {
    LimitHierarchy,
    MissingComputeCapacity,
    NonMonotonicComputeEnvelope,
    TransientComputeOverflow,
}

impl ContinuousResourceLimits {
    pub(crate) fn validate(
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
pub(crate) enum ContinuousChargeRecord {
    PreAccepted {
        resources: ContinuousResourceVector,
        residency_peer: Option<u8>,
        compute_peer: Option<u8>,
    },
    ReplacementHistory(ContinuousResourceVector),
    Accepted(ContinuousAcceptedResources),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContinuousChargeError {
    ComputeEnvelope,
    AttributionMismatch,
    Arithmetic,
    ExistingChargeMismatch,
    DuplicateChange,
    ResourceLimit,
    InvalidComputeRelease,
}

impl ContinuousChargeRecord {
    pub(crate) fn validate(self) -> Result<Self, ContinuousChargeError> {
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
pub(crate) struct ContinuousResourceChange {
    pub(crate) key: u8,
    pub(crate) expected: Option<ContinuousChargeRecord>,
    pub(crate) after: Option<ContinuousChargeRecord>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ContinuousResourceUsage {
    pub(crate) preaccepted: ContinuousResourceVector,
    pub(crate) remote: ContinuousResourceVector,
    pub(crate) per_peer: std::collections::BTreeMap<u8, ContinuousResourceVector>,
    pub(crate) replacement_history: ContinuousResourceVector,
    pub(crate) accepted: ContinuousAcceptedResources,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ContinuousResourceLedger {
    limits: ContinuousResourceLimits,
    charges: std::collections::BTreeMap<u8, ContinuousChargeRecord>,
}

impl ContinuousResourceLedger {
    pub(crate) fn new(
        limits: ContinuousResourceLimits,
        charges: std::collections::BTreeMap<u8, ContinuousChargeRecord>,
    ) -> Result<Self, ContinuousChargeError> {
        let ledger = Self { limits, charges };
        ledger.usage()?;
        Ok(ledger)
    }

    pub(crate) fn charges(&self) -> &std::collections::BTreeMap<u8, ContinuousChargeRecord> {
        &self.charges
    }

    pub(crate) fn usage(&self) -> Result<ContinuousResourceUsage, ContinuousChargeError> {
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

    pub(crate) fn plan_changes(
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

    pub(crate) fn plan_compute_release(
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
