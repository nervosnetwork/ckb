use super::state::RawTxHash;
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
    pub(super) preaccepted: ResourceVector,
    pub(super) remote: ResourceVector,
    pub(super) per_peer: ResourceVector,
    pub(super) accepted: AcceptedResources,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChargeRecord {
    PreAccepted {
        resources: ResourceVector,
        peer: Option<PeerIndex>,
    },
    Accepted(AcceptedResources),
}

impl ChargeRecord {
    fn preaccepted(self) -> Option<(ResourceVector, Option<PeerIndex>)> {
        match self {
            Self::PreAccepted { resources, peer } => Some((resources, peer)),
            Self::Accepted(_) => None,
        }
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
    Allocation,
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
        let mut preaccepted = self.preaccepted;
        let mut remote = self.remote;
        if let Some((resources, peer)) = old_preaccepted {
            preaccepted = preaccepted
                .checked_sub(resources)
                .ok_or(ResourceError::Arithmetic)?;
            if peer.is_some() {
                remote = remote
                    .checked_sub(resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
        }
        if let Some((resources, peer)) = new_preaccepted {
            preaccepted = preaccepted
                .checked_add(resources)
                .ok_or(ResourceError::Arithmetic)?;
            if peer.is_some() {
                remote = remote
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

        let old_peer = old_preaccepted.and_then(|(_, peer)| peer);
        let new_peer = new_preaccepted.and_then(|(_, peer)| peer);
        let project_peer = |peer: PeerIndex| {
            let mut usage = self.peer(peer);
            if old_peer == Some(peer) {
                let resources = old_preaccepted
                    .map(|(resources, _)| resources)
                    .ok_or(ResourceError::Arithmetic)?;
                usage = usage
                    .checked_sub(resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
            if new_peer == Some(peer) {
                let resources = new_preaccepted
                    .map(|(resources, _)| resources)
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
            if let Some((resources, peer)) = expected.and_then(ChargeRecord::preaccepted) {
                preaccepted = preaccepted
                    .checked_sub(resources)
                    .ok_or(ResourceError::Arithmetic)?;
                if let Some(peer) = peer {
                    remote = remote
                        .checked_sub(resources)
                        .ok_or(ResourceError::Arithmetic)?;
                    let usage = peer_updates.entry(peer).or_insert_with(|| self.peer(peer));
                    *usage = usage
                        .checked_sub(resources)
                        .ok_or(ResourceError::Arithmetic)?;
                }
            }
            if let Some(resources) = expected.and_then(ChargeRecord::accepted) {
                accepted = accepted
                    .checked_sub(resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
        }
        for (_, _, after) in &changes {
            if let Some((resources, peer)) = after.and_then(ChargeRecord::preaccepted) {
                preaccepted = preaccepted
                    .checked_add(resources)
                    .ok_or(ResourceError::Arithmetic)?;
                if let Some(peer) = peer {
                    remote = remote
                        .checked_add(resources)
                        .ok_or(ResourceError::Arithmetic)?;
                    let usage = peer_updates.entry(peer).or_insert_with(|| self.peer(peer));
                    *usage = usage
                        .checked_add(resources)
                        .ok_or(ResourceError::Arithmetic)?;
                }
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
