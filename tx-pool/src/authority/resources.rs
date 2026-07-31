use super::state::{ComputeAttribution, RawTxHash};
#[cfg(test)]
use super::state::{OwnedTx, PreAcceptedPhase, QueuedWork};
use ckb_network::PeerIndex;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct ResourceVector {
    pub(super) entries: usize,
    pub(super) bytes: usize,
    pub(super) edges: usize,
    pub(super) active_work: usize,
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
        }
    }

    pub(super) fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_add(other.entries)?,
            bytes: self.bytes.checked_add(other.bytes)?,
            edges: self.edges.checked_add(other.edges)?,
            active_work: self.active_work.checked_add(other.active_work)?,
        })
    }

    pub(super) fn checked_sub(self, other: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_sub(other.entries)?,
            bytes: self.bytes.checked_sub(other.bytes)?,
            edges: self.edges.checked_sub(other.edges)?,
            active_work: self.active_work.checked_sub(other.active_work)?,
        })
    }

    pub(super) fn fits(self, limit: Self) -> bool {
        self.entries <= limit.entries
            && self.bytes <= limit.bytes
            && self.edges <= limit.edges
            && self.active_work <= limit.active_work
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
    accepted: AcceptedResources,
    compute: ComputeLimits,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResourceConfigError {
    LimitHierarchy,
    MissingComputeCapacity,
    NonMonotonicComputeEnvelope,
    TransientComputeOverflow,
}

impl ResourceLimits {
    pub(super) fn new(
        preaccepted: ResourceVector,
        remote: ResourceVector,
        per_peer: ResourceVector,
        accepted: AcceptedResources,
        compute: ComputeLimits,
    ) -> Result<Self, ResourceConfigError> {
        if !remote.fits(preaccepted) || !per_peer.fits(remote) {
            return Err(ResourceConfigError::LimitHierarchy);
        }
        if compute.resolved_resident_bytes == 0
            || compute.accepted_resident_bytes == 0
            || (preaccepted.entries != 0 && preaccepted.active_work == 0)
            || (remote.entries != 0 && remote.active_work == 0)
            || (per_peer.entries != 0 && per_peer.active_work == 0)
        {
            return Err(ResourceConfigError::MissingComputeCapacity);
        }
        if compute.accepted_resident_bytes < compute.resolved_resident_bytes {
            return Err(ResourceConfigError::NonMonotonicComputeEnvelope);
        }
        for retained in [preaccepted, remote, per_peer] {
            compute
                .checked_physical_ceiling(retained)
                .ok_or(ResourceConfigError::TransientComputeOverflow)?;
        }
        Ok(Self {
            preaccepted,
            remote,
            per_peer,
            accepted,
            compute,
        })
    }

    #[cfg(test)]
    pub(super) fn with_accepted_for_foundation(mut self, accepted: AcceptedResources) -> Self {
        self.accepted = accepted;
        self
    }
}

/// Per-lease upper bounds reserved before attacker-shaped resolve/verify
/// facts can become retained authority state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ComputeLimits {
    resolved_resident_bytes: usize,
    accepted_resident_bytes: usize,
    expanded_edges: usize,
}

impl ComputeLimits {
    pub(super) const fn new(
        resolved_resident_bytes: usize,
        accepted_resident_bytes: usize,
        expanded_edges: usize,
    ) -> Self {
        Self {
            resolved_resident_bytes,
            accepted_resident_bytes,
            expanded_edges,
        }
    }

    pub(super) fn reservation_for(self, permit: super::state::WorkPermit) -> (usize, usize) {
        let resident_bytes = match permit {
            super::state::WorkPermit::ResolveOnly => self.resolved_resident_bytes,
            super::state::WorkPermit::VerifyOnly(_) => self.accepted_resident_bytes,
            super::state::WorkPermit::ResolveThenVerify(_) => self
                .resolved_resident_bytes
                .max(self.accepted_resident_bytes),
        };
        (resident_bytes, self.expanded_edges)
    }

    fn admits(self, resources: ResourceVector) -> bool {
        resources.entries == 1
            && resources.active_work == 0
            && resources.bytes <= self.resolved_resident_bytes
            && resources.bytes <= self.accepted_resident_bytes
            && resources.edges <= self.expanded_edges
    }

