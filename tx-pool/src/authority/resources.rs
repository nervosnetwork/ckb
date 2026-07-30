use super::state::RawTxHash;
use ckb_network::PeerIndex;
use std::collections::HashMap;

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

    fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_add(other.entries)?,
            bytes: self.bytes.checked_add(other.bytes)?,
            edges: self.edges.checked_add(other.edges)?,
            active_work: self.active_work.checked_add(other.active_work)?,
        })
    }

    fn checked_sub(self, other: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_sub(other.entries)?,
            bytes: self.bytes.checked_sub(other.bytes)?,
            edges: self.edges.checked_sub(other.edges)?,
            active_work: self.active_work.checked_sub(other.active_work)?,
        })
    }

    fn fits(self, limit: Self) -> bool {
        self.entries <= limit.entries
            && self.bytes <= limit.bytes
            && self.edges <= limit.edges
            && self.active_work <= limit.active_work
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ResourceLimits {
    pub(super) total: ResourceVector,
    pub(super) remote: ResourceVector,
    pub(super) per_peer: ResourceVector,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ResourceSnapshot {
    pub(super) charges: HashMap<RawTxHash, ChargeRecord>,
    pub(super) total: ResourceVector,
    pub(super) remote: ResourceVector,
    pub(super) peers: HashMap<PeerIndex, ResourceVector>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ChargeRecord {
    pub(super) resources: ResourceVector,
    pub(super) peer: Option<PeerIndex>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ResourceError {
    Arithmetic,
    TotalLimit,
    RemoteLimit,
    PeerLimit(PeerIndex),
    ExistingChargeMismatch,
    Allocation,
}

#[derive(Debug)]
pub(super) struct ResourceLedger {
    charges: HashMap<RawTxHash, ChargeRecord>,
    total: ResourceVector,
    remote: ResourceVector,
    peers: HashMap<PeerIndex, ResourceVector>,
    limits: ResourceLimits,
}

pub(super) struct ResourcePlan {
    key: RawTxHash,
    after: Option<ChargeRecord>,
    total: ResourceVector,
    remote: ResourceVector,
    peer_updates: [Option<(PeerIndex, ResourceVector)>; 2],
}

impl ResourceLedger {
    pub(super) fn new(limits: ResourceLimits) -> Self {
        Self {
            charges: HashMap::new(),
            total: ResourceVector::default(),
            remote: ResourceVector::default(),
            peers: HashMap::new(),
            limits,
        }
    }

    pub(super) fn charge(&self, key: &RawTxHash) -> Option<ChargeRecord> {
        self.charges.get(key).copied()
    }

    pub(super) fn total(&self) -> ResourceVector {
        self.total
    }

    pub(super) fn remote(&self) -> ResourceVector {
        self.remote
    }

    pub(super) fn peer(&self, peer: PeerIndex) -> ResourceVector {
        self.peers.get(&peer).copied().unwrap_or_default()
    }

    pub(super) fn snapshot(&self) -> ResourceSnapshot {
        ResourceSnapshot {
            charges: self.charges.clone(),
            total: self.total,
            remote: self.remote,
            peers: self.peers.clone(),
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

        let old = expected.unwrap_or(ChargeRecord {
            resources: ResourceVector::default(),
            peer: None,
        });
        let new = after.unwrap_or(ChargeRecord {
            resources: ResourceVector::default(),
            peer: None,
        });
        let total = self
            .total
            .checked_sub(old.resources)
            .and_then(|usage| usage.checked_add(new.resources))
            .ok_or(ResourceError::Arithmetic)?;
        if !total.fits(self.limits.total) {
            return Err(ResourceError::TotalLimit);
        }

        let mut remote = self.remote;
        if old.peer.is_some() {
            remote = remote
                .checked_sub(old.resources)
                .ok_or(ResourceError::Arithmetic)?;
        }
        if new.peer.is_some() {
            remote = remote
                .checked_add(new.resources)
                .ok_or(ResourceError::Arithmetic)?;
        }
        if !remote.fits(self.limits.remote) {
            return Err(ResourceError::RemoteLimit);
        }

        let project_peer = |peer: PeerIndex| {
            let mut usage = self.peer(peer);
            if old.peer == Some(peer) {
                usage = usage
                    .checked_sub(old.resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
            if new.peer == Some(peer) {
                usage = usage
                    .checked_add(new.resources)
                    .ok_or(ResourceError::Arithmetic)?;
            }
            if !usage.fits(self.limits.per_peer) {
                return Err(ResourceError::PeerLimit(peer));
            }
            Ok(usage)
        };

        let old_update = old
            .peer
            .map(|peer| project_peer(peer).map(|usage| (peer, usage)))
            .transpose()?;
        let new_update = new
            .peer
            .filter(|peer| Some(*peer) != old.peer)
            .map(|peer| project_peer(peer).map(|usage| (peer, usage)))
            .transpose()?;

        if expected.is_none() && after.is_some() {
            self.charges
                .try_reserve(1)
                .map_err(|_| ResourceError::Allocation)?;
        }
        if new.peer.is_some_and(|peer| !self.peers.contains_key(&peer)) {
            self.peers
                .try_reserve(1)
                .map_err(|_| ResourceError::Allocation)?;
        }

        Ok(ResourcePlan {
            key,
            after,
            total,
            remote,
            peer_updates: [old_update, new_update],
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
        self.total = plan.total;
        self.remote = plan.remote;
        for (peer, usage) in plan.peer_updates.into_iter().flatten() {
            if usage == ResourceVector::default() {
                self.peers.remove(&peer);
            } else {
                self.peers.insert(peer, usage);
            }
        }
    }
}
