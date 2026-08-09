use super::super::state::{OwnedTx, PreAcceptedPhase, QueuedWork};
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) struct ResourceSnapshot {
    pub(in crate::authority) charges: HashMap<RawTxHash, ChargeRecord>,
    pub(in crate::authority) preaccepted: ResourceVector,
    pub(in crate::authority) remote: ResourceVector,
    pub(in crate::authority) peers: HashMap<PeerIndex, ResourceVector>,
    pub(in crate::authority) replacement_history: ResourceVector,
    pub(in crate::authority) accepted: AcceptedResources,
}

impl ResidencyPolicy {
    pub(in crate::authority) const fn foundation() -> Self {
        Self {
            entry_metadata_bytes: 0,
            edge_metadata_bytes: 0,
        }
    }
}

impl ComputeGrant {
    pub(in crate::authority) const fn for_foundation(
        max_total_retained_bytes: usize,
        max_edges: usize,
    ) -> Self {
        Self {
            max_total_retained_bytes,
            max_edges,
            payload_bytes: 0,
            encoded_edges: 0,
            residency: ResidencyPolicy::foundation(),
        }
    }
}

impl ResourceVector {
    pub(in crate::authority) const fn compute_bytes(self) -> usize {
        self.compute_bytes
    }

    pub(in crate::authority) const fn compute_edges(self) -> usize {
        self.compute_edges
    }
}

impl ResourceLedger {
    /// Sequential-checkout resource probe retained only for refinement tests.
    /// Production compute plans against `OrderedResourceProjection` so a
    /// bounded wave observes earlier members of the same exchange.
    pub(in crate::authority) fn active_work_availability_for_reference(
        &self,
        attribution: ComputeAttribution,
    ) -> Result<ActiveWorkAvailability, ResourceError> {
        active_work_availability(
            self.preaccepted,
            self.remote,
            attribution.peer().map(|peer| (peer, self.peer(peer))),
            self.limits,
        )
    }
}

impl ComputeLimits {
    fn checked_compute_capacity(self, active_work: usize) -> Option<(usize, usize)> {
        Some((
            active_work.checked_mul(self.max_total_retained_bytes())?,
            active_work.checked_mul(self.expanded_edges)?,
        ))
    }
}

impl ResourceLimits {
    pub(in crate::authority) fn new(
        preaccepted: ResourceVector,
        remote: ResourceVector,
        per_peer: ResourceVector,
        accepted: AcceptedResources,
        compute: ComputeLimits,
    ) -> Result<Self, ResourceConfigError> {
        let attach_compute_partition = |limit: ResourceVector| {
            let (bytes, edges) = compute
                .checked_compute_capacity(limit.active_work)
                .ok_or(ResourceConfigError::TransientComputeOverflow)?;
            limit
                .with_compute_capacity(bytes, edges)
                .ok_or(ResourceConfigError::TransientComputeOverflow)
        };
        Self::with_residency_policy(
            attach_compute_partition(preaccepted)?,
            attach_compute_partition(remote)?,
            attach_compute_partition(per_peer)?,
            accepted,
            compute,
            ResidencyPolicy::foundation(),
        )
    }

    pub(in crate::authority) fn with_accepted_for_foundation(
        mut self,
        accepted: AcceptedResources,
    ) -> Self {
        self.accepted = accepted;
        self
    }

    pub(in crate::authority) const fn preaccepted_limit_for_foundation(self) -> ResourceVector {
        self.preaccepted
    }
}

impl ResourceLedger {
    pub(in crate::authority) fn snapshot(&self) -> ResourceSnapshot {
        ResourceSnapshot {
            charges: self.charges.clone(),
            preaccepted: self.preaccepted,
            remote: self.remote,
            peers: self.peers.clone(),
            replacement_history: self.replacement_history,
            accepted: self.accepted,
        }
    }

    pub(in crate::authority) fn charge_count(&self) -> usize {
        self.charges.len()
    }

    pub(in crate::authority) fn semantically_matches(
        &self,
        entries: &HashMap<RawTxHash, OwnedTx>,
    ) -> bool {
        let mut expected = ResourceSnapshot {
            charges: HashMap::new(),
            preaccepted: ResourceVector::default(),
            remote: ResourceVector::default(),
            peers: HashMap::new(),
            replacement_history: ResourceVector::default(),
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
                        PreAcceptedPhase::Queued(QueuedWork::Verify(resolved)) => {
                            let Ok(charge) = self.retained_entry_charge(
                                entry,
                                resolved.payload().resolved_resident_bytes(),
                                resolved.payload().dependencies().len(),
                            ) else {
                                return false;
                            };
                            charge
                        }
                        PreAcceptedPhase::Computing(active) => {
                            if active.grant != self.compute_grant(entry, active.permit) {
                                return false;
                            }
                            let Some(exact) = active.grant.retained_charge(
                                entry.basis.payload_bytes(),
                                active.dependencies.len(),
                            ) else {
                                return false;
                            };
                            let Some(exact) = exact.reserve_compute(active.grant) else {
                                return false;
                            };
                            exact
                        }
                        PreAcceptedPhase::Waiting(observed) => {
                            let Ok(charge) = self.retained_entry_charge(
                                entry,
                                entry.basis.payload_bytes(),
                                observed.retained().len(),
                            ) else {
                                return false;
                            };
                            charge
                        }
                        PreAcceptedPhase::Ready(verified) => {
                            if verified.payload().resolved_resident_bytes()
                                > verified.metrics().cost.resident_bytes
                            {
                                return false;
                            }
                            let Ok(charge) = self.retained_entry_charge(
                                entry,
                                verified.metrics().cost.resident_bytes,
                                verified.payload().dependencies().len(),
                            ) else {
                                return false;
                            };
                            charge
                        }
                        PreAcceptedPhase::Queued(QueuedWork::Resolve) => entry.original_charge(),
                    };
                    if resources != exact_resources {
                        return false;
                    }
                    let expected_compute_peer = match &entry.phase {
                        PreAcceptedPhase::Computing(active) => active.attribution.peer(),
                        PreAcceptedPhase::Queued(_)
                        | PreAcceptedPhase::Waiting(_)
                        | PreAcceptedPhase::Ready(_) => None,
                    };
                    if residency_peer != entry.source.ingress_peer()
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
                (
                    OwnedTx::ReplacementHistory(entry),
                    ChargeRecord::ReplacementHistory(resources),
                ) => {
                    let recovery = entry.recovery_charge();
                    if resources != entry.charge()
                        || resources.entries != 1
                        || resources.active_work != 0
                        || resources.bytes
                            != recovery.bytes.max(entry.record().tx.data().total_size())
                        || resources.edges != recovery.edges.max(entry.dependencies().len())
                    {
                        return false;
                    }
                    let Some(preaccepted) = expected.preaccepted.checked_add(resources) else {
                        return false;
                    };
                    let Some(replacement_history) =
                        expected.replacement_history.checked_add(resources)
                    else {
                        return false;
                    };
                    expected.preaccepted = preaccepted;
                    expected.replacement_history = replacement_history;
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
                .replacement_history
                .fits(self.limits.replacement_history)
            && self
                .peers
                .values()
                .all(|usage| usage.fits(self.limits.per_peer))
            && self.accepted.fits(self.limits.accepted)
    }
}