    fn checked_physical_ceiling(self, retained: ResourceVector) -> Option<(usize, usize)> {
        let max_resident_bytes = self
            .resolved_resident_bytes
            .max(self.accepted_resident_bytes);
        let transient_bytes = retained.active_work.checked_mul(max_resident_bytes)?;
        let transient_edges = retained.active_work.checked_mul(self.expanded_edges)?;
        Some((
            retained.bytes.checked_add(transient_bytes)?,
            retained.edges.checked_add(transient_edges)?,
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChargeRecord {
    PreAccepted {
        resources: ResourceVector,
        residency_peer: Option<PeerIndex>,
        compute_peer: Option<PeerIndex>,
    },
    Accepted(AcceptedResources),
}

impl ChargeRecord {
    fn preaccepted(self) -> Option<ResourceVector> {
        match self {
            Self::PreAccepted { resources, .. } => Some(resources),
            Self::Accepted(_) => None,
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
        let active_work = if compute_peer == Some(peer) {
            resources.active_work
        } else {
            0
        };
        Ok(Some((
            peer,
            ResourceVector::new(
                resources.entries,
                resources.bytes,
                resources.edges,
                active_work,
            ),
        )))
    }

    fn accepted(self) -> Option<AcceptedResources> {
        match self {
            Self::PreAccepted { .. } => None,
            Self::Accepted(resources) => Some(resources),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ResourceSnapshot {
    pub(super) charges: HashMap<RawTxHash, ChargeRecord>,
    pub(super) preaccepted: ResourceVector,
    pub(super) remote: ResourceVector,
    pub(super) peers: HashMap<PeerIndex, ResourceVector>,
    pub(super) accepted: AcceptedResources,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ResourceError {
    Arithmetic,
    PreAcceptedLimit,
    RemoteLimit,
    PeerLimit(PeerIndex),
    AcceptedLimit,
    ExistingChargeMismatch,
    DuplicateChange,
    ComputeEnvelope,
    AttributionMismatch,
    Allocation,
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
    accepted: AcceptedResources,
    limits: ResourceLimits,
}

pub(super) struct ResourcePlan {
    key: RawTxHash,
    after: Option<ChargeRecord>,
    preaccepted: ResourceVector,
    remote: ResourceVector,
    peer_updates: [Option<(PeerIndex, ResourceVector)>; 2],
    accepted: AcceptedResources,
}

pub(super) struct ResourceBatchPlan {
    changes: Vec<(RawTxHash, Option<ChargeRecord>, Option<ChargeRecord>)>,
    preaccepted: ResourceVector,
    remote: ResourceVector,
    peer_updates: HashMap<PeerIndex, ResourceVector>,
    accepted: AcceptedResources,
}

impl ResourceLedger {
    pub(super) fn new(limits: ResourceLimits) -> Self {
        Self {
            charges: HashMap::new(),
            preaccepted: ResourceVector::default(),
            remote: ResourceVector::default(),
            peers: HashMap::new(),
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

    pub(super) fn peer(&self, peer: PeerIndex) -> ResourceVector {
        self.peers.get(&peer).copied().unwrap_or_default()
    }

    pub(super) fn accepted(&self) -> AcceptedResources {
        self.accepted
    }

    pub(super) fn accepted_fits(&self, projected: AcceptedResources) -> bool {
        projected.fits(self.limits.accepted)
    }

    pub(super) fn compute_limits(&self) -> ComputeLimits {
        self.limits.compute
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

    pub(super) fn active_work_availability(
        &self,
        attribution: ComputeAttribution,
    ) -> Result<ActiveWorkAvailability, ResourceError> {
        let available = |used: usize, limit: usize| match used.cmp(&limit) {
            std::cmp::Ordering::Less => Ok(true),
            std::cmp::Ordering::Equal => Ok(false),
            std::cmp::Ordering::Greater => Err(ResourceError::Arithmetic),
        };
        if !available(
            self.preaccepted.active_work,
            self.limits.preaccepted.active_work,
        )? {
            return Ok(ActiveWorkAvailability::PreAcceptedExhausted);
        }
        let Some(peer) = attribution.peer() else {
            return Ok(ActiveWorkAvailability::Available);
        };
        if !available(self.remote.active_work, self.limits.remote.active_work)? {
            return Ok(ActiveWorkAvailability::RemoteExhausted);
        }
        if !available(
            self.peer(peer).active_work,
            self.limits.per_peer.active_work,
        )? {
            return Ok(ActiveWorkAvailability::PeerExhausted(peer));
        }
        Ok(ActiveWorkAvailability::Available)
    }

    pub(super) fn snapshot(&self) -> ResourceSnapshot {
        ResourceSnapshot {
            charges: self.charges.clone(),
            preaccepted: self.preaccepted,
            remote: self.remote,
            peers: self.peers.clone(),
            accepted: self.accepted,
        }
    }

    pub(super) fn charge_count(&self) -> usize {
        self.charges.len()
    }

    #[cfg(test)]
    pub(super) fn semantically_matches(&self, entries: &HashMap<RawTxHash, OwnedTx>) -> bool {
        let mut expected = ResourceSnapshot {
            charges: HashMap::new(),
            preaccepted: ResourceVector::default(),
            remote: ResourceVector::default(),
            peers: HashMap::new(),
            accepted: AcceptedResources::default(),
        };
        for (hash, owner) in entries {
            let charge = owner.charge_record();
            if expected.charges.insert(hash.clone(), charge).is_some() {
                return false;
            }
            match (owner, charge) {
                (
                    OwnedTx::PreAccepted(entry),
                    ChargeRecord::PreAccepted {
                        resources,
                        residency_peer,
                        compute_peer,
                    },
                ) => {
                    let exact_resources = match &entry.phase {
                        PreAcceptedPhase::Queued(QueuedWork::Verify(resolved)) => entry
                            .retained_charge(
                                resolved.payload().resolved_resident_bytes(),
                                resolved.payload().dependencies(),
                            ),
                        PreAcceptedPhase::Computing(active) => {
                            let (max_resident_bytes, max_edges) =
                                self.limits.compute.reservation_for(active.permit);
                            if active.grant.max_resident_bytes != max_resident_bytes
                                || active.grant.max_edges != max_edges
                            {
                                return false;
                            }
                            let mut exact = entry.retained_charge(
                                entry.original_charge().bytes,
                                &active.dependencies,
                            );
                            exact.active_work = 1;
                            exact
                        }
                        PreAcceptedPhase::Waiting(waiting) => {
                            let dependencies = match waiting {
                                super::state::WaitCondition::Missing(observed)
                                | super::state::WaitCondition::Conflict(observed) => {
                                    observed.retained()
                                }
                            };
                            entry.retained_charge(entry.original_charge().bytes, dependencies)
                        }
                        PreAcceptedPhase::Computed(super::state::ComputedOutcome::Verified(
                            verified,
                        )) => {
                            if verified.payload().resolved_resident_bytes()
                                > verified.metrics().cost.resident_bytes
                            {
                                return false;
                            }
                            entry.retained_charge(
                                verified.metrics().cost.resident_bytes,
                                verified.payload().dependencies(),
                            )
                        }
                        PreAcceptedPhase::Queued(QueuedWork::Resolve)
                        | PreAcceptedPhase::Computed(_) => entry.original_charge(),
                    };
                    if resources != exact_resources {
                        return false;
                    }
                    let expected_compute_peer = match &entry.phase {
                        PreAcceptedPhase::Computing(active) => active.attribution.peer(),
                        PreAcceptedPhase::Queued(_)
                        | PreAcceptedPhase::Waiting(_)
                        | PreAcceptedPhase::Computed(_) => None,
                    };
                    if residency_peer != entry.record.ingress.peer()
                        || compute_peer != expected_compute_peer
                    {
                        return false;
                    }
                    let Some(preaccepted) = expected.preaccepted.checked_add(resources) else {
                        return false;
                    };
                    expected.preaccepted = preaccepted;
                    let Ok(peer_charge) = charge.peer_preaccepted() else {
                        return false;
                    };
                    if let Some((peer, peer_resources)) = peer_charge {
                        let Some(remote) = expected.remote.checked_add(peer_resources) else {
                            return false;
                        };
                        expected.remote = remote;
                        let usage = expected.peers.entry(peer).or_default();
                        let Some(next) = usage.checked_add(peer_resources) else {
                            return false;
                        };
                        *usage = next;
                    }
                }
                (OwnedTx::Accepted(entry), ChargeRecord::Accepted(resources)) => {
                    if entry.proof.payload().serialized_bytes() != resources.serialized_bytes
                        || entry.proof.payload().resolved_resident_bytes()
                            > resources.resident_bytes
                        || entry.proof.metrics().cost
                            != (AcceptedCost {
                                serialized_bytes: resources.serialized_bytes,
                                resident_bytes: resources.resident_bytes,
                                cycles: resources.cycles,
                            })
                    {
                        return false;
                    }
                    let Some(accepted) = expected.accepted.checked_add(resources) else {
                        return false;
                    };
                    expected.accepted = accepted;
                }
                _ => return false,
            }
        }
        expected == self.snapshot()
            && self.preaccepted.fits(self.limits.preaccepted)
            && self.remote.fits(self.limits.remote)
            && self
                .peers
                .values()
                .all(|usage| usage.fits(self.limits.per_peer))
            && self.accepted.fits(self.limits.accepted)
    }

    pub(super) fn plan_replace(
        &mut self,
        key: RawTxHash,
        expected: Option<ChargeRecord>,
        after: Option<ChargeRecord>,
    ) -> Result<ResourcePlan, ResourceError> {
        if self.charge(&key) != expected {
            return Err(ResourceError::ExistingChargeMismatch);
        }

        let old_preaccepted = expected.and_then(ChargeRecord::preaccepted);
        let new_preaccepted = after.and_then(ChargeRecord::preaccepted);
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
        if !preaccepted.fits(self.limits.preaccepted) {
            return Err(ResourceError::PreAcceptedLimit);
        }
        if !remote.fits(self.limits.remote) {
            return Err(ResourceError::RemoteLimit);
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
            accepted,
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
        let mut accepted = self.accepted;
        let mut new_charge_count = 0usize;

        for (key, expected, after) in &changes {
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
