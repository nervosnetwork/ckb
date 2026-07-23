//! Single authoritative owner for the production pre-pool pipeline.
//!
//! Lifecycle state, payload phase, worker leases, dependency/conflict edges,
//! queue tickets and residency accounting share one incarnation/revision and
//! one short-held transition lock.

use ckb_network::PeerIndex;
use ckb_types::packed::{Byte32, OutPoint, ProposalShortId};
use ckb_types::prelude::Entity;
use std::cmp::Ordering;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;

mod audit;
mod indexes;
mod types;
pub(crate) use types::*;
#[derive(Debug)]
pub(crate) struct PipelineCoordinator<R, U, V> {
    entries: HashMap<Byte32, CoordinatorEntry<R, U, V>>,
    by_short_id: HashMap<ProposalShortId, Byte32>,
    by_peer: HashMap<PeerIndex, HashSet<Byte32>>,
    by_parent: HashMap<Byte32, HashSet<Byte32>>,
    dependency_failures: VecDeque<Byte32>,
    dependency_failure_set: HashSet<Byte32>,
    candidates_by_input: HashMap<OutPoint, HashSet<Byte32>>,
    active_by_input: HashMap<OutPoint, Byte32>,
    waiters_by_blocker: HashMap<Byte32, HashSet<Byte32>>,
    conflict_rechecks: VecDeque<Byte32>,
    conflict_recheck_set: HashSet<Byte32>,
    conflict_edge_count: usize,
    queues: HashMap<QueueKind, TicketQueue>,
    deadlines: BinaryHeap<Reverse<DeadlineTicket>>,
    live_deadlines: HashMap<Byte32, DeadlineTicket>,
    global_usage: CoordinatorResidency,
    peer_usage: HashMap<PeerIndex, CoordinatorResidency>,
    active_work: usize,
    active_work_by_peer: HashMap<PeerIndex, usize>,
    limits: CoordinatorLimits,
    next_incarnation: u64,
    next_arrival: u64,
    next_maintenance_sequence: u64,
    next_queue_sequence: u64,
    #[cfg(test)]
    fault_after_apply_steps: Option<usize>,
    #[cfg(test)]
    apply_steps_seen: usize,
}

enum CapacitySubject {
    Absent(Byte32),
    Present(Byte32),
}

impl<R, U, V> PipelineCoordinator<R, U, V> {
    pub(crate) fn new(limits: CoordinatorLimits) -> Self {
        let verify_ordering = match limits.verify_ordering {
            CoordinatorVerifyOrdering::ArrivalTime => QueueOrdering::Fifo,
            CoordinatorVerifyOrdering::FeeRate => QueueOrdering::FeeRate,
        };
        Self {
            entries: HashMap::new(),
            by_short_id: HashMap::new(),
            by_peer: HashMap::new(),
            by_parent: HashMap::new(),
            dependency_failures: VecDeque::new(),
            dependency_failure_set: HashSet::new(),
            candidates_by_input: HashMap::new(),
            active_by_input: HashMap::new(),
            waiters_by_blocker: HashMap::new(),
            conflict_rechecks: VecDeque::new(),
            conflict_recheck_set: HashSet::new(),
            conflict_edge_count: 0,
            queues: HashMap::from([
                (QueueKind::PreCheck, TicketQueue::new(QueueOrdering::Fifo)),
                (QueueKind::Resolve, TicketQueue::new(QueueOrdering::Fifo)),
                (QueueKind::Verify, TicketQueue::new(verify_ordering)),
                (QueueKind::Commit, TicketQueue::new(QueueOrdering::Fifo)),
            ]),
            deadlines: BinaryHeap::new(),
            live_deadlines: HashMap::new(),
            global_usage: CoordinatorResidency::default(),
            peer_usage: HashMap::new(),
            active_work: 0,
            active_work_by_peer: HashMap::new(),
            limits,
            next_incarnation: 1,
            next_arrival: 0,
            next_maintenance_sequence: 0,
            next_queue_sequence: 0,
            #[cfg(test)]
            fault_after_apply_steps: None,
            #[cfg(test)]
            apply_steps_seen: 0,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn usage(&self) -> CoordinatorResidency {
        self.global_usage
    }

    pub(crate) fn peer_usage(&self, peer: PeerIndex) -> CoordinatorResidency {
        self.peer_usage.get(&peer).copied().unwrap_or_default()
    }

    pub(crate) fn peer_hashes(&self, peer: PeerIndex, max: usize) -> Vec<Byte32> {
        let mut hashes: Vec<_> = self
            .by_peer
            .get(&peer)
            .into_iter()
            .flat_map(|hashes| hashes.iter())
            .filter(|hash| {
                self.entries.get(*hash).is_some_and(|entry| {
                    !matches!(entry.view().location, CoordinatorLocation::Committing)
                })
            })
            .cloned()
            .collect();
        hashes.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        hashes.truncate(max);
        hashes
    }

    pub(crate) fn view(&self, hash: &Byte32) -> Option<CoordinatorView> {
        self.entries.get(hash).map(CoordinatorEntry::view)
    }

    pub(crate) fn contains_hash(&self, hash: &Byte32) -> bool {
        self.entries.contains_key(hash)
    }

    pub(crate) fn raw_by_hash(&self, hash: &Byte32) -> Option<Arc<R>> {
        self.entries
            .get(hash)
            .map(|entry| Arc::clone(entry.state.raw()))
    }

    pub(crate) fn raw_by_short_id(&self, short_id: &ProposalShortId) -> Option<Arc<R>> {
        self.by_short_id
            .get(short_id)
            .and_then(|hash| self.raw_by_hash(hash))
    }

    pub(crate) fn unverified_by_short_id(&self, short_id: &ProposalShortId) -> Option<Arc<U>> {
        let hash = self.by_short_id.get(short_id)?;
        match &self.entries.get(hash)?.state {
            EntryState::Unverified { payload, .. }
            | EntryState::Invalidated {
                payload: InvalidatedPayload::Unverified(payload),
                ..
            } => Some(Arc::clone(payload)),
            _ => None,
        }
    }

    pub(crate) fn verified_by_short_id(&self, short_id: &ProposalShortId) -> Option<Arc<V>> {
        let hash = self.by_short_id.get(short_id)?;
        match &self.entries.get(hash)?.state {
            EntryState::PlainVerified { payload, .. }
            | EntryState::CandidateVerified { payload, .. }
            | EntryState::Invalidated {
                payload: InvalidatedPayload::Verified(payload),
                ..
            } => Some(Arc::clone(payload)),
            _ => None,
        }
    }

    pub(crate) fn hash_by_short_id(&self, short_id: &ProposalShortId) -> Option<&Byte32> {
        self.by_short_id.get(short_id)
    }

    pub(crate) fn queue_len(&self, kind: QueueKind) -> usize {
        self.queues.get(&kind).map_or(0, |queue| queue.live.len())
    }

    pub(crate) fn waiting_parent_len(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| {
                matches!(
                    entry.view().location,
                    CoordinatorLocation::WaitingParents { .. }
                )
            })
            .count()
    }

    pub(crate) fn conflict_recheck_len(&self) -> usize {
        self.conflict_recheck_set.len()
    }

    pub(crate) fn active_conflict_owner(&self, input: &OutPoint) -> Option<&Byte32> {
        self.active_by_input.get(input)
    }

    pub(crate) fn conflict_edge_count(&self) -> usize {
        self.conflict_edge_count
    }

    pub(crate) fn deadline_len(&self) -> usize {
        self.live_deadlines.len()
    }

    pub(crate) fn active_work(&self) -> usize {
        self.active_work
    }

    pub(crate) fn peer_active_work(&self, peer: PeerIndex) -> usize {
        self.active_work_by_peer.get(&peer).copied().unwrap_or(0)
    }

    pub(crate) fn admit_raw(
        &mut self,
        hash: Byte32,
        short_id: ProposalShortId,
        raw: R,
        initial_stage: RawStage,
        peer: Option<PeerIndex>,
        charge_bytes: usize,
        dependencies: HashSet<Byte32>,
    ) -> Result<(CoordinatorVersion, Vec<TerminalRecord<R, U, V>>), CoordinatorError> {
        let source = peer.map_or(CoordinatorSource::Local, CoordinatorSource::Remote);
        self.admit_raw_sourced(
            hash,
            short_id,
            raw,
            initial_stage,
            source,
            None,
            charge_bytes,
            dependencies,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn admit_raw_sourced(
        &mut self,
        hash: Byte32,
        short_id: ProposalShortId,
        raw: R,
        initial_stage: RawStage,
        source: CoordinatorSource,
        expires_at: Option<u64>,
        charge_bytes: usize,
        dependencies: HashSet<Byte32>,
    ) -> Result<(CoordinatorVersion, Vec<TerminalRecord<R, U, V>>), CoordinatorError> {
        if self.entries.contains_key(&hash) {
            return Err(CoordinatorError::DuplicateHash(hash));
        }
        if let Some(existing_hash) = self.by_short_id.get(&short_id) {
            return Err(CoordinatorError::ShortIdCollision {
                short_id,
                existing_hash: existing_hash.clone(),
            });
        }
        if dependencies.contains(&hash) {
            return Err(CoordinatorError::SelfDependency(hash));
        }
        if dependencies.len() > self.limits.max_dependencies_per_entry {
            return Err(CoordinatorError::DependencyLimitExceeded);
        }
        let protected = self.dependency_ancestor_closure(&hash, &dependencies)?;
        let mut victims = self.dependency_capacity_victims(source, &dependencies, &protected)?;
        let base_metadata_bytes =
            self.metadata_charge_bytes(dependencies.len(), expires_at.is_some(), 0)?;
        let incoming_charge_bytes = charge_bytes
            .checked_add(base_metadata_bytes)
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        let selected: HashSet<_> = victims.iter().cloned().collect();
        self.check_peer_budget_after_victims(None, source, incoming_charge_bytes, &selected)?;
        victims.extend(self.global_capacity_victims(
            None,
            source,
            incoming_charge_bytes,
            &selected,
            &protected,
        )?);
        victims.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        victims.dedup();
        let subject = CapacitySubject::Absent(hash.clone());
        self.with_capacity_victims(subject, victims, move |coordinator| {
            coordinator.admit_raw_sourced_inner(
                hash,
                short_id,
                raw,
                initial_stage,
                source,
                expires_at,
                charge_bytes,
                dependencies,
            )
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn admit_raw_sourced_inner(
        &mut self,
        hash: Byte32,
        short_id: ProposalShortId,
        raw: R,
        initial_stage: RawStage,
        source: CoordinatorSource,
        expires_at: Option<u64>,
        charge_bytes: usize,
        dependencies: HashSet<Byte32>,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        if self.entries.contains_key(&hash) {
            return Err(CoordinatorError::DuplicateHash(hash));
        }
        if let Some(existing_hash) = self.by_short_id.get(&short_id) {
            return Err(CoordinatorError::ShortIdCollision {
                short_id,
                existing_hash: existing_hash.clone(),
            });
        }
        if dependencies.contains(&hash) {
            return Err(CoordinatorError::SelfDependency(hash));
        }
        if dependencies.len() > self.limits.max_dependencies_per_entry {
            return Err(CoordinatorError::DependencyLimitExceeded);
        }
        for parent in &dependencies {
            if self
                .by_parent
                .get(parent)
                .map_or(0, HashSet::len)
                .saturating_add(1)
                > self.limits.max_dependents_per_parent
            {
                return Err(CoordinatorError::ParentFanoutLimitExceeded(parent.clone()));
            }
        }
        let base_metadata_bytes =
            self.metadata_charge_bytes(dependencies.len(), expires_at.is_some(), 0)?;
        let total_charge_bytes = charge_bytes
            .checked_add(base_metadata_bytes)
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        let charge = CoordinatorResidency::new(1, total_charge_bytes);
        let peer = source.peer();
        self.check_add_budget(peer, charge)?;
        let incarnation = self.next_incarnation;
        let next_incarnation = incarnation
            .checked_add(1)
            .ok_or(CoordinatorError::IncarnationExhausted)?;
        let (queue_sequence, next_queue_sequence) = self.queue_sequence_range(1)?;
        let queue_kind = match initial_stage {
            RawStage::PreCheck => QueueKind::PreCheck,
            RawStage::Resolve => QueueKind::Resolve,
        };
        self.queue_mut(queue_kind)?
            .reserve_live(source.is_proposal(), source.queue_owner())?;
        if expires_at.is_some() {
            self.deadlines
                .try_reserve(1)
                .map_err(|_| CoordinatorError::QueueReservationFailed)?;
            self.live_deadlines
                .try_reserve(1)
                .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        }

        let entry = CoordinatorEntry {
            short_id: short_id.clone(),
            state: EntryState::Raw {
                raw: Arc::new(raw),
                location: RawLocation::Queued(initial_stage),
            },
            source,
            expires_at,
            raw_charge_bytes: total_charge_bytes,
            raw_resident_payload_bytes: charge_bytes,
            resident_payload_bytes: charge_bytes,
            base_metadata_bytes,
            metadata_bytes: base_metadata_bytes,
            charge_bytes: total_charge_bytes,
            dependencies: dependencies.clone(),
            incarnation,
            revision: 0,
            deadline_generation: 0,
            queue_sequence,
            verify_schedule: VerifySchedule::default(),
        };
        let ticket = entry.ticket(&hash);
        self.next_incarnation = next_incarnation;
        self.next_queue_sequence = next_queue_sequence;
        self.global_usage = self
            .global_usage
            .checked_add(charge)
            .ok_or(CoordinatorError::GlobalBudgetExceeded)?;
        if let Some(peer) = peer {
            let usage = self.peer_usage.entry(peer).or_default();
            *usage = usage
                .checked_add(charge)
                .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
            self.by_peer.entry(peer).or_default().insert(hash.clone());
        }
        for parent in dependencies {
            self.by_parent
                .entry(parent)
                .or_default()
                .insert(hash.clone());
        }
        self.by_short_id.insert(short_id, hash.clone());
        if let Some(expires_at) = expires_at {
            let deadline = DeadlineTicket {
                expires_at,
                hash: hash.clone(),
                incarnation,
                generation: 0,
            };
            self.deadlines.push(Reverse(deadline.clone()));
            self.live_deadlines.insert(hash.clone(), deadline);
        }
        self.entries.insert(hash, entry);
        self.queue_mut(queue_kind)?
            .push_reserved(queue_kind, ticket, source.is_proposal())?;
        Ok(CoordinatorVersion {
            incarnation,
            revision: 0,
        })
    }

    pub(crate) fn promote_source(
        &mut self,
        hash: &Byte32,
        promotion: TrustedSource,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        let (current, charge, old_ticket, queue_kind, version, active) = {
            let entry = self
                .entries
                .get(hash)
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            (
                entry.source,
                CoordinatorResidency::new(1, entry.charge_bytes),
                entry.ticket(hash),
                entry.queue_kind(),
                entry.version(),
                entry.uses_active_slot(),
            )
        };
        let target = match promotion {
            TrustedSource::Local => CoordinatorSource::Local,
            TrustedSource::Proposal => CoordinatorSource::Proposal,
        };
        if current == CoordinatorSource::Proposal && target == CoordinatorSource::Local {
            return Err(CoordinatorError::SourceDowngrade);
        }
        let repeated_proposal = current == CoordinatorSource::Proposal
            && target == CoordinatorSource::Proposal
            && queue_kind.is_some();
        if current == target && !repeated_proposal {
            return Ok(version);
        }
        let reticket = queue_kind.is_some();
        let queue_sequence = if reticket {
            Some(self.queue_sequence_range(1)?)
        } else {
            None
        };
        if reticket {
            self.ensure_revision_capacity(hash)?;
            self.queue_mut(queue_kind.ok_or(CoordinatorError::SourceDowngrade)?)?
                .reserve_live(target.is_proposal(), target.queue_owner())?;
        }
        if let Some(peer) = current.peer() {
            let usage = self
                .peer_usage
                .get(&peer)
                .copied()
                .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
            usage
                .checked_sub(charge)
                .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
            if !self
                .by_peer
                .get(&peer)
                .is_some_and(|hashes| hashes.contains(hash))
            {
                return Err(CoordinatorError::PeerBudgetExceeded(peer));
            }
            if active && self.peer_active_work(peer) == 0 {
                return Err(CoordinatorError::PeerActiveWorkLimitExceeded(peer));
            }
        }

        if let Some(peer) = current.peer() {
            let remove_usage = {
                let usage = self
                    .peer_usage
                    .get_mut(&peer)
                    .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
                *usage = usage
                    .checked_sub(charge)
                    .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
                *usage == CoordinatorResidency::default()
            };
            if remove_usage {
                self.peer_usage.remove(&peer);
            }
            let remove_bucket = if let Some(hashes) = self.by_peer.get_mut(&peer) {
                hashes.remove(hash);
                hashes.is_empty()
            } else {
                false
            };
            if remove_bucket {
                self.by_peer.remove(&peer);
            }
            if active {
                let remove_active = {
                    let active = self
                        .active_work_by_peer
                        .get_mut(&peer)
                        .ok_or(CoordinatorError::PeerActiveWorkLimitExceeded(peer))?;
                    *active = active
                        .checked_sub(1)
                        .ok_or(CoordinatorError::PeerActiveWorkLimitExceeded(peer))?;
                    *active == 0
                };
                if remove_active {
                    self.active_work_by_peer.remove(&peer);
                }
            }
        }
        let entry = self
            .entries
            .get_mut(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        entry.source = target;
        if let Some(kind) = queue_kind.filter(|_| reticket) {
            let (sequence, next_sequence) =
                queue_sequence.ok_or(CoordinatorError::QueueSequenceExhausted)?;
            entry.queue_sequence = sequence;
            entry.revision += 1;
            let new_ticket = entry.ticket(hash);
            let queue = self.queue_mut(kind)?;
            queue.remove_live(&old_ticket);
            queue.push_reserved(kind, new_ticket, target.is_proposal())?;
            self.next_queue_sequence = next_sequence;
        }
        self.entries
            .get(hash)
            .map(CoordinatorEntry::version)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))
    }

    pub(crate) fn checkout_raw(
        &mut self,
        stage: RawStage,
    ) -> Result<Option<RawWorkLease<R>>, CoordinatorError> {
        let kind = match stage {
            RawStage::PreCheck => QueueKind::PreCheck,
            RawStage::Resolve => QueueKind::Resolve,
        };
        let Some(ticket) = self.peek_live_ticket(kind, WorkerCapability::Any)? else {
            return Ok(None);
        };
        let expected = CoordinatorLocation::RawQueued(stage);
        self.validate_version_location_phase(
            &ticket.hash,
            ticket.version,
            &expected,
            PayloadPhase::Raw,
        )?;
        self.ensure_revision_capacity(&ticket.hash)?;
        let source = self
            .entries
            .get(&ticket.hash)
            .map(|entry| entry.source)
            .ok_or_else(|| CoordinatorError::Missing(ticket.hash.clone()))?;
        self.check_activate_source(source)?;
        self.consume_front_ticket(kind, &ticket)?;
        self.activate_source(source)?;
        let entry = self
            .entries
            .get_mut(&ticket.hash)
            .ok_or_else(|| CoordinatorError::Missing(ticket.hash.clone()))?;
        let EntryState::Raw { raw, location } = &mut entry.state else {
            return Err(CoordinatorError::PhaseMismatch {
                expected: PayloadPhase::Raw,
                actual: entry.phase_kind(),
            });
        };
        *location = RawLocation::Active(stage);
        let payload = Arc::clone(raw);
        entry.revision += 1;
        Ok(Some(RawWorkLease {
            hash: ticket.hash,
            stage,
            version: entry.version(),
            payload,
            source,
        }))
    }

    /// Terminalize exactly the active raw owner represented by `lease`.
    /// Administrative hash-only removal remains separate because it has
    /// different stale-worker semantics.
    pub(crate) fn terminalize_raw(
        &mut self,
        lease: &RawWorkLease<R>,
        disposition: TerminalDisposition,
    ) -> Result<TerminalRecord<R, U, V>, CoordinatorError> {
        let expected = CoordinatorLocation::RawActive(lease.stage);
        self.validate_version_location_phase(
            &lease.hash,
            lease.version,
            &expected,
            PayloadPhase::Raw,
        )?;
        let undo = self.causal_undo_hashes(std::slice::from_ref(&lease.hash));
        self.with_entry_undo(&undo, |coordinator| {
            coordinator.mark_children_invalid(&lease.hash, &lease.hash)?;
            let entry = coordinator.remove_present_apply(&lease.hash)?;
            coordinator.apply_fault_checkpoint();
            Ok(Self::terminal_record(
                lease.hash.clone(),
                entry,
                disposition,
            ))
        })
    }

    /// Terminalize exactly one active verification lease. Source promotion
    /// may update attribution without invalidating the work lease; the final
    /// terminal record therefore takes its source from the authoritative
    /// entry, not from a worker snapshot.
    pub(crate) fn terminalize_verification(
        &mut self,
        lease: &VerifyWorkLease<U>,
        disposition: TerminalDisposition,
    ) -> Result<TerminalRecord<R, U, V>, CoordinatorError> {
        self.validate_version_location_phase(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::VerifyActive,
            PayloadPhase::Unverified,
        )?;
        let undo = self.causal_undo_hashes(std::slice::from_ref(&lease.hash));
        self.with_entry_undo(&undo, |coordinator| {
            coordinator.mark_children_invalid(&lease.hash, &lease.hash)?;
            let entry = coordinator.remove_present_apply(&lease.hash)?;
            coordinator.apply_fault_checkpoint();
            Ok(Self::terminal_record(
                lease.hash.clone(),
                entry,
                disposition,
            ))
        })
    }

    /// A chain update can make an input disappear after resolution but before
    /// script verification completes. Preserve the transaction under the
    /// coordinator instead of terminalizing it into a second orphan owner:
    /// discard the stale resolved payload, recharge the raw phase, and either
    /// wait for the exact still-missing parents or requeue resolution when a
    /// TxPool/coordinator handoff made every reported parent available.
    pub(crate) fn verification_retry_resolution(
        &mut self,
        lease: &VerifyWorkLease<U>,
        missing: HashSet<Byte32>,
    ) -> Result<(CoordinatorVersion, CoordinatorSource), CoordinatorError> {
        self.validate_version_location_phase(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::VerifyActive,
            PayloadPhase::Unverified,
        )?;
        let entry = self
            .entries
            .get(&lease.hash)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        if let Some(parent) = missing
            .iter()
            .find(|parent| !entry.dependencies.contains(*parent))
        {
            return Err(CoordinatorError::MissingParentNotDependency {
                child: lease.hash.clone(),
                parent: parent.clone(),
            });
        }
        self.ensure_revision_capacity(&lease.hash)?;
        let requeue = missing.is_empty();
        let (queue_sequence, next_queue_sequence) = if requeue {
            let source = self
                .entries
                .get(&lease.hash)
                .map(|entry| entry.source)
                .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
            self.queue_mut(QueueKind::Resolve)?
                .reserve_live(source.is_proposal(), source.queue_owner())?;
            let (first, next) = self.queue_sequence_range(1)?;
            (Some(first), Some(next))
        } else {
            (None, None)
        };
        self.with_entry_undo(std::slice::from_ref(&lease.hash), |coordinator| {
            let (source, raw_charge) = coordinator
                .entries
                .get(&lease.hash)
                .map(|entry| (entry.source, entry.raw_charge_bytes))
                .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
            coordinator.deactivate_source(source)?;
            coordinator.apply_recharge(&lease.hash, raw_charge)?;
            let version = {
                let entry = coordinator
                    .entries
                    .get_mut(&lease.hash)
                    .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
                let raw = Arc::clone(entry.state.raw());
                let location = if requeue {
                    RawLocation::Queued(RawStage::Resolve)
                } else {
                    RawLocation::WaitingParents { missing }
                };
                entry.state = EntryState::Raw { raw, location };
                entry.resident_payload_bytes = entry.raw_resident_payload_bytes;
                entry.metadata_bytes = entry.base_metadata_bytes;
                entry.verify_schedule = VerifySchedule::default();
                if let Some(queue_sequence) = queue_sequence {
                    entry.queue_sequence = queue_sequence;
                }
                entry.revision += 1;
                entry.version()
            };
            if let Some(next_queue_sequence) = next_queue_sequence {
                let entry = coordinator
                    .entries
                    .get(&lease.hash)
                    .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
                let ticket = entry.ticket(&lease.hash);
                let priority = entry.source.is_proposal();
                coordinator.queue_mut(QueueKind::Resolve)?.push_reserved(
                    QueueKind::Resolve,
                    ticket,
                    priority,
                )?;
                coordinator.next_queue_sequence = next_queue_sequence;
            }
            coordinator.apply_fault_checkpoint();
            Ok((version, source))
        })
    }

    /// Replace raw work with an unverified phase bundle. `charge_bytes` is
    /// the total payload residency of that entire bundle, including the raw
    /// transaction retained for dependency demotion and terminal handoff.
    pub(crate) fn complete_raw(
        &mut self,
        lease: &RawWorkLease<R>,
        unverified: U,
        charge_bytes: usize,
        verify_schedule: VerifySchedule,
    ) -> Result<(CoordinatorVersion, Vec<TerminalRecord<R, U, V>>), CoordinatorError> {
        let expected = CoordinatorLocation::RawActive(lease.stage);
        self.validate_version_location_phase(
            &lease.hash,
            lease.version,
            &expected,
            PayloadPhase::Raw,
        )?;
        let entry = self
            .entries
            .get(&lease.hash)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        let total_charge_bytes = charge_bytes
            .checked_add(entry.base_metadata_bytes)
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        let protected = self.dependency_ancestor_closure(&lease.hash, &entry.dependencies)?;
        self.check_peer_budget_after_victims(
            Some(&lease.hash),
            entry.source,
            total_charge_bytes,
            &HashSet::new(),
        )?;
        let victims = self.global_capacity_victims(
            Some(&lease.hash),
            entry.source,
            total_charge_bytes,
            &HashSet::new(),
            &protected,
        )?;
        let subject = CapacitySubject::Present(lease.hash.clone());
        self.with_capacity_victims(subject, victims, move |coordinator| {
            coordinator.complete_raw_inner(lease, unverified, charge_bytes, verify_schedule)
        })
    }

    fn complete_raw_inner(
        &mut self,
        lease: &RawWorkLease<R>,
        unverified: U,
        charge_bytes: usize,
        verify_schedule: VerifySchedule,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        let expected = CoordinatorLocation::RawActive(lease.stage);
        self.validate_version_location_phase(
            &lease.hash,
            lease.version,
            &expected,
            PayloadPhase::Raw,
        )?;
        self.ensure_revision_capacity(&lease.hash)?;
        let metadata_bytes = self
            .entries
            .get(&lease.hash)
            .map(|entry| entry.base_metadata_bytes)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        let total_charge_bytes = charge_bytes
            .checked_add(metadata_bytes)
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        self.check_recharge(&lease.hash, total_charge_bytes)?;
        let source = self
            .entries
            .get(&lease.hash)
            .map(|entry| entry.source)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        let priority = source.is_proposal();
        self.queue_mut(QueueKind::Verify)?
            .reserve_live(priority, source.queue_owner())?;
        let (queue_sequence, next_queue_sequence) = self.queue_sequence_range(1)?;
        self.deactivate_source(source)?;
        self.apply_recharge(&lease.hash, total_charge_bytes)?;
        let entry = self
            .entries
            .get_mut(&lease.hash)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        let raw = Arc::clone(entry.state.raw());
        entry.state = EntryState::Unverified {
            raw,
            payload: Arc::new(unverified),
            location: UnverifiedLocation::Queued,
        };
        entry.resident_payload_bytes = charge_bytes;
        entry.metadata_bytes = metadata_bytes;
        entry.queue_sequence = queue_sequence;
        entry.verify_schedule = verify_schedule;
        entry.revision += 1;
        let version = entry.version();
        let ticket = entry.ticket(&lease.hash);
        let front = entry.source.is_proposal();
        self.queue_mut(QueueKind::Verify)?
            .push_reserved(QueueKind::Verify, ticket, front)?;
        self.next_queue_sequence = next_queue_sequence;
        Ok(version)
    }

    pub(crate) fn wait_for_parents(
        &mut self,
        lease: &RawWorkLease<R>,
        missing: HashSet<Byte32>,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        let expected = CoordinatorLocation::RawActive(lease.stage);
        self.validate_version_location_phase(
            &lease.hash,
            lease.version,
            &expected,
            PayloadPhase::Raw,
        )?;
        if let Some(parent) = missing.iter().find(|parent| {
            !self
                .entries
                .get(&lease.hash)
                .is_some_and(|entry| entry.dependencies.contains(*parent))
        }) {
            return Err(CoordinatorError::MissingParentNotDependency {
                child: lease.hash.clone(),
                parent: parent.clone(),
            });
        }
        if missing.is_empty() {
            return self.requeue_raw(lease);
        }
        self.ensure_revision_capacity(&lease.hash)?;
        let source = self
            .entries
            .get(&lease.hash)
            .map(|entry| entry.source)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        self.deactivate_source(source)?;
        let entry = self
            .entries
            .get_mut(&lease.hash)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        let EntryState::Raw { location, .. } = &mut entry.state else {
            return Err(CoordinatorError::ConflictInvariant);
        };
        *location = RawLocation::WaitingParents { missing };
        entry.revision += 1;
        Ok(entry.version())
    }

    pub(crate) fn requeue_raw(
        &mut self,
        lease: &RawWorkLease<R>,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        let expected = CoordinatorLocation::RawActive(lease.stage);
        self.validate_version_location_phase(
            &lease.hash,
            lease.version,
            &expected,
            PayloadPhase::Raw,
        )?;
        self.ensure_revision_capacity(&lease.hash)?;
        let kind = match lease.stage {
            RawStage::PreCheck => QueueKind::PreCheck,
            RawStage::Resolve => QueueKind::Resolve,
        };
        let source = self
            .entries
            .get(&lease.hash)
            .map(|entry| entry.source)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        let priority = source.is_proposal();
        self.queue_mut(kind)?
            .reserve_live(priority, source.queue_owner())?;
        let (queue_sequence, next_queue_sequence) = self.queue_sequence_range(1)?;
        self.deactivate_source(source)?;
        let entry = self
            .entries
            .get_mut(&lease.hash)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        let EntryState::Raw { location, .. } = &mut entry.state else {
            return Err(CoordinatorError::ConflictInvariant);
        };
        *location = RawLocation::Queued(lease.stage);
        entry.queue_sequence = queue_sequence;
        entry.verify_schedule = VerifySchedule::default();
        entry.revision += 1;
        let version = entry.version();
        let ticket = entry.ticket(&lease.hash);
        let front = entry.source.is_proposal();
        self.queue_mut(kind)?.push_reserved(kind, ticket, front)?;
        self.next_queue_sequence = next_queue_sequence;
        Ok(version)
    }

    pub(crate) fn parent_available(
        &mut self,
        parent: &Byte32,
    ) -> Result<Vec<CoordinatorTicket>, CoordinatorError> {
        let mut children: Vec<_> = self
            .by_parent
            .get(parent)
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        children.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        let mut affected = Vec::new();
        let mut ready_count = 0usize;
        let mut priority_ready_count = 0usize;
        let mut priority_owners = Vec::new();
        let mut normal_owners = Vec::new();
        for child in children {
            let Some(entry) = self.entries.get(&child) else {
                continue;
            };
            let EntryState::Raw {
                location: RawLocation::WaitingParents { missing },
                ..
            } = &entry.state
            else {
                continue;
            };
            if !missing.contains(parent) {
                continue;
            }
            self.ensure_revision_capacity(&child)?;
            if missing.len() == 1 {
                ready_count = ready_count.saturating_add(1);
                if entry.source.is_proposal() {
                    priority_ready_count = priority_ready_count.saturating_add(1);
                    priority_owners.push(entry.source.queue_owner());
                } else {
                    normal_owners.push(entry.source.queue_owner());
                }
            }
            affected.push(child);
        }
        self.queue_mut(QueueKind::Resolve)?.reserve_many(
            true,
            priority_owners,
            priority_ready_count,
        )?;
        self.queue_mut(QueueKind::Resolve)?.reserve_many(
            false,
            normal_owners,
            ready_count.saturating_sub(priority_ready_count),
        )?;
        let mut ready = Vec::new();
        ready
            .try_reserve(ready_count)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        let (first_queue_sequence, next_queue_sequence) = self.queue_sequence_range(ready_count)?;

        let undo = affected.clone();
        let mut queue_sequence = first_queue_sequence;
        self.with_entry_undo(&undo, |coordinator| {
            coordinator.next_queue_sequence = next_queue_sequence;
            for child in affected {
                let entry = coordinator
                    .entries
                    .get_mut(&child)
                    .ok_or_else(|| CoordinatorError::Missing(child.clone()))?;
                let missing = match &mut entry.state {
                    EntryState::Raw {
                        location: RawLocation::WaitingParents { missing },
                        ..
                    } => missing,
                    state => {
                        return Err(CoordinatorError::LocationMismatch {
                            expected: CoordinatorLocation::WaitingParents {
                                missing: HashSet::from([parent.clone()]),
                            },
                            actual: state.location(),
                        });
                    }
                };
                missing.remove(parent);
                let ready_now = missing.is_empty();
                entry.revision += 1;
                if ready_now {
                    let EntryState::Raw { location, .. } = &mut entry.state else {
                        return Err(CoordinatorError::ConflictInvariant);
                    };
                    *location = RawLocation::Queued(RawStage::Resolve);
                    entry.queue_sequence = queue_sequence;
                    entry.verify_schedule = VerifySchedule::default();
                    queue_sequence = queue_sequence
                        .checked_add(1)
                        .ok_or(CoordinatorError::QueueSequenceExhausted)?;
                    let ticket = entry.ticket(&child);
                    let front = entry.source.is_proposal();
                    coordinator.queue_mut(QueueKind::Resolve)?.push_reserved(
                        QueueKind::Resolve,
                        ticket.clone(),
                        front,
                    )?;
                    ready.push(ticket);
                }
                coordinator.apply_fault_checkpoint();
            }
            Ok(ready)
        })
    }

    fn parents_unavailable_undo_hashes(&self, parents: &HashSet<Byte32>) -> Vec<Byte32> {
        let mut affected = HashSet::new();
        for parent in parents {
            for child in self.by_parent.get(parent).into_iter().flatten() {
                let Some(entry) = self.entries.get(child) else {
                    continue;
                };
                if entry.invalidated_cause().is_some()
                    || matches!(
                        &entry.state,
                        EntryState::Raw {
                            location: RawLocation::WaitingParents { missing },
                            ..
                        } if missing.contains(parent)
                    )
                {
                    continue;
                }
                affected.insert(child.clone());
            }
        }
        let direct: Vec<_> = affected.iter().cloned().collect();
        for child in direct {
            if let Some(waiters) = self.waiters_by_blocker.get(&child) {
                affected.extend(waiters.iter().cloned());
            }
        }
        affected.into_iter().collect()
    }

    pub(crate) fn parent_unavailable(
        &mut self,
        parent: &Byte32,
    ) -> Result<Vec<Byte32>, CoordinatorError> {
        self.parents_unavailable(&HashSet::from([parent.clone()]))
    }

    /// Atomically demote every coordinator transaction whose dependency is in
    /// `parents`. Administrative pool removal uses this before deleting a
    /// root and its accepted descendants, so no already-resolved consumer can
    /// outlive any member of the removed closure. Every child is transitioned
    /// once even when several of its parents disappear together.
    pub(crate) fn parents_unavailable(
        &mut self,
        parents: &HashSet<Byte32>,
    ) -> Result<Vec<Byte32>, CoordinatorError> {
        let mut ordered_parents: Vec<_> = parents.iter().cloned().collect();
        ordered_parents.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        let mut missing_by_child: HashMap<Byte32, HashSet<Byte32>> = HashMap::new();
        for parent in ordered_parents {
            let mut children: Vec<_> = self
                .by_parent
                .get(&parent)
                .into_iter()
                .flatten()
                .cloned()
                .collect();
            children.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
            for child in children {
                let Some(entry) = self.entries.get(&child) else {
                    continue;
                };
                // Definitive dependency failure has precedence over a later
                // availability transition for another parent.
                if entry.invalidated_cause().is_some() {
                    continue;
                }
                let already_missing = matches!(
                    &entry.state,
                    EntryState::Raw {
                        location: RawLocation::WaitingParents { missing },
                        ..
                    } if missing.contains(&parent)
                );
                if !already_missing {
                    missing_by_child
                        .entry(child)
                        .or_default()
                        .insert(parent.clone());
                }
            }
        }

        let mut affected: Vec<_> = missing_by_child.keys().cloned().collect();
        affected.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        for child in &affected {
            self.ensure_revision_capacity(child)?;
            self.preflight_remove_conflict_indexes(child)?;
        }

        let mut undo = affected.clone();
        for child in &affected {
            if let Some(waiters) = self.waiters_by_blocker.get(child) {
                undo.extend(waiters.iter().cloned());
            }
        }
        undo.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        undo.dedup();
        let result = affected.clone();
        self.with_entry_undo(&undo, |coordinator| {
            for child in &affected {
                let active_source = coordinator
                    .entries
                    .get(child)
                    .and_then(|entry| entry.uses_active_slot().then_some(entry.source));
                if let Some(source) = active_source {
                    coordinator.deactivate_source(source)?;
                }
                coordinator.remove_current_queue_ticket(child)?;
                coordinator.remove_conflict_indexes(child)?;
                coordinator.apply_fault_checkpoint();
                let raw_charge = coordinator
                    .entries
                    .get(child)
                    .ok_or_else(|| CoordinatorError::Missing(child.clone()))?
                    .raw_charge_bytes;
                coordinator.apply_recharge(child, raw_charge)?;
                let entry = coordinator
                    .entries
                    .get_mut(child)
                    .ok_or_else(|| CoordinatorError::Missing(child.clone()))?;
                let mut missing = match &entry.state {
                    EntryState::Raw {
                        location: RawLocation::WaitingParents { missing },
                        ..
                    } => missing.clone(),
                    _ => HashSet::new(),
                };
                missing.extend(
                    missing_by_child
                        .get(child)
                        .ok_or(CoordinatorError::ConflictInvariant)?
                        .iter()
                        .cloned(),
                );
                entry.resident_payload_bytes = entry.raw_resident_payload_bytes;
                entry.metadata_bytes = entry.base_metadata_bytes;
                entry.verify_schedule = VerifySchedule::default();
                let raw = Arc::clone(entry.state.raw());
                entry.state = EntryState::Raw {
                    raw,
                    location: RawLocation::WaitingParents { missing },
                };
                entry.revision += 1;
                coordinator.apply_fault_checkpoint();
            }
            Ok(result)
        })
    }

    /// Make every direct dependent fail-closed immediately, then defer the
    /// transitive terminal cascade to bounded maintenance slices.
    pub(crate) fn schedule_parent_failure(
        &mut self,
        parent: &Byte32,
    ) -> Result<Vec<Byte32>, CoordinatorError> {
        self.mark_children_invalid(parent, parent)
    }

    pub(crate) fn drain_dependency_failures(
        &mut self,
        max: usize,
    ) -> Result<Vec<TerminalRecord<R, U, V>>, CoordinatorError> {
        let roots = self.preview_dependency_failure_roots(max);
        let mut affected = roots.clone();
        for root in &roots {
            if let Some(children) = self.by_parent.get(root) {
                affected.extend(children.iter().cloned());
            }
        }
        let directly_affected = affected.clone();
        for hash in &directly_affected {
            if let Some(waiters) = self.waiters_by_blocker.get(hash) {
                affected.extend(waiters.iter().cloned());
            }
        }
        let mut terminal = Vec::new();
        terminal
            .try_reserve(roots.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.with_entry_undo(&affected, |coordinator| {
            for hash in roots {
                let cause = coordinator
                    .entries
                    .get(&hash)
                    .and_then(CoordinatorEntry::invalidated_cause)
                    .cloned()
                    .ok_or(CoordinatorError::ConflictInvariant)?;
                coordinator.mark_children_invalid(&hash, &cause)?;
                let entry = coordinator.remove_present_apply(&hash)?;
                terminal.push(Self::terminal_record(
                    hash,
                    entry,
                    TerminalDisposition::DependencyFailed,
                ));
                coordinator.apply_fault_checkpoint();
            }
            Ok(terminal)
        })
    }

    pub(crate) fn dependency_failure_len(&self) -> usize {
        self.dependency_failure_set.len()
    }

    pub(crate) fn checkout_verify(
        &mut self,
        capability: WorkerCapability,
    ) -> Result<Option<VerifyWorkLease<U>>, CoordinatorError> {
        let Some(ticket) = self.peek_live_ticket(QueueKind::Verify, capability)? else {
            return Ok(None);
        };
        self.validate_version_location_phase(
            &ticket.hash,
            ticket.version,
            &CoordinatorLocation::VerifyQueued,
            PayloadPhase::Unverified,
        )?;
        self.ensure_revision_capacity(&ticket.hash)?;
        let source = self
            .entries
            .get(&ticket.hash)
            .map(|entry| entry.source)
            .ok_or_else(|| CoordinatorError::Missing(ticket.hash.clone()))?;
        self.check_activate_source(source)?;
        self.consume_front_ticket(QueueKind::Verify, &ticket)?;
        self.activate_source(source)?;
        let entry = self
            .entries
            .get_mut(&ticket.hash)
            .ok_or_else(|| CoordinatorError::Missing(ticket.hash.clone()))?;
        let payload = match &entry.state {
            EntryState::Unverified { payload, .. } => Arc::clone(payload),
            _ => {
                return Err(CoordinatorError::PhaseMismatch {
                    expected: PayloadPhase::Unverified,
                    actual: entry.phase_kind(),
                });
            }
        };
        let raw = Arc::clone(entry.state.raw());
        entry.state = EntryState::Unverified {
            raw,
            payload: Arc::clone(&payload),
            location: UnverifiedLocation::Active,
        };
        entry.revision += 1;
        Ok(Some(VerifyWorkLease {
            hash: ticket.hash,
            version: entry.version(),
            payload,
            source,
        }))
    }

    /// Return an active verification lease to its ordered queue without
    /// changing payload ownership. Used only when a non-transactional
    /// service dependency (such as effect-journal shutdown) cannot accept the
    /// terminal/phase transition yet.
    pub(crate) fn requeue_verification(
        &mut self,
        lease: &VerifyWorkLease<U>,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        self.validate_version_location_phase(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::VerifyActive,
            PayloadPhase::Unverified,
        )?;
        self.ensure_revision_capacity(&lease.hash)?;
        let source = self
            .entries
            .get(&lease.hash)
            .map(|entry| entry.source)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        let priority = source.is_proposal();
        self.queue_mut(QueueKind::Verify)?
            .reserve_live(priority, source.queue_owner())?;
        let (queue_sequence, next_queue_sequence) = self.queue_sequence_range(1)?;
        self.deactivate_source(source)?;
        let entry = self
            .entries
            .get_mut(&lease.hash)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        let EntryState::Unverified { location, .. } = &mut entry.state else {
            return Err(CoordinatorError::ConflictInvariant);
        };
        *location = UnverifiedLocation::Queued;
        entry.queue_sequence = queue_sequence;
        entry.revision += 1;
        let version = entry.version();
        let ticket = entry.ticket(&lease.hash);
        let front = entry.source.is_proposal();
        self.queue_mut(QueueKind::Verify)?
            .push_reserved(QueueKind::Verify, ticket, front)?;
        self.next_queue_sequence = next_queue_sequence;
        Ok(version)
    }

    /// Install a verified phase bundle. `charge_bytes` covers every payload
    /// object retained by the bundle, not only the newly produced proof.
    pub(crate) fn complete_verification(
        &mut self,
        lease: &VerifyWorkLease<U>,
        verified: V,
        charge_bytes: usize,
    ) -> Result<(CoordinatorVersion, Vec<TerminalRecord<R, U, V>>), CoordinatorError> {
        self.validate_version_location_phase(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::VerifyActive,
            PayloadPhase::Unverified,
        )?;
        let entry = self
            .entries
            .get(&lease.hash)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        let total_charge_bytes = charge_bytes
            .checked_add(entry.base_metadata_bytes)
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        let protected = self.dependency_ancestor_closure(&lease.hash, &entry.dependencies)?;
        self.check_peer_budget_after_victims(
            Some(&lease.hash),
            entry.source,
            total_charge_bytes,
            &HashSet::new(),
        )?;
        let victims = self.global_capacity_victims(
            Some(&lease.hash),
            entry.source,
            total_charge_bytes,
            &HashSet::new(),
            &protected,
        )?;
        let subject = CapacitySubject::Present(lease.hash.clone());
        self.with_capacity_victims(subject, victims, move |coordinator| {
            coordinator.complete_verification_inner(lease, verified, charge_bytes)
        })
    }

    fn complete_verification_inner(
        &mut self,
        lease: &VerifyWorkLease<U>,
        verified: V,
        charge_bytes: usize,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        self.validate_version_location_phase(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::VerifyActive,
            PayloadPhase::Unverified,
        )?;
        self.ensure_revision_capacity(&lease.hash)?;
        let metadata_bytes = self
            .entries
            .get(&lease.hash)
            .map(|entry| entry.base_metadata_bytes)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        let total_charge_bytes = charge_bytes
            .checked_add(metadata_bytes)
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        self.check_recharge(&lease.hash, total_charge_bytes)?;
        let source = self
            .entries
            .get(&lease.hash)
            .map(|entry| entry.source)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        let priority = source.is_proposal();
        self.queue_mut(QueueKind::Commit)?
            .reserve_live(priority, source.queue_owner())?;
        let (queue_sequence, next_queue_sequence) = self.queue_sequence_range(1)?;
        self.deactivate_source(source)?;
        self.apply_recharge(&lease.hash, total_charge_bytes)?;
        let entry = self
            .entries
            .get_mut(&lease.hash)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        let raw = Arc::clone(entry.state.raw());
        entry.state = EntryState::PlainVerified {
            raw,
            payload: Arc::new(verified),
            location: PlainVerifiedLocation::Ready,
        };
        entry.resident_payload_bytes = charge_bytes;
        entry.metadata_bytes = metadata_bytes;
        entry.queue_sequence = queue_sequence;
        entry.verify_schedule = VerifySchedule::default();
        entry.revision += 1;
        let version = entry.version();
        let ticket = entry.ticket(&lease.hash);
        let front = entry.source.is_proposal();
        self.queue_mut(QueueKind::Commit)?
            .push_reserved(QueueKind::Commit, ticket, front)?;
        self.next_queue_sequence = next_queue_sequence;
        Ok(version)
    }

    /// Install a verified conflict candidate. `charge_bytes` covers the
    /// complete resident phase bundle; conflict index metadata is added by
    /// the coordinator separately.
    pub(crate) fn complete_verification_candidate(
        &mut self,
        lease: &VerifyWorkLease<U>,
        verified: V,
        charge_bytes: usize,
        candidate: VerifiedCandidate,
    ) -> Result<(CoordinatorVersion, Vec<TerminalRecord<R, U, V>>), CoordinatorError> {
        self.validate_version_location_phase(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::VerifyActive,
            PayloadPhase::Unverified,
        )?;
        if candidate.inputs.len() > self.limits.max_conflict_inputs_per_entry {
            return Err(CoordinatorError::ConflictInputLimitExceeded);
        }
        let entry = self
            .entries
            .get(&lease.hash)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        let source = entry.source;
        let protected = self.dependency_ancestor_closure(&lease.hash, &entry.dependencies)?;
        let incoming = CandidateMeta {
            inputs: candidate.inputs.clone(),
            fee: candidate.fee,
            tx_size: candidate.tx_size,
            arrival: self.next_arrival,
        };
        let mut victims =
            self.conflict_capacity_victims(&lease.hash, source, &incoming, &protected)?;
        let metadata_bytes = self.metadata_charge_bytes(
            entry.dependencies.len(),
            entry.expires_at.is_some(),
            candidate.inputs.len(),
        )?;
        let total_charge_bytes = charge_bytes
            .checked_add(metadata_bytes)
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        let selected: HashSet<_> = victims.iter().cloned().collect();
        self.check_peer_budget_after_victims(
            Some(&lease.hash),
            source,
            total_charge_bytes,
            &selected,
        )?;
        victims.extend(self.global_capacity_victims(
            Some(&lease.hash),
            source,
            total_charge_bytes,
            &selected,
            &protected,
        )?);
        victims.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        victims.dedup();
        let subject = CapacitySubject::Present(lease.hash.clone());
        self.with_capacity_victims(subject, victims, move |coordinator| {
            coordinator.complete_verification_candidate_inner(
                lease,
                verified,
                charge_bytes,
                candidate,
            )
        })
    }

    fn complete_verification_candidate_inner(
        &mut self,
        lease: &VerifyWorkLease<U>,
        verified: V,
        charge_bytes: usize,
        candidate: VerifiedCandidate,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        self.validate_version_location_phase(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::VerifyActive,
            PayloadPhase::Unverified,
        )?;
        self.ensure_revision_capacity(&lease.hash)?;
        if candidate.inputs.len() > self.limits.max_conflict_inputs_per_entry {
            return Err(CoordinatorError::ConflictInputLimitExceeded);
        }
        let next_edges = self
            .conflict_edge_count
            .checked_add(candidate.inputs.len())
            .ok_or(CoordinatorError::ConflictEdgeLimitExceeded)?;
        if next_edges > self.limits.max_conflict_edges {
            return Err(CoordinatorError::ConflictEdgeLimitExceeded);
        }
        for input in &candidate.inputs {
            if self
                .candidates_by_input
                .get(input)
                .map_or(0, HashSet::len)
                .saturating_add(1)
                > self.limits.max_candidates_per_input
            {
                return Err(CoordinatorError::ConflictCandidateLimitExceeded(
                    input.clone(),
                ));
            }
        }
        let (dependencies, has_deadline) = self
            .entries
            .get(&lease.hash)
            .map(|entry| (entry.dependencies.len(), entry.expires_at.is_some()))
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        let metadata_bytes =
            self.metadata_charge_bytes(dependencies, has_deadline, candidate.inputs.len())?;
        let total_charge_bytes = charge_bytes
            .checked_add(metadata_bytes)
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        self.check_recharge(&lease.hash, total_charge_bytes)?;
        let arrival = self.next_arrival;
        let next_arrival = arrival
            .checked_add(1)
            .ok_or(CoordinatorError::ArrivalSequenceExhausted)?;
        let meta = CandidateMeta {
            inputs: candidate.inputs,
            fee: candidate.fee,
            tx_size: candidate.tx_size,
            arrival,
        };
        let blockers = self.active_blockers_for_inputs(&lease.hash, &meta.inputs);
        let can_preempt = !blockers.is_empty()
            && blockers.iter().all(|blocker| {
                self.entries.get(blocker).is_some_and(|entry| {
                    matches!(
                        &entry.state,
                        EntryState::CandidateVerified {
                            candidate: blocker_meta,
                            location: CandidateLocation::Ready,
                            ..
                        } if Self::compare_candidates(
                            &lease.hash,
                            &meta,
                            blocker,
                            blocker_meta,
                        ) == Ordering::Greater
                    )
                })
            });

        let mut invalidated_waiters = HashSet::new();
        if can_preempt {
            for blocker in &blockers {
                self.ensure_revision_capacity(blocker)?;
                if let Some(waiters) = self.waiters_by_blocker.get(blocker) {
                    invalidated_waiters.extend(waiters.iter().cloned());
                }
            }
            for waiter in &invalidated_waiters {
                self.ensure_revision_capacity(waiter)?;
            }
            if blockers.len() > self.limits.max_candidates_per_input {
                return Err(CoordinatorError::ConflictCandidateLimitExceeded(
                    meta.inputs
                        .iter()
                        .next()
                        .cloned()
                        .ok_or(CoordinatorError::ConflictInvariant)?,
                ));
            }
        } else {
            for blocker in &blockers {
                if self
                    .waiters_by_blocker
                    .get(blocker)
                    .map_or(0, HashSet::len)
                    .saturating_add(1)
                    > self.limits.max_candidates_per_input
                {
                    return Err(CoordinatorError::ConflictCandidateLimitExceeded(
                        meta.inputs
                            .iter()
                            .next()
                            .cloned()
                            .ok_or(CoordinatorError::ConflictInvariant)?,
                    ));
                }
            }
        }

        let ready_to_commit = blockers.is_empty() || can_preempt;
        if ready_to_commit {
            let source = self
                .entries
                .get(&lease.hash)
                .map(|entry| entry.source)
                .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
            self.queue_mut(QueueKind::Commit)?
                .reserve_live(source.is_proposal(), source.queue_owner())?;
        }
        let queue_sequence = if ready_to_commit {
            Some(self.queue_sequence_range(1)?)
        } else {
            None
        };
        self.conflict_rechecks
            .try_reserve(invalidated_waiters.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.conflict_recheck_set
            .try_reserve(invalidated_waiters.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;

        let undo_capacity = 1usize
            .saturating_add(blockers.len())
            .saturating_add(invalidated_waiters.len());
        let mut undo = Vec::new();
        undo.try_reserve(undo_capacity)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        undo.push(lease.hash.clone());
        undo.extend(blockers.iter().cloned());
        undo.extend(invalidated_waiters.iter().cloned());
        self.with_entry_undo(&undo, |coordinator| {
            if let Some((_, next_queue_sequence)) = queue_sequence {
                coordinator.next_queue_sequence = next_queue_sequence;
            }
            let source = coordinator
                .entries
                .get(&lease.hash)
                .map(|entry| entry.source)
                .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
            coordinator.deactivate_source(source)?;
            coordinator.apply_recharge(&lease.hash, total_charge_bytes)?;
            coordinator.next_arrival = next_arrival;
            coordinator.conflict_edge_count = next_edges;
            for input in &meta.inputs {
                coordinator
                    .candidates_by_input
                    .entry(input.clone())
                    .or_default()
                    .insert(lease.hash.clone());
            }
            coordinator.apply_fault_checkpoint();

            if can_preempt {
                for blocker in &blockers {
                    coordinator.invalidate_conflict_waiters(blocker)?;
                    coordinator.remove_current_queue_ticket(blocker)?;
                    coordinator.release_conflict_claims(blocker)?;
                    let entry = coordinator
                        .entries
                        .get_mut(blocker)
                        .ok_or_else(|| CoordinatorError::Missing(blocker.clone()))?;
                    let EntryState::CandidateVerified { location, .. } = &mut entry.state else {
                        return Err(CoordinatorError::ConflictInvariant);
                    };
                    *location = CandidateLocation::WaitingConflict {
                        blockers: HashSet::from([lease.hash.clone()]),
                    };
                    entry.revision += 1;
                    coordinator
                        .waiters_by_blocker
                        .entry(lease.hash.clone())
                        .or_default()
                        .insert(blocker.clone());
                    coordinator.apply_fault_checkpoint();
                }
            }

            let entry = coordinator
                .entries
                .get_mut(&lease.hash)
                .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
            let location = if ready_to_commit {
                CandidateLocation::Ready
            } else {
                CandidateLocation::WaitingConflict {
                    blockers: blockers.clone(),
                }
            };
            let raw = Arc::clone(entry.state.raw());
            entry.state = EntryState::CandidateVerified {
                raw,
                payload: Arc::new(verified),
                candidate: meta,
                location,
            };
            entry.resident_payload_bytes = charge_bytes;
            entry.metadata_bytes = metadata_bytes;
            if let Some((sequence, _)) = queue_sequence {
                entry.queue_sequence = sequence;
            }
            entry.verify_schedule = VerifySchedule::default();
            entry.revision += 1;
            if ready_to_commit {
                let version = entry.version();
                let ticket = entry.ticket(&lease.hash);
                let front = entry.source.is_proposal();
                coordinator.claim_conflict_inputs(&lease.hash)?;
                coordinator.queue_mut(QueueKind::Commit)?.push_reserved(
                    QueueKind::Commit,
                    ticket,
                    front,
                )?;
                coordinator.apply_fault_checkpoint();
                Ok(version)
            } else {
                let version = entry.version();
                for blocker in blockers {
                    coordinator
                        .waiters_by_blocker
                        .entry(blocker)
                        .or_default()
                        .insert(lease.hash.clone());
                }
                coordinator.apply_fault_checkpoint();
                Ok(version)
            }
        })
    }

    pub(crate) fn begin_next_commit(&mut self) -> Result<Option<CommitLease<V>>, CoordinatorError> {
        let Some(ticket) = self.peek_live_ticket(QueueKind::Commit, WorkerCapability::Any)? else {
            return Ok(None);
        };
        self.validate_version_location_phase(
            &ticket.hash,
            ticket.version,
            &CoordinatorLocation::ReadyToCommit,
            PayloadPhase::Verified,
        )?;
        self.ensure_revision_capacity(&ticket.hash)?;
        if self.entries.get(&ticket.hash).is_some_and(|entry| {
            entry.expires_at.is_some() && entry.deadline_generation == u64::MAX
        }) {
            return Err(CoordinatorError::DeadlineGenerationExhausted(
                ticket.hash.clone(),
            ));
        }
        let source = self
            .entries
            .get(&ticket.hash)
            .map(|entry| entry.source)
            .ok_or_else(|| CoordinatorError::Missing(ticket.hash.clone()))?;
        self.check_activate_source(source)?;
        self.consume_front_ticket(QueueKind::Commit, &ticket)?;
        self.activate_source(source)?;
        // A committing entry is temporarily outside expiry scheduling. This
        // prevents one frozen deadline from blocking every later due ticket;
        // abort restores the original incarnation-scoped deadline, while a
        // successful handoff consumes it with the entry.
        self.live_deadlines.remove(&ticket.hash);
        let entry = self
            .entries
            .get_mut(&ticket.hash)
            .ok_or_else(|| CoordinatorError::Missing(ticket.hash.clone()))?;
        if entry.expires_at.is_some() {
            entry.deadline_generation += 1;
        }
        let payload = match &mut entry.state {
            EntryState::PlainVerified {
                payload, location, ..
            } => {
                *location = PlainVerifiedLocation::Committing;
                Arc::clone(payload)
            }
            EntryState::CandidateVerified {
                payload, location, ..
            } => {
                *location = CandidateLocation::Committing;
                Arc::clone(payload)
            }
            _ => {
                return Err(CoordinatorError::PhaseMismatch {
                    expected: PayloadPhase::Verified,
                    actual: entry.phase_kind(),
                });
            }
        };
        entry.revision += 1;
        Ok(Some(CommitLease {
            hash: ticket.hash,
            version: entry.version(),
            payload,
        }))
    }

    pub(crate) fn drain_conflict_rechecks(
        &mut self,
        limit: usize,
    ) -> Result<Vec<CoordinatorTicket>, CoordinatorError> {
        // Freeze the slice before applying it. Waiters discovered while this
        // slice runs remain level-triggered for the next slice instead of
        // extending the current transaction's attack-controlled work graph.
        let selected: Vec<_> = self
            .conflict_rechecks
            .iter()
            .filter(|hash| self.conflict_recheck_set.contains(*hash))
            .take(limit)
            .cloned()
            .collect();
        let mut affected = selected.clone();
        for hash in &selected {
            let candidate = self
                .entries
                .get(hash)
                .and_then(CoordinatorEntry::candidate)
                .ok_or(CoordinatorError::ConflictInvariant)?;
            for input in &candidate.inputs {
                if let Some(candidates) = self.candidates_by_input.get(input) {
                    affected.extend(candidates.iter().cloned());
                }
            }
        }
        let conflict_cohort = affected.clone();
        for hash in &conflict_cohort {
            if let Some(waiters) = self.waiters_by_blocker.get(hash) {
                affected.extend(waiters.iter().cloned());
            }
        }
        let mut activated = Vec::new();
        activated
            .try_reserve(selected.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.with_entry_undo(&affected, |coordinator| {
            for hash in &selected {
                let plan = coordinator.prepare_conflict_recheck(hash)?;
                if let Some(ticket) = coordinator.apply_conflict_recheck(hash, &plan)? {
                    activated.push(ticket);
                }
                coordinator.conflict_recheck_set.remove(hash);
                coordinator.apply_fault_checkpoint();
            }
            coordinator.compact_conflict_rechecks();
            Ok(activated)
        })
    }

    pub(crate) fn abort_commit(
        &mut self,
        lease: &CommitLease<V>,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        self.validate_version_location_phase(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::Committing,
            PayloadPhase::Verified,
        )?;
        self.ensure_revision_capacity(&lease.hash)?;
        let source = self
            .entries
            .get(&lease.hash)
            .map(|entry| entry.source)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        let priority = source.is_proposal();
        self.queue_mut(QueueKind::Commit)?
            .reserve_live(priority, source.queue_owner())?;
        let (queue_sequence, next_queue_sequence) = self.queue_sequence_range(1)?;
        let deadline = self.entries.get(&lease.hash).and_then(|entry| {
            entry.expires_at.map(|expires_at| DeadlineTicket {
                expires_at,
                hash: lease.hash.clone(),
                incarnation: entry.incarnation,
                generation: entry.deadline_generation,
            })
        });
        if deadline.is_some() {
            self.deadlines
                .try_reserve(1)
                .map_err(|_| CoordinatorError::QueueReservationFailed)?;
            self.live_deadlines
                .try_reserve(1)
                .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        }
        self.deactivate_source(source)?;
        let entry = self
            .entries
            .get_mut(&lease.hash)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        match &mut entry.state {
            EntryState::PlainVerified { location, .. } => {
                *location = PlainVerifiedLocation::Ready;
            }
            EntryState::CandidateVerified { location, .. } => {
                *location = CandidateLocation::Ready;
            }
            _ => return Err(CoordinatorError::ConflictInvariant),
        }
        entry.queue_sequence = queue_sequence;
        entry.verify_schedule = VerifySchedule::default();
        entry.revision += 1;
        let version = entry.version();
        let ticket = entry.ticket(&lease.hash);
        let front = entry.source.is_proposal();
        if let Some(deadline) = deadline {
            self.deadlines.push(Reverse(deadline.clone()));
            self.live_deadlines.insert(lease.hash.clone(), deadline);
            self.compact_deadlines();
        }
        self.queue_mut(QueueKind::Commit)?
            .push_reserved(QueueKind::Commit, ticket, front)?;
        self.next_queue_sequence = next_queue_sequence;
        Ok(version)
    }

    /// Definitively fail exactly one committing owner. Unlike
    /// [`Self::abort_commit`], this is a terminal transition: dependents are
    /// invalidated and speculative conflict waiters are scheduled for
    /// re-evaluation. The versioned commit lease prevents a late submit task
    /// from removing a re-admitted transaction with the same hash.
    pub(crate) fn fail_commit(
        &mut self,
        lease: &CommitLease<V>,
        disposition: TerminalDisposition,
    ) -> Result<TerminalRecord<R, U, V>, CoordinatorError> {
        self.validate_version_location_phase(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::Committing,
            PayloadPhase::Verified,
        )?;
        let undo = self.causal_undo_hashes(std::slice::from_ref(&lease.hash));
        self.with_entry_undo(&undo, |coordinator| {
            coordinator.mark_children_invalid(&lease.hash, &lease.hash)?;
            let entry = coordinator.remove_present_apply(&lease.hash)?;
            coordinator.apply_fault_checkpoint();
            Ok(Self::terminal_record(
                lease.hash.clone(),
                entry,
                disposition,
            ))
        })
    }

    pub(crate) fn commit_handoff(
        &mut self,
        lease: &CommitLease<V>,
    ) -> Result<CommitHandoff<R, V>, CoordinatorError> {
        self.validate_version_location_phase(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::Committing,
            PayloadPhase::Verified,
        )?;
        if self
            .entries
            .get(&lease.hash)
            .is_some_and(|entry| entry.candidate().is_some())
        {
            return Err(CoordinatorError::ConflictInvariant);
        }
        let undo = self.causal_undo_hashes(std::slice::from_ref(&lease.hash));
        self.with_entry_undo(&undo, |coordinator| {
            let ready_children = coordinator.parent_available(&lease.hash)?;
            let entry = coordinator.remove_present_apply(&lease.hash)?;
            let EntryState::PlainVerified {
                raw,
                payload: verified,
                location: PlainVerifiedLocation::Committing,
            } = entry.state
            else {
                return Err(CoordinatorError::ConflictInvariant);
            };
            Ok(CommitHandoff {
                hash: lease.hash.clone(),
                short_id: entry.short_id,
                raw,
                verified,
                peer: entry.source.peer(),
                source: entry.source,
                ready_children,
            })
        })
    }

    pub(crate) fn commit_candidate_handoff(
        &mut self,
        lease: &CommitLease<V>,
    ) -> Result<ConflictCommitHandoff<R, U, V>, CoordinatorError> {
        self.validate_version_location_phase(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::Committing,
            PayloadPhase::Verified,
        )?;
        let winner_inputs = self
            .entries
            .get(&lease.hash)
            .and_then(CoordinatorEntry::candidate)
            .map(|candidate| candidate.inputs.clone())
            .ok_or(CoordinatorError::ConflictInvariant)?;
        let mut rejected = HashSet::new();
        for input in &winner_inputs {
            if let Some(candidates) = self.candidates_by_input.get(input) {
                rejected.extend(
                    candidates
                        .iter()
                        .filter(|hash| *hash != &lease.hash)
                        .cloned(),
                );
            }
        }
        for hash in &rejected {
            let entry = self
                .entries
                .get(hash)
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            let candidate = entry
                .candidate()
                .ok_or(CoordinatorError::ConflictInvariant)?;
            if candidate.inputs.is_disjoint(&winner_inputs)
                || matches!(
                    entry.location(),
                    CoordinatorLocation::ReadyToCommit | CoordinatorLocation::Committing
                )
            {
                return Err(CoordinatorError::ConflictInvariant);
            }
            self.preflight_remove_conflict_indexes(hash)?;
        }
        self.preflight_remove_conflict_indexes(&lease.hash)?;

        let mut rejected: Vec<_> = rejected.into_iter().collect();
        rejected.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        let mut terminal = Vec::new();
        terminal
            .try_reserve(rejected.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        let mut undo = rejected.clone();
        undo.push(lease.hash.clone());
        let causal_undo = self.causal_undo_hashes(&undo);
        undo.extend(causal_undo);
        let directly_affected = undo.clone();
        for hash in &directly_affected {
            if let Some(waiters) = self.waiters_by_blocker.get(hash) {
                undo.extend(waiters.iter().cloned());
            }
        }
        self.with_entry_undo(&undo, |coordinator| {
            for hash in rejected {
                coordinator.mark_children_invalid(&hash, &hash)?;
                let entry = coordinator.remove_present_apply(&hash)?;
                terminal.push(Self::terminal_record(
                    hash,
                    entry,
                    TerminalDisposition::Rejected,
                ));
                coordinator.apply_fault_checkpoint();
            }
            let ready_children = coordinator.parent_available(&lease.hash)?;
            let entry = coordinator.remove_present_apply(&lease.hash)?;
            coordinator.apply_fault_checkpoint();
            let EntryState::CandidateVerified {
                raw,
                payload: verified,
                location: CandidateLocation::Committing,
                ..
            } = entry.state
            else {
                return Err(CoordinatorError::ConflictInvariant);
            };
            Ok(ConflictCommitHandoff {
                winner: CommitHandoff {
                    hash: lease.hash.clone(),
                    short_id: entry.short_id,
                    raw,
                    verified,
                    peer: entry.source.peer(),
                    source: entry.source,
                    ready_children,
                },
                rejected: terminal,
            })
        })
    }

    pub(crate) fn commit_any_handoff(
        &mut self,
        lease: &CommitLease<V>,
    ) -> Result<ConflictCommitHandoff<R, U, V>, CoordinatorError> {
        if self
            .entries
            .get(&lease.hash)
            .is_some_and(|entry| entry.candidate().is_some())
        {
            self.commit_candidate_handoff(lease)
        } else {
            self.commit_handoff(lease)
                .map(|winner| ConflictCommitHandoff {
                    winner,
                    rejected: Vec::new(),
                })
        }
    }

    fn commit_handoff_undo_hashes(
        &self,
        lease: &CommitLease<V>,
    ) -> Result<Vec<Byte32>, CoordinatorError> {
        self.validate_version_location_phase(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::Committing,
            PayloadPhase::Verified,
        )?;
        let mut roots = vec![lease.hash.clone()];
        if let Some(candidate) = self
            .entries
            .get(&lease.hash)
            .and_then(CoordinatorEntry::candidate)
        {
            let mut rejected = HashSet::new();
            for input in &candidate.inputs {
                if let Some(candidates) = self.candidates_by_input.get(input) {
                    rejected.extend(
                        candidates
                            .iter()
                            .filter(|hash| *hash != &lease.hash)
                            .cloned(),
                    );
                }
            }
            roots.extend(rejected);
        }
        let mut undo = self.causal_undo_hashes(&roots);
        let direct = undo.clone();
        for hash in direct {
            if let Some(waiters) = self.waiters_by_blocker.get(&hash) {
                undo.extend(waiters.iter().cloned());
            }
        }
        Ok(undo)
    }

    /// Finalize a coordinator winner and demote consumers of every accepted
    /// pool entry removed by the same commit as one undo-protected ownership
    /// transaction. The outer snapshot is intentionally targeted to the
    /// bounded conflict/dependency cohorts; it never clones the whole
    /// coordinator on the commit hot path.
    pub(crate) fn commit_any_handoff_with_unavailable_parents(
        &mut self,
        lease: &CommitLease<V>,
        unavailable_parents: &HashSet<Byte32>,
    ) -> Result<ConflictCommitHandoff<R, U, V>, CoordinatorError> {
        let mut undo = self.commit_handoff_undo_hashes(lease)?;
        undo.extend(self.parents_unavailable_undo_hashes(unavailable_parents));
        undo.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        undo.dedup();
        self.with_entry_undo(&undo, |coordinator| {
            coordinator.parents_unavailable(unavailable_parents)?;
            coordinator.commit_any_handoff(lease)
        })
    }

    /// Consume a transaction that became committed through an attached block
    /// rather than this coordinator's submit path. Chain membership is
    /// authoritative: dependents are woken, not invalidated, and every stale
    /// worker/commit lease becomes harmless when the entry is removed.
    pub(crate) fn external_commit(
        &mut self,
        hash: &Byte32,
    ) -> Result<Option<ExternalCommitRecord<R>>, CoordinatorError> {
        let mut undo = self.causal_undo_hashes(std::slice::from_ref(hash));
        if self.entries.contains_key(hash) {
            self.with_entry_undo(&undo, |coordinator| coordinator.external_commit_apply(hash))
        } else {
            undo.retain(|affected| affected != hash);
            self.with_absent_entry_undo(hash, &undo, |coordinator| {
                coordinator.external_commit_apply(hash)
            })
        }
    }

    /// Synchronous Local/reorg commit counterpart to
    /// [`Self::commit_any_handoff_with_unavailable_parents`]. The new pool
    /// member may not have been coordinator-resident, but consumers of every
    /// journaled pool removal still demote in the same transition that wakes
    /// consumers of the committed hash.
    pub(crate) fn external_commit_with_unavailable_parents(
        &mut self,
        hash: &Byte32,
        unavailable_parents: &HashSet<Byte32>,
    ) -> Result<Option<ExternalCommitRecord<R>>, CoordinatorError> {
        let mut undo = self.causal_undo_hashes(std::slice::from_ref(hash));
        undo.extend(self.parents_unavailable_undo_hashes(unavailable_parents));
        undo.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        undo.dedup();
        if self.entries.contains_key(hash) {
            self.with_entry_undo(&undo, |coordinator| {
                coordinator.parents_unavailable(unavailable_parents)?;
                coordinator.external_commit_apply(hash)
            })
        } else {
            undo.retain(|affected| affected != hash);
            self.with_absent_entry_undo(hash, &undo, |coordinator| {
                coordinator.parents_unavailable(unavailable_parents)?;
                coordinator.external_commit_apply(hash)
            })
        }
    }

    /// Apply a reorg's complete membership delta to the coordinator in one
    /// targeted transaction. `committed` parents remain available and wake
    /// consumers; `unavailable_parents` were physically removed from the
    /// accepted pool and demote consumers. The snapshot records both present
    /// and absent committed hashes because attached transactions need not
    /// have entered this node's pipeline.
    pub(crate) fn external_commits_with_unavailable_parents(
        &mut self,
        committed: &HashSet<Byte32>,
        unavailable_parents: &HashSet<Byte32>,
    ) -> Result<Vec<ExternalCommitRecord<R>>, CoordinatorError> {
        let mut undo = self.parents_unavailable_undo_hashes(unavailable_parents);
        for hash in committed {
            undo.extend(self.causal_undo_hashes(std::slice::from_ref(hash)));
        }
        undo.extend(committed.iter().cloned());
        undo.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        undo.dedup();
        let mut ordered_committed: Vec<_> = committed.iter().cloned().collect();
        ordered_committed.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        self.with_mixed_entry_undo(&undo, |coordinator| {
            coordinator.parents_unavailable(unavailable_parents)?;
            let mut records = Vec::new();
            records
                .try_reserve(ordered_committed.len())
                .map_err(|_| CoordinatorError::QueueReservationFailed)?;
            for hash in ordered_committed {
                if let Some(record) = coordinator.external_commit_apply(&hash)? {
                    records.push(record);
                }
                coordinator.apply_fault_checkpoint();
            }
            Ok(records)
        })
    }

    fn external_commit_apply(
        &mut self,
        hash: &Byte32,
    ) -> Result<Option<ExternalCommitRecord<R>>, CoordinatorError> {
        let ready_children = self.parent_available(hash)?;
        if !self.entries.contains_key(hash) {
            // The parent may have entered through the synchronous local path
            // or an attached block without ever being coordinator resident.
            return Ok(None);
        }
        let entry = self.remove_present_apply(hash)?;
        self.apply_fault_checkpoint();
        Ok(Some(ExternalCommitRecord {
            hash: hash.clone(),
            short_id: entry.short_id,
            raw: Arc::clone(entry.state.raw()),
            source: entry.source,
            ready_children,
        }))
    }

    pub(crate) fn force_terminalize(
        &mut self,
        hash: &Byte32,
        disposition: TerminalDisposition,
    ) -> Result<Option<TerminalRecord<R, U, V>>, CoordinatorError> {
        if !self.entries.contains_key(hash) {
            return Ok(None);
        }
        if self.entries.get(hash).is_some_and(|entry| {
            matches!(
                &entry.state,
                EntryState::PlainVerified {
                    location: PlainVerifiedLocation::Committing,
                    ..
                } | EntryState::CandidateVerified {
                    location: CandidateLocation::Committing,
                    ..
                }
            )
        }) {
            return Err(CoordinatorError::CommitInProgress(hash.clone()));
        }
        let undo = self.causal_undo_hashes(std::slice::from_ref(hash));
        self.with_entry_undo(&undo, |coordinator| {
            coordinator.mark_children_invalid(hash, hash)?;
            let entry = coordinator.remove_present_apply(hash)?;
            coordinator.apply_fault_checkpoint();
            Ok(Some(Self::terminal_record(
                hash.clone(),
                entry,
                disposition,
            )))
        })
    }

    /// Expiry is incarnation-scoped rather than revision-scoped: ordinary
    /// stage transitions cannot extend a remote transaction's original
    /// lifetime, while removal/re-admission makes the old ticket stale.
    pub(crate) fn expire_due(
        &mut self,
        now: u64,
        max: usize,
    ) -> Result<Vec<TerminalRecord<R, U, V>>, CoordinatorError> {
        let capacity = max.min(self.live_deadlines.len());
        let mut selected = Vec::new();
        selected
            .try_reserve(capacity)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        while selected.len() < max {
            let Some(Reverse(ticket)) = self.deadlines.peek().cloned() else {
                break;
            };
            if ticket.expires_at > now {
                break;
            }
            let is_live = self
                .live_deadlines
                .get(&ticket.hash)
                .is_some_and(|live| live == &ticket);
            if !is_live {
                self.deadlines.pop();
                continue;
            }
            let entry = self
                .entries
                .get(&ticket.hash)
                .ok_or_else(|| CoordinatorError::Missing(ticket.hash.clone()))?;
            if entry.incarnation != ticket.incarnation
                || entry.expires_at != Some(ticket.expires_at)
            {
                return Err(CoordinatorError::ConflictInvariant);
            }
            if matches!(
                &entry.state,
                EntryState::PlainVerified {
                    location: PlainVerifiedLocation::Committing,
                    ..
                } | EntryState::CandidateVerified {
                    location: CandidateLocation::Committing,
                    ..
                }
            ) {
                break;
            }
            self.deadlines.pop();
            selected.push(ticket);
        }
        // Restore the selected physical tickets before any fallible undo
        // preparation. Successful removal makes them lazy-stale; rollback can
        // therefore rebuild only logical state without a deadline liveness gap.
        for ticket in &selected {
            self.deadlines.push(Reverse(ticket.clone()));
        }
        let roots: Vec<_> = selected.iter().map(|ticket| ticket.hash.clone()).collect();
        let affected = self.causal_undo_hashes(&roots);
        let mut terminal = Vec::new();
        terminal
            .try_reserve(selected.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.with_entry_undo(&affected, |coordinator| {
            for ticket in selected {
                coordinator.mark_children_invalid(&ticket.hash, &ticket.hash)?;
                let entry = coordinator.remove_present_apply(&ticket.hash)?;
                terminal.push(Self::terminal_record(
                    ticket.hash,
                    entry,
                    TerminalDisposition::Expired,
                ));
                coordinator.apply_fault_checkpoint();
            }
            Ok(terminal)
        })
    }

    pub(crate) fn clear(&mut self) -> Result<Vec<TerminalRecord<R, U, V>>, CoordinatorError> {
        // Clear is one ownership transaction, not N conflict removals. It must
        // not wake/revise records that are themselves being cleared, and stale
        // worker leases become harmless because re-admission receives a new
        // incarnation.
        let mut terminal = Vec::new();
        terminal
            .try_reserve(self.entries.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.apply_fault_checkpoint();
        let entries = std::mem::take(&mut self.entries);
        for (hash, entry) in entries {
            terminal.push(Self::terminal_record(
                hash,
                entry,
                TerminalDisposition::Cleared,
            ));
        }
        self.by_short_id.clear();
        self.by_peer.clear();
        self.by_parent.clear();
        self.dependency_failures.clear();
        self.dependency_failure_set.clear();
        self.candidates_by_input.clear();
        self.active_by_input.clear();
        self.waiters_by_blocker.clear();
        self.conflict_rechecks.clear();
        self.conflict_recheck_set.clear();
        self.conflict_edge_count = 0;
        self.deadlines.clear();
        self.live_deadlines.clear();
        for queue in self.queues.values_mut() {
            queue.clear();
        }
        self.global_usage = CoordinatorResidency::default();
        self.peer_usage.clear();
        self.active_work = 0;
        self.active_work_by_peer.clear();
        Ok(terminal)
    }

    #[cfg(test)]
    pub(crate) fn set_revision_for_test(
        &mut self,
        hash: &Byte32,
        revision: u64,
    ) -> Result<(), CoordinatorError> {
        let entry = self
            .entries
            .get_mut(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        let old_ticket = entry.ticket(hash);
        entry.revision = revision;
        if let Some(kind) = entry.queue_kind() {
            let new_ticket = entry.ticket(hash);
            let front = entry.source.is_proposal();
            let owner = entry.source.queue_owner();
            let queue = self.queue_mut(kind)?;
            queue.remove_live(&old_ticket);
            queue.reserve_live(front, owner)?;
            queue.push_reserved(kind, new_ticket, front)?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn set_next_maintenance_sequence_for_test(&mut self, sequence: u64) {
        self.next_maintenance_sequence = sequence;
    }

    #[cfg(test)]
    pub(crate) fn set_next_queue_sequence_for_test(&mut self, sequence: u64) {
        self.next_queue_sequence = sequence;
    }

    #[cfg(test)]
    pub(crate) fn rebuild_derived_indexes_for_test(&mut self) -> Result<(), CoordinatorError> {
        self.rebuild_derived_indexes()
    }

    #[cfg(test)]
    pub(crate) fn physical_queue_slots_for_test(&self, kind: QueueKind) -> usize {
        self.queues.get(&kind).map_or(0, TicketQueue::physical_len)
    }

    #[cfg(test)]
    pub(crate) fn physical_deadline_slots_for_test(&self) -> usize {
        self.deadlines.len()
    }

    #[cfg(test)]
    pub(crate) fn set_apply_fault_for_test(&mut self, after: Option<usize>) {
        self.fault_after_apply_steps = after;
        self.apply_steps_seen = 0;
    }

    #[cfg(test)]
    fn apply_fault_checkpoint(&mut self) {
        self.apply_steps_seen = self.apply_steps_seen.saturating_add(1);
        if self.fault_after_apply_steps == Some(self.apply_steps_seen) {
            std::panic::panic_any("injected coordinator apply fault");
        }
    }

    #[cfg(not(test))]
    #[inline(always)]
    fn apply_fault_checkpoint(&mut self) {}

    fn queue_mut(&mut self, kind: QueueKind) -> Result<&mut TicketQueue, CoordinatorError> {
        self.queues
            .get_mut(&kind)
            .ok_or(CoordinatorError::QueueInvariant(kind))
    }

    fn peek_live_ticket(
        &mut self,
        kind: QueueKind,
        capability: WorkerCapability,
    ) -> Result<Option<CoordinatorTicket>, CoordinatorError> {
        if self.active_work >= self.limits.max_active_work {
            return Ok(None);
        }
        let per_peer_limit = self.limits.max_active_work_per_peer;
        let active_by_peer = &self.active_work_by_peer;
        let queue = self
            .queues
            .get_mut(&kind)
            .ok_or(CoordinatorError::QueueInvariant(kind))?;
        Ok(queue.peek_eligible(|ticket| {
            let source_eligible = match ticket.owner {
                QueueOwner::Trusted => true,
                QueueOwner::Remote(peer) => {
                    active_by_peer.get(&peer).copied().unwrap_or(0) < per_peer_limit
                }
            };
            source_eligible && TicketQueue::ticket_is_eligible(ticket, capability)
        }))
    }

    fn consume_front_ticket(
        &mut self,
        kind: QueueKind,
        ticket: &CoordinatorTicket,
    ) -> Result<(), CoordinatorError> {
        let queue = self.queue_mut(kind)?;
        queue.consume(kind, ticket)
    }

    fn remove_current_queue_ticket(&mut self, hash: &Byte32) -> Result<(), CoordinatorError> {
        let Some(entry) = self.entries.get(hash) else {
            return Err(CoordinatorError::Missing(hash.clone()));
        };
        let Some(kind) = entry.queue_kind() else {
            return Ok(());
        };
        let ticket = entry.ticket(hash);
        let queue = self.queue_mut(kind)?;
        queue.remove_live(&ticket);
        queue.compact();
        Ok(())
    }

    fn validate_version_location_phase(
        &self,
        hash: &Byte32,
        version: CoordinatorVersion,
        expected_location: &CoordinatorLocation,
        expected_phase: PayloadPhase,
    ) -> Result<(), CoordinatorError> {
        let entry = self
            .entries
            .get(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        if entry.incarnation != version.incarnation {
            return Err(CoordinatorError::IncarnationMismatch {
                expected: version.incarnation,
                actual: entry.incarnation,
            });
        }
        if entry.revision != version.revision {
            return Err(CoordinatorError::RevisionMismatch {
                expected: version.revision,
                actual: entry.revision,
            });
        }
        if let Some(parent) = entry.dependencies.iter().find(|parent| {
            self.entries
                .get(*parent)
                .is_some_and(|parent_entry| parent_entry.invalidated_cause().is_some())
        }) {
            return Err(CoordinatorError::DependencyInvalidated {
                child: hash.clone(),
                parent: parent.clone(),
            });
        }
        let actual_location = entry.location();
        if actual_location != *expected_location {
            return Err(CoordinatorError::LocationMismatch {
                expected: expected_location.clone(),
                actual: actual_location,
            });
        }
        let actual_phase = entry.phase_kind();
        if actual_phase != expected_phase {
            return Err(CoordinatorError::PhaseMismatch {
                expected: expected_phase,
                actual: actual_phase,
            });
        }
        Ok(())
    }

    fn ensure_revision_capacity(&self, hash: &Byte32) -> Result<(), CoordinatorError> {
        let entry = self
            .entries
            .get(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        if entry.revision == u64::MAX {
            return Err(CoordinatorError::RevisionExhausted(hash.clone()));
        }
        Ok(())
    }

    fn maintenance_sequence_range(&self, count: usize) -> Result<(u64, u64), CoordinatorError> {
        let count =
            u64::try_from(count).map_err(|_| CoordinatorError::MaintenanceSequenceExhausted)?;
        let first = self.next_maintenance_sequence;
        let next = first
            .checked_add(count)
            .ok_or(CoordinatorError::MaintenanceSequenceExhausted)?;
        Ok((first, next))
    }

    fn queue_sequence_range(&self, count: usize) -> Result<(u64, u64), CoordinatorError> {
        let count = u64::try_from(count).map_err(|_| CoordinatorError::QueueSequenceExhausted)?;
        let first = self.next_queue_sequence;
        let next = first
            .checked_add(count)
            .ok_or(CoordinatorError::QueueSequenceExhausted)?;
        Ok((first, next))
    }

    fn source_capacity_strength(source: CoordinatorSource) -> u8 {
        match source {
            CoordinatorSource::Remote(_) => 0,
            CoordinatorSource::Local => 1,
            CoordinatorSource::Proposal => 2,
        }
    }

    fn dependency_capacity_victims(
        &self,
        source: CoordinatorSource,
        dependencies: &HashSet<Byte32>,
        protected: &HashSet<Byte32>,
    ) -> Result<Vec<Byte32>, CoordinatorError> {
        let mut parents: Vec<_> = dependencies.iter().cloned().collect();
        parents.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        let mut selected = HashSet::new();
        let mut victims = Vec::new();
        for parent in parents {
            let Some(children) = self.by_parent.get(&parent) else {
                continue;
            };
            let occupied = children
                .iter()
                .filter(|child| !selected.contains(*child))
                .count();
            if occupied < self.limits.max_dependents_per_parent {
                continue;
            }
            let incoming_strength = Self::source_capacity_strength(source);
            let victim = children
                .iter()
                .filter(|child| !selected.contains(*child))
                .filter(|child| !protected.contains(*child))
                .filter_map(|child| self.entries.get(child).map(|entry| (child, entry)))
                .filter(|(_, entry)| {
                    !entry.is_committing()
                        && (entry.invalidated_cause().is_some()
                            || Self::source_capacity_strength(entry.source) < incoming_strength)
                })
                .min_by(|(left_hash, left), (right_hash, right)| {
                    left.invalidated_cause()
                        .is_none()
                        .cmp(&right.invalidated_cause().is_none())
                        .then_with(|| {
                            Self::source_capacity_strength(left.source)
                                .cmp(&Self::source_capacity_strength(right.source))
                        })
                        .then_with(|| right.queue_sequence.cmp(&left.queue_sequence))
                        .then_with(|| left_hash.as_slice().cmp(right_hash.as_slice()))
                })
                .map(|(hash, _)| hash.clone())
                .ok_or_else(|| CoordinatorError::ParentFanoutLimitExceeded(parent.clone()))?;
            selected.insert(victim.clone());
            victims.push(victim);
        }
        victims.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        Ok(victims)
    }

    fn dependency_ancestor_closure(
        &self,
        owner: &Byte32,
        dependencies: &HashSet<Byte32>,
    ) -> Result<HashSet<Byte32>, CoordinatorError> {
        let mut ancestors = HashSet::new();
        let mut pending: Vec<_> = dependencies.iter().cloned().collect();
        while let Some(hash) = pending.pop() {
            if &hash == owner {
                return Err(CoordinatorError::DependencyCycle(owner.clone()));
            }
            if !ancestors.insert(hash.clone()) {
                continue;
            }
            if ancestors.len() > self.limits.max_dependency_ancestors {
                return Err(CoordinatorError::DependencyAncestorLimitExceeded);
            }
            if let Some(entry) = self.entries.get(&hash) {
                pending.extend(entry.dependencies.iter().cloned());
            }
        }
        Ok(ancestors)
    }

    fn check_peer_budget_after_victims(
        &self,
        incoming_hash: Option<&Byte32>,
        incoming_source: CoordinatorSource,
        incoming_charge_bytes: usize,
        victims: &HashSet<Byte32>,
    ) -> Result<(), CoordinatorError> {
        let (Some(peer), Some(limit)) = (incoming_source.peer(), self.limits.per_peer) else {
            return Ok(());
        };
        let mut projected = self.peer_usage(peer);
        if let Some(hash) = incoming_hash {
            let old = self
                .entries
                .get(hash)
                .filter(|entry| entry.source.peer() == Some(peer))
                .map(|entry| CoordinatorResidency::new(1, entry.charge_bytes))
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            projected = projected
                .checked_sub(old)
                .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
        }
        for hash in victims {
            let Some(entry) = self.entries.get(hash) else {
                return Err(CoordinatorError::Missing(hash.clone()));
            };
            if entry.source.peer() == Some(peer) {
                projected = projected
                    .checked_sub(CoordinatorResidency::new(1, entry.charge_bytes))
                    .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
            }
        }
        projected = projected
            .checked_add(CoordinatorResidency::new(1, incoming_charge_bytes))
            .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
        if !projected.fits(limit) {
            return Err(CoordinatorError::PeerBudgetExceeded(peer));
        }
        Ok(())
    }

    fn global_capacity_victims(
        &self,
        incoming_hash: Option<&Byte32>,
        incoming_source: CoordinatorSource,
        incoming_charge_bytes: usize,
        preselected: &HashSet<Byte32>,
        protected: &HashSet<Byte32>,
    ) -> Result<Vec<Byte32>, CoordinatorError> {
        if preselected.len() > self.limits.max_capacity_evictions_per_transition {
            return Err(CoordinatorError::CapacityEvictionLimitExceeded);
        }
        let mut projected = self.global_usage;
        if let Some(hash) = incoming_hash {
            let old = self
                .entries
                .get(hash)
                .map(|entry| CoordinatorResidency::new(1, entry.charge_bytes))
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            projected = projected
                .checked_sub(old)
                .ok_or(CoordinatorError::GlobalBudgetExceeded)?;
        }
        for hash in preselected {
            let charge = self
                .entries
                .get(hash)
                .map(|entry| CoordinatorResidency::new(1, entry.charge_bytes))
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            projected = projected
                .checked_sub(charge)
                .ok_or(CoordinatorError::GlobalBudgetExceeded)?;
        }
        projected = projected
            .checked_add(CoordinatorResidency::new(1, incoming_charge_bytes))
            .ok_or(CoordinatorError::GlobalBudgetExceeded)?;

        let mut selected = preselected.clone();
        let mut victims = Vec::new();
        while !projected.fits(self.limits.global) {
            if selected.len() >= self.limits.max_capacity_evictions_per_transition {
                return Err(CoordinatorError::CapacityEvictionLimitExceeded);
            }
            let incoming_strength = Self::source_capacity_strength(incoming_source);
            let victim = self
                .entries
                .iter()
                .filter(|(hash, _)| incoming_hash != Some(*hash))
                .filter(|(hash, _)| !selected.contains(*hash) && !protected.contains(*hash))
                .filter(|(_, entry)| {
                    !entry.is_committing()
                        && (entry.invalidated_cause().is_some()
                            || Self::source_capacity_strength(entry.source) < incoming_strength)
                })
                .min_by(|(left_hash, left), (right_hash, right)| {
                    left.invalidated_cause()
                        .is_none()
                        .cmp(&right.invalidated_cause().is_none())
                        .then_with(|| {
                            Self::source_capacity_strength(left.source)
                                .cmp(&Self::source_capacity_strength(right.source))
                        })
                        .then_with(|| right.charge_bytes.cmp(&left.charge_bytes))
                        .then_with(|| right.queue_sequence.cmp(&left.queue_sequence))
                        .then_with(|| left_hash.as_slice().cmp(right_hash.as_slice()))
                })
                .map(|(hash, entry)| (hash.clone(), entry.charge_bytes))
                .ok_or(CoordinatorError::GlobalBudgetExceeded)?;
            selected.insert(victim.0.clone());
            projected = projected
                .checked_sub(CoordinatorResidency::new(1, victim.1))
                .ok_or(CoordinatorError::GlobalBudgetExceeded)?;
            victims.push(victim.0);
        }
        victims.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        Ok(victims)
    }

    fn compare_candidate_capacity(
        left_hash: &Byte32,
        left_source: CoordinatorSource,
        left: &CandidateMeta,
        right_hash: &Byte32,
        right_source: CoordinatorSource,
        right: &CandidateMeta,
    ) -> Ordering {
        Self::source_capacity_strength(left_source)
            .cmp(&Self::source_capacity_strength(right_source))
            .then_with(|| Self::compare_candidates(left_hash, left, right_hash, right))
    }

    fn conflict_capacity_victims(
        &self,
        incoming_hash: &Byte32,
        incoming_source: CoordinatorSource,
        incoming: &CandidateMeta,
        protected: &HashSet<Byte32>,
    ) -> Result<Vec<Byte32>, CoordinatorError> {
        let mut inputs: Vec<_> = incoming.inputs.iter().cloned().collect();
        inputs.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        let mut selected = HashSet::new();
        let mut victims = Vec::new();
        for input in inputs {
            let Some(candidates) = self.candidates_by_input.get(&input) else {
                continue;
            };
            let occupied = candidates
                .iter()
                .filter(|hash| !selected.contains(*hash))
                .count();
            if occupied < self.limits.max_candidates_per_input {
                continue;
            }
            let victim = candidates
                .iter()
                .filter(|hash| !selected.contains(*hash))
                .filter(|hash| !protected.contains(*hash))
                .filter_map(|hash| {
                    self.entries.get(hash).and_then(|entry| {
                        entry.candidate().map(|candidate| (hash, entry, candidate))
                    })
                })
                .filter(|(hash, entry, candidate)| {
                    !entry.is_committing()
                        && Self::compare_candidate_capacity(
                            incoming_hash,
                            incoming_source,
                            incoming,
                            hash,
                            entry.source,
                            candidate,
                        ) == Ordering::Greater
                })
                .min_by(
                    |(left_hash, left_entry, left), (right_hash, right_entry, right)| {
                        Self::compare_candidate_capacity(
                            left_hash,
                            left_entry.source,
                            left,
                            right_hash,
                            right_entry.source,
                            right,
                        )
                    },
                )
                .map(|(hash, _, _)| hash.clone())
                .ok_or_else(|| CoordinatorError::ConflictCandidateLimitExceeded(input.clone()))?;
            selected.insert(victim.clone());
            victims.push(victim);
        }
        let mut projected_edges = self.conflict_edge_count;
        for hash in &selected {
            let edges = self
                .entries
                .get(hash)
                .and_then(CoordinatorEntry::candidate)
                .map(|candidate| candidate.inputs.len())
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            projected_edges = projected_edges
                .checked_sub(edges)
                .ok_or(CoordinatorError::ConflictEdgeLimitExceeded)?;
        }
        projected_edges = projected_edges
            .checked_add(incoming.inputs.len())
            .ok_or(CoordinatorError::ConflictEdgeLimitExceeded)?;
        while projected_edges > self.limits.max_conflict_edges {
            if selected.len() >= self.limits.max_capacity_evictions_per_transition {
                return Err(CoordinatorError::CapacityEvictionLimitExceeded);
            }
            let (victim, edges) = self
                .entries
                .iter()
                .filter(|(hash, _)| *hash != incoming_hash)
                .filter(|(hash, _)| !selected.contains(*hash) && !protected.contains(*hash))
                .filter_map(|(hash, entry)| {
                    entry.candidate().map(|candidate| (hash, entry, candidate))
                })
                .filter(|(hash, entry, candidate)| {
                    !entry.is_committing()
                        && Self::compare_candidate_capacity(
                            incoming_hash,
                            incoming_source,
                            incoming,
                            hash,
                            entry.source,
                            candidate,
                        ) == Ordering::Greater
                })
                .min_by(
                    |(left_hash, left_entry, left), (right_hash, right_entry, right)| {
                        Self::compare_candidate_capacity(
                            left_hash,
                            left_entry.source,
                            left,
                            right_hash,
                            right_entry.source,
                            right,
                        )
                    },
                )
                .map(|(hash, _, candidate)| (hash.clone(), candidate.inputs.len()))
                .ok_or(CoordinatorError::ConflictEdgeLimitExceeded)?;
            selected.insert(victim.clone());
            victims.push(victim);
            projected_edges = projected_edges
                .checked_sub(edges)
                .ok_or(CoordinatorError::ConflictEdgeLimitExceeded)?;
        }
        victims.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        Ok(victims)
    }

    fn check_activate_source(&self, source: CoordinatorSource) -> Result<(), CoordinatorError> {
        if self.active_work >= self.limits.max_active_work {
            return Err(CoordinatorError::ActiveWorkLimitExceeded);
        }
        if let Some(peer) = source.peer()
            && self.peer_active_work(peer) >= self.limits.max_active_work_per_peer
        {
            return Err(CoordinatorError::PeerActiveWorkLimitExceeded(peer));
        }
        Ok(())
    }

    fn activate_source(&mut self, source: CoordinatorSource) -> Result<(), CoordinatorError> {
        self.check_activate_source(source)?;
        self.active_work = self
            .active_work
            .checked_add(1)
            .ok_or(CoordinatorError::ActiveWorkLimitExceeded)?;
        if let Some(peer) = source.peer() {
            let active = self.active_work_by_peer.entry(peer).or_default();
            *active = active
                .checked_add(1)
                .ok_or(CoordinatorError::PeerActiveWorkLimitExceeded(peer))?;
        }
        Ok(())
    }

    fn deactivate_source(&mut self, source: CoordinatorSource) -> Result<(), CoordinatorError> {
        self.active_work = self
            .active_work
            .checked_sub(1)
            .ok_or(CoordinatorError::ActiveWorkLimitExceeded)?;
        if let Some(peer) = source.peer() {
            let remove = {
                let active = self
                    .active_work_by_peer
                    .get_mut(&peer)
                    .ok_or(CoordinatorError::PeerActiveWorkLimitExceeded(peer))?;
                *active = active
                    .checked_sub(1)
                    .ok_or(CoordinatorError::PeerActiveWorkLimitExceeded(peer))?;
                *active == 0
            };
            if remove {
                self.active_work_by_peer.remove(&peer);
            }
        }
        Ok(())
    }

    fn metadata_charge_bytes(
        &self,
        dependencies: usize,
        has_deadline: bool,
        conflict_inputs: usize,
    ) -> Result<usize, CoordinatorError> {
        let cost = self.limits.metadata_cost;
        let mut bytes = cost
            .entry_bytes
            .checked_add(cost.lifecycle_ticket_bytes)
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        bytes = bytes
            .checked_add(
                dependencies
                    .checked_mul(cost.dependency_edge_bytes)
                    .ok_or(CoordinatorError::ResidencyChargeOverflow)?,
            )
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        if has_deadline {
            bytes = bytes
                .checked_add(cost.deadline_ticket_bytes)
                .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        }
        bytes = bytes
            .checked_add(
                conflict_inputs
                    .checked_mul(cost.conflict_edge_bytes)
                    .ok_or(CoordinatorError::ResidencyChargeOverflow)?,
            )
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        Ok(bytes)
    }

    fn check_add_budget(
        &self,
        peer: Option<PeerIndex>,
        charge: CoordinatorResidency,
    ) -> Result<(), CoordinatorError> {
        let next_global = self
            .global_usage
            .checked_add(charge)
            .ok_or(CoordinatorError::GlobalBudgetExceeded)?;
        if !next_global.fits(self.limits.global) {
            return Err(CoordinatorError::GlobalBudgetExceeded);
        }
        if let (Some(peer), Some(limit)) = (peer, self.limits.per_peer) {
            let next_peer = self
                .peer_usage(peer)
                .checked_add(charge)
                .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
            if !next_peer.fits(limit) {
                return Err(CoordinatorError::PeerBudgetExceeded(peer));
            }
        }
        Ok(())
    }

    fn check_recharge(&self, hash: &Byte32, new_bytes: usize) -> Result<(), CoordinatorError> {
        let entry = self
            .entries
            .get(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        let old = CoordinatorResidency::new(1, entry.charge_bytes);
        let new = CoordinatorResidency::new(1, new_bytes);
        let next_global = self
            .global_usage
            .checked_sub(old)
            .and_then(|usage| usage.checked_add(new))
            .ok_or(CoordinatorError::GlobalBudgetExceeded)?;
        if !next_global.fits(self.limits.global) {
            return Err(CoordinatorError::GlobalBudgetExceeded);
        }
        if let (Some(peer), Some(limit)) = (entry.source.peer(), self.limits.per_peer) {
            let next_peer = self
                .peer_usage(peer)
                .checked_sub(old)
                .and_then(|usage| usage.checked_add(new))
                .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
            if !next_peer.fits(limit) {
                return Err(CoordinatorError::PeerBudgetExceeded(peer));
            }
        }
        Ok(())
    }

    fn apply_recharge(&mut self, hash: &Byte32, new_bytes: usize) -> Result<(), CoordinatorError> {
        let (peer, old_bytes) = self
            .entries
            .get(hash)
            .map(|entry| (entry.source.peer(), entry.charge_bytes))
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        let old = CoordinatorResidency::new(1, old_bytes);
        let new = CoordinatorResidency::new(1, new_bytes);
        self.global_usage = self
            .global_usage
            .checked_sub(old)
            .and_then(|usage| usage.checked_add(new))
            .ok_or(CoordinatorError::GlobalBudgetExceeded)?;
        if let Some(peer) = peer {
            let usage = self
                .peer_usage
                .get_mut(&peer)
                .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
            *usage = usage
                .checked_sub(old)
                .and_then(|usage| usage.checked_add(new))
                .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
        }
        self.entries
            .get_mut(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?
            .charge_bytes = new_bytes;
        Ok(())
    }

    fn with_capacity_victims<T, F>(
        &mut self,
        subject: CapacitySubject,
        victims: Vec<Byte32>,
        apply_subject: F,
    ) -> Result<(T, Vec<TerminalRecord<R, U, V>>), CoordinatorError>
    where
        F: FnOnce(&mut Self) -> Result<T, CoordinatorError>,
    {
        if victims.len() > self.limits.max_capacity_evictions_per_transition {
            return Err(CoordinatorError::CapacityEvictionLimitExceeded);
        }
        let mut terminal = Vec::new();
        terminal
            .try_reserve(victims.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        let mut affected = self.causal_undo_hashes(&victims);
        for victim in &victims {
            self.preflight_remove_conflict_indexes(victim)?;
        }
        let transaction = move |coordinator: &mut Self| {
            for victim in victims {
                coordinator.mark_children_invalid(&victim, &victim)?;
                let entry = coordinator.remove_present_apply(&victim)?;
                terminal.push(Self::terminal_record(
                    victim,
                    entry,
                    TerminalDisposition::CapacityEvicted,
                ));
                coordinator.apply_fault_checkpoint();
            }
            let result = apply_subject(coordinator)?;
            coordinator.apply_fault_checkpoint();
            Ok((result, terminal))
        };
        match subject {
            CapacitySubject::Absent(hash) => {
                self.with_absent_entry_undo(&hash, &affected, transaction)
            }
            CapacitySubject::Present(hash) => {
                affected.push(hash);
                self.with_entry_undo(&affected, transaction)
            }
        }
    }

    fn with_entry_undo<T, F>(&mut self, hashes: &[Byte32], apply: F) -> Result<T, CoordinatorError>
    where
        F: FnOnce(&mut Self) -> Result<T, CoordinatorError>,
    {
        let mut unique = HashSet::new();
        unique
            .try_reserve(hashes.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        let mut snapshot = Vec::new();
        snapshot
            .try_reserve(hashes.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        for hash in hashes {
            if unique.insert(hash.clone()) {
                let entry = self
                    .entries
                    .get(hash)
                    .cloned()
                    .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
                snapshot.push((hash.clone(), Some(entry)));
            }
        }
        self.with_entry_snapshot(snapshot, apply)
    }

    fn with_absent_entry_undo<T, F>(
        &mut self,
        absent: &Byte32,
        hashes: &[Byte32],
        apply: F,
    ) -> Result<T, CoordinatorError>
    where
        F: FnOnce(&mut Self) -> Result<T, CoordinatorError>,
    {
        if self.entries.contains_key(absent) {
            return Err(CoordinatorError::DuplicateHash(absent.clone()));
        }
        let mut unique = HashSet::new();
        unique
            .try_reserve(hashes.len().saturating_add(1))
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        let mut snapshot = Vec::new();
        snapshot
            .try_reserve(hashes.len().saturating_add(1))
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        unique.insert(absent.clone());
        snapshot.push((absent.clone(), None));
        for hash in hashes {
            if unique.insert(hash.clone()) {
                let entry = self
                    .entries
                    .get(hash)
                    .cloned()
                    .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
                snapshot.push((hash.clone(), Some(entry)));
            }
        }
        self.with_entry_snapshot(snapshot, apply)
    }

    /// Snapshot the current presence/absence of an arbitrary bounded cohort.
    /// Reorg membership deltas legitimately mix coordinator-resident and
    /// never-admitted hashes, so neither `with_entry_undo` nor
    /// `with_absent_entry_undo` can express the transaction alone.
    fn with_mixed_entry_undo<T, F>(
        &mut self,
        hashes: &[Byte32],
        apply: F,
    ) -> Result<T, CoordinatorError>
    where
        F: FnOnce(&mut Self) -> Result<T, CoordinatorError>,
    {
        let mut unique = HashSet::new();
        unique
            .try_reserve(hashes.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        let mut snapshot = Vec::new();
        snapshot
            .try_reserve(hashes.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        for hash in hashes {
            if unique.insert(hash.clone()) {
                snapshot.push((hash.clone(), self.entries.get(hash).cloned()));
            }
        }
        self.with_entry_snapshot(snapshot, apply)
    }

    fn with_entry_snapshot<T, F>(
        &mut self,
        snapshot: Vec<(Byte32, Option<CoordinatorEntry<R, U, V>>)>,
        apply: F,
    ) -> Result<T, CoordinatorError>
    where
        F: FnOnce(&mut Self) -> Result<T, CoordinatorError>,
    {
        let next_incarnation = self.next_incarnation;
        let next_arrival = self.next_arrival;
        let next_maintenance_sequence = self.next_maintenance_sequence;
        let next_queue_sequence = self.next_queue_sequence;
        let outcome = catch_unwind(AssertUnwindSafe(|| apply(self)));
        match outcome {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => {
                self.restore_entry_snapshot(
                    snapshot,
                    next_incarnation,
                    next_arrival,
                    next_maintenance_sequence,
                    next_queue_sequence,
                )?;
                Err(error)
            }
            Err(payload) => {
                if let Err(error) = self.restore_entry_snapshot(
                    snapshot,
                    next_incarnation,
                    next_arrival,
                    next_maintenance_sequence,
                    next_queue_sequence,
                ) {
                    std::panic::panic_any(error);
                }
                resume_unwind(payload)
            }
        }
    }

    fn restore_entry_snapshot(
        &mut self,
        snapshot: Vec<(Byte32, Option<CoordinatorEntry<R, U, V>>)>,
        next_incarnation: u64,
        next_arrival: u64,
        next_maintenance_sequence: u64,
        next_queue_sequence: u64,
    ) -> Result<(), CoordinatorError> {
        for (hash, entry) in snapshot {
            if let Some(entry) = entry {
                self.entries.insert(hash, entry);
            } else {
                self.entries.remove(&hash);
            }
        }
        self.next_incarnation = next_incarnation;
        self.next_arrival = next_arrival;
        self.next_maintenance_sequence = next_maintenance_sequence;
        self.next_queue_sequence = next_queue_sequence;
        self.rebuild_derived_indexes()
    }

    fn mark_children_invalid(
        &mut self,
        parent: &Byte32,
        cause: &Byte32,
    ) -> Result<Vec<Byte32>, CoordinatorError> {
        let mut children: Vec<_> = self
            .by_parent
            .get(parent)
            .into_iter()
            .flat_map(|children| children.iter().cloned())
            .collect();
        children.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        children.retain(|child| {
            self.entries
                .get(child)
                .is_some_and(|entry| entry.invalidated_cause().is_none())
        });
        for child in &children {
            let (uses_active_slot, source, invalidated_charge) = {
                let entry = self
                    .entries
                    .get(child)
                    .ok_or_else(|| CoordinatorError::Missing(child.clone()))?;
                (
                    entry.uses_active_slot(),
                    entry.source,
                    entry
                        .resident_payload_bytes
                        .checked_add(entry.base_metadata_bytes)
                        .ok_or(CoordinatorError::ResidencyChargeOverflow)?,
                )
            };
            self.ensure_revision_capacity(child)?;
            if uses_active_slot
                && (self.active_work == 0
                    || source
                        .peer()
                        .is_some_and(|peer| self.peer_active_work(peer) == 0))
            {
                return Err(CoordinatorError::ActiveWorkLimitExceeded);
            }
            self.preflight_remove_conflict_indexes(child)?;
            self.check_recharge(child, invalidated_charge)?;
        }
        self.dependency_failures
            .try_reserve(children.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.dependency_failure_set
            .try_reserve(children.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        let (first_sequence, next_maintenance_sequence) =
            self.maintenance_sequence_range(children.len())?;
        let mut undo_hashes = children.clone();
        for child in &children {
            if let Some(waiters) = self.waiters_by_blocker.get(child) {
                undo_hashes.extend(waiters.iter().cloned());
            }
        }
        let result = children.clone();
        self.with_entry_undo(&undo_hashes, |coordinator| {
            coordinator.next_maintenance_sequence = next_maintenance_sequence;
            for (offset, child) in children.iter().enumerate() {
                let sequence = first_sequence
                    .checked_add(
                        u64::try_from(offset)
                            .map_err(|_| CoordinatorError::MaintenanceSequenceExhausted)?,
                    )
                    .ok_or(CoordinatorError::MaintenanceSequenceExhausted)?;
                let active_source = coordinator
                    .entries
                    .get(child)
                    .and_then(|entry| entry.uses_active_slot().then_some(entry.source));
                if let Some(source) = active_source {
                    coordinator.deactivate_source(source)?;
                }
                coordinator.remove_current_queue_ticket(child)?;
                coordinator.remove_conflict_indexes(child)?;
                coordinator.apply_fault_checkpoint();
                let invalidated_charge = coordinator
                    .entries
                    .get(child)
                    .and_then(|entry| {
                        entry
                            .resident_payload_bytes
                            .checked_add(entry.base_metadata_bytes)
                    })
                    .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
                coordinator.apply_recharge(child, invalidated_charge)?;
                let entry = coordinator
                    .entries
                    .get_mut(child)
                    .ok_or_else(|| CoordinatorError::Missing(child.clone()))?;
                let payload = match &entry.state {
                    EntryState::Raw { .. } => InvalidatedPayload::Raw,
                    EntryState::Unverified { payload, .. } => {
                        InvalidatedPayload::Unverified(Arc::clone(payload))
                    }
                    EntryState::PlainVerified { payload, .. }
                    | EntryState::CandidateVerified { payload, .. } => {
                        InvalidatedPayload::Verified(Arc::clone(payload))
                    }
                    EntryState::Invalidated { .. } => {
                        return Err(CoordinatorError::ConflictInvariant);
                    }
                };
                let raw = Arc::clone(entry.state.raw());
                entry.state = EntryState::Invalidated {
                    raw,
                    payload,
                    cause: cause.clone(),
                    sequence,
                };
                entry.metadata_bytes = entry.base_metadata_bytes;
                entry.revision += 1;
                if coordinator.dependency_failure_set.insert(child.clone()) {
                    coordinator.dependency_failures.push_back(child.clone());
                }
                coordinator.apply_fault_checkpoint();
            }
            Ok(result)
        })
    }

    fn causal_undo_hashes(&self, roots: &[Byte32]) -> Vec<Byte32> {
        let mut affected = roots.to_vec();
        for root in roots {
            if let Some(children) = self.by_parent.get(root) {
                affected.extend(children.iter().cloned());
            }
            if let Some(waiters) = self.waiters_by_blocker.get(root) {
                affected.extend(waiters.iter().cloned());
            }
        }
        let direct = affected.clone();
        for hash in direct {
            if let Some(waiters) = self.waiters_by_blocker.get(&hash) {
                affected.extend(waiters.iter().cloned());
            }
        }
        affected
    }

    fn remove_present_apply(
        &mut self,
        hash: &Byte32,
    ) -> Result<CoordinatorEntry<R, U, V>, CoordinatorError> {
        let active_source = self
            .entries
            .get(hash)
            .and_then(|entry| entry.uses_active_slot().then_some(entry.source));
        if let Some(source) = active_source {
            if self.active_work == 0
                || source
                    .peer()
                    .is_some_and(|peer| self.peer_active_work(peer) == 0)
            {
                return Err(CoordinatorError::ActiveWorkLimitExceeded);
            }
        }
        self.preflight_remove_conflict_indexes(hash)?;
        self.remove_current_queue_ticket(hash)?;
        self.remove_conflict_indexes(hash)?;
        self.apply_fault_checkpoint();
        if let Some(source) = active_source {
            self.deactivate_source(source)?;
        }
        let entry = self
            .entries
            .remove(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        let charge = CoordinatorResidency::new(1, entry.charge_bytes);
        self.global_usage = self
            .global_usage
            .checked_sub(charge)
            .ok_or(CoordinatorError::GlobalBudgetExceeded)?;
        self.by_short_id.remove(&entry.short_id);
        if let Some(peer) = entry.source.peer() {
            let remove_usage = {
                let usage = self
                    .peer_usage
                    .get_mut(&peer)
                    .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
                *usage = usage
                    .checked_sub(charge)
                    .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
                *usage == CoordinatorResidency::default()
            };
            if remove_usage {
                self.peer_usage.remove(&peer);
            }
            if let Some(hashes) = self.by_peer.get_mut(&peer) {
                hashes.remove(hash);
                if hashes.is_empty() {
                    self.by_peer.remove(&peer);
                }
            }
        }
        for parent in &entry.dependencies {
            if let Some(children) = self.by_parent.get_mut(parent) {
                children.remove(hash);
                if children.is_empty() {
                    self.by_parent.remove(parent);
                }
            }
        }
        self.live_deadlines.remove(hash);
        self.compact_deadlines();
        self.dependency_failure_set.remove(hash);
        self.compact_dependency_failures();
        self.apply_fault_checkpoint();
        Ok(entry)
    }

    fn compact_dependency_failures(&mut self) {
        const STALE_SLACK: usize = 64;
        if self.dependency_failures.len()
            > self
                .dependency_failure_set
                .len()
                .saturating_mul(2)
                .saturating_add(STALE_SLACK)
        {
            self.dependency_failures
                .retain(|hash| self.dependency_failure_set.contains(hash));
        }
    }

    fn compact_conflict_rechecks(&mut self) {
        const STALE_SLACK: usize = 64;
        if self.conflict_rechecks.len()
            > self
                .conflict_recheck_set
                .len()
                .saturating_mul(2)
                .saturating_add(STALE_SLACK)
        {
            self.conflict_rechecks
                .retain(|hash| self.conflict_recheck_set.contains(hash));
        }
    }

    fn preview_dependency_failure_roots(&self, max: usize) -> Vec<Byte32> {
        let mut pending: VecDeque<_> = self
            .dependency_failures
            .iter()
            .filter(|hash| self.dependency_failure_set.contains(*hash))
            .cloned()
            .collect();
        let mut scheduled: HashSet<_> = pending.iter().cloned().collect();
        let mut roots = Vec::with_capacity(max.min(self.dependency_failure_set.len()));
        while roots.len() < max {
            let Some(root) = pending.pop_front() else {
                break;
            };
            if !self.dependency_failure_set.contains(&root) {
                continue;
            }
            let mut children: Vec<_> = self
                .by_parent
                .get(&root)
                .into_iter()
                .flat_map(|children| children.iter().cloned())
                .filter(|child| {
                    self.entries
                        .get(child)
                        .is_some_and(|entry| entry.invalidated_cause().is_none())
                })
                .collect();
            children.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
            for child in children {
                if scheduled.insert(child.clone()) {
                    pending.push_back(child);
                }
            }
            roots.push(root);
        }
        roots
    }

    fn compact_deadlines(&mut self) {
        const STALE_SLACK: usize = 64;
        if self.deadlines.len()
            > self
                .live_deadlines
                .len()
                .saturating_mul(2)
                .saturating_add(STALE_SLACK)
        {
            self.deadlines.retain(|Reverse(ticket)| {
                self.live_deadlines
                    .get(&ticket.hash)
                    .is_some_and(|live| live == ticket)
            });
        }
    }

    fn terminal_record(
        hash: Byte32,
        entry: CoordinatorEntry<R, U, V>,
        disposition: TerminalDisposition,
    ) -> TerminalRecord<R, U, V> {
        let (raw, later_phase) = match entry.state {
            EntryState::Raw { raw, .. }
            | EntryState::Invalidated {
                raw,
                payload: InvalidatedPayload::Raw,
                ..
            } => (raw, None),
            EntryState::Unverified { raw, payload, .. }
            | EntryState::Invalidated {
                raw,
                payload: InvalidatedPayload::Unverified(payload),
                ..
            } => (raw, Some(TerminalPhase::Unverified(payload))),
            EntryState::PlainVerified { raw, payload, .. }
            | EntryState::CandidateVerified { raw, payload, .. }
            | EntryState::Invalidated {
                raw,
                payload: InvalidatedPayload::Verified(payload),
                ..
            } => (raw, Some(TerminalPhase::Verified(payload))),
        };
        TerminalRecord {
            hash,
            short_id: entry.short_id,
            raw,
            later_phase,
            peer: entry.source.peer(),
            source: entry.source,
            disposition,
        }
    }
}
