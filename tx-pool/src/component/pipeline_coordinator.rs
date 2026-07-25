//! Single authoritative owner for the production pre-pool pipeline.
//!
//! Lifecycle state, payload phase, worker leases, dependency/conflict edges,
//! queue tickets and residency accounting share one incarnation/revision and
//! one short-held transition lock.

use crate::constants::lazy_ticket_compaction_limit;
use ckb_network::PeerIndex;
use ckb_types::packed::{Byte32, OutPoint, ProposalShortId};
use ckb_types::prelude::Entity;
#[cfg(test)]
use std::cell::Cell;
use std::cmp::Ordering;
use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, HashMap, HashSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;

mod audit;
mod indexes;
mod types;
use self::indexes::*;
pub(crate) use types::*;
#[derive(Debug)]
pub(crate) struct PipelineCoordinator<R, U, V> {
    entries: HashMap<Byte32, CoordinatorEntry<R, U, V>>,
    by_short_id: HashMap<ProposalShortId, Byte32>,
    by_peer: HashMap<PeerIndex, HashSet<Byte32>>,
    by_parent: HashMap<Byte32, HashSet<Byte32>>,
    /// O(1) RPC/metrics projection of `RawLocation::WaitingParents`.
    /// Rebuilt with every undo recovery and independently checked by audit so
    /// an untrusted pool-info query never scans the whole coordinator while
    /// holding its mutation mutex.
    waiting_parent_count: usize,
    dependency_failures: VecDeque<Byte32>,
    dependency_failure_set: HashSet<Byte32>,
    conflicts: StagedConflictIndex,
    queues: HashMap<QueueKind, TicketQueue>,
    deadlines: BinaryHeap<Reverse<DeadlineTicket>>,
    live_deadlines: HashMap<Byte32, DeadlineTicket>,
    capacity_victim_index: BTreeSet<CapacityVictimKey>,
    candidate_victim_index: BTreeSet<CandidateVictimKey>,
    /// Nested undo scopes mutate one authoritative entry cohort. Derived
    /// victim indexes are published only by the outermost successful scope,
    /// preventing intermediate keys from becoming a second state history.
    entry_transaction_depth: usize,
    /// Number of active undo scopes that snapshot each hash. A write is legal
    /// only when the count equals `entry_transaction_depth`, so nested scopes
    /// can mutate only hashes snapshotted by themselves and every outer scope.
    /// This turns cohort completeness into an O(1) checked invariant.
    entry_transaction_membership: HashMap<Byte32, usize>,
    #[cfg(test)]
    capacity_victim_probes: Cell<usize>,
    #[cfg(test)]
    candidate_victim_probes: Cell<usize>,
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
    /// One-shot recoverable error after a commit handoff has exercised its
    /// complete apply path. The enclosing undo scope must restore it before
    /// the production PoolMap cutover rolls back. Test-only: production has
    /// no alternate handoff state or failure channel.
    #[cfg(test)]
    fail_next_handoff_after_apply: Option<CoordinatorError>,
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
            waiting_parent_count: 0,
            dependency_failures: VecDeque::new(),
            dependency_failure_set: HashSet::new(),
            conflicts: StagedConflictIndex::default(),
            queues: HashMap::from([
                (QueueKind::PreCheck, TicketQueue::new(QueueOrdering::Fifo)),
                (QueueKind::Resolve, TicketQueue::new(QueueOrdering::Fifo)),
                (QueueKind::Verify, TicketQueue::new(verify_ordering)),
                (
                    QueueKind::Commit,
                    TicketQueue::new(QueueOrdering::Candidate),
                ),
            ]),
            deadlines: BinaryHeap::new(),
            live_deadlines: HashMap::new(),
            capacity_victim_index: BTreeSet::new(),
            candidate_victim_index: BTreeSet::new(),
            entry_transaction_depth: 0,
            entry_transaction_membership: HashMap::new(),
            #[cfg(test)]
            capacity_victim_probes: Cell::new(0),
            #[cfg(test)]
            candidate_victim_probes: Cell::new(0),
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
            #[cfg(test)]
            fail_next_handoff_after_apply: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn usage(&self) -> CoordinatorResidency {
        self.global_usage
    }

    pub(crate) fn peer_usage(&self, peer: PeerIndex) -> CoordinatorResidency {
        self.peer_usage.get(&peer).copied().unwrap_or_default()
    }

    pub(crate) fn peer_hashes(&self, peer: PeerIndex, max: usize) -> Vec<Byte32> {
        self.by_peer
            .get(&peer)
            .into_iter()
            .flat_map(|hashes| hashes.iter())
            .filter(|hash| {
                self.entries.get(*hash).is_some_and(|entry| {
                    !matches!(entry.view().location, CoordinatorLocation::Committing)
                })
            })
            // Peer revocation has no ordering contract. Taking the bounded
            // slice directly avoids collecting and sorting the peer's entire
            // attacker-controlled residency on every maintenance turn.
            .take(max)
            .cloned()
            .collect()
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

    pub(crate) fn unverified_by_hash(&self, hash: &Byte32) -> Option<&U> {
        match &self.entries.get(hash)?.state {
            EntryState::Unverified { payload, .. } => Some(payload.as_ref()),
            _ => None,
        }
    }

    pub(crate) fn verified_by_hash(&self, hash: &Byte32) -> Option<&V> {
        match &self.entries.get(hash)?.state {
            EntryState::CandidateVerified { payload, .. } => Some(payload.as_ref()),
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
        self.waiting_parent_count
    }

    fn enter_waiting_parent(&mut self) -> Result<(), CoordinatorError> {
        self.waiting_parent_count = self
            .waiting_parent_count
            .checked_add(1)
            .ok_or(CoordinatorError::ConflictInvariant)?;
        Ok(())
    }

    fn leave_waiting_parent(&mut self) -> Result<(), CoordinatorError> {
        self.waiting_parent_count = self
            .waiting_parent_count
            .checked_sub(1)
            .ok_or(CoordinatorError::ConflictInvariant)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn active_conflict_owner(&self, input: &OutPoint) -> Option<&Byte32> {
        self.conflicts.by_input.get(input).and_then(|candidates| {
            candidates
                .iter()
                .filter_map(|hash| self.candidate_rank(hash).ok().map(|rank| (hash, rank)))
                .max_by(|(_, left), (_, right)| left.cmp(right))
                .map(|(hash, _)| hash)
        })
    }

    #[cfg(test)]
    pub(crate) fn conflict_edge_count(&self) -> usize {
        self.conflicts.input_memberships
    }

    #[cfg(test)]
    pub(crate) fn deadline_len(&self) -> usize {
        self.live_deadlines.len()
    }

    #[cfg(test)]
    pub(crate) fn active_work(&self) -> usize {
        self.active_work
    }

    pub(crate) fn peer_active_work(&self, peer: PeerIndex) -> usize {
        self.active_work_by_peer.get(&peer).copied().unwrap_or(0)
    }

    #[cfg(test)]
    pub(crate) fn admit_raw(
        &mut self,
        hash: Byte32,
        short_id: ProposalShortId,
        raw: R,
        initial_stage: RawStage,
        peer: Option<PeerIndex>,
        charge_bytes: usize,
        dependencies: HashSet<Byte32>,
    ) -> Result<(CoordinatorVersion, Vec<TerminalRecord<R>>), CoordinatorError> {
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
    ) -> Result<(CoordinatorVersion, Vec<TerminalRecord<R>>), CoordinatorError> {
        // The coordinator is the persistent ownership boundary. Enforce
        // compact identity and parent keys here rather than relying on every
        // adapter to remember that molecule accessors may share a whole raw
        // transaction, block, or relay envelope.
        let hash = crate::util::compact_packed(&hash);
        let short_id = crate::util::compact_packed(&short_id);
        let dependencies = dependencies
            .into_iter()
            .map(|parent| crate::util::compact_packed(&parent))
            .collect::<HashSet<_>>();
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
        self.with_capacity_victims(subject, victims, Vec::new(), move |coordinator| {
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
            if self.by_parent.get(parent).map_or(0, HashSet::len)
                >= self.limits.max_dependents_per_parent
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
            .reserve_live(source.queue_owner(), false)?;
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
        self.insert_absent_entry(hash, entry)?;
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
        let (
            current,
            old_charge,
            new_charge,
            new_raw_charge_bytes,
            new_base_metadata_bytes,
            new_metadata_bytes,
            old_ticket,
            queue_kind,
            version,
            active,
            had_expiry,
            waiting_parent,
            candidate_state,
        ) = {
            let entry = self
                .entries
                .get(hash)
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            let old_charge = CoordinatorResidency::new(1, entry.charge_bytes);
            let (new_base_metadata_bytes, new_metadata_bytes) = if entry.expires_at.is_some() {
                (
                    self.metadata_charge_bytes(entry.dependencies.len(), false, 0)?,
                    self.metadata_charge_bytes(
                        entry.dependencies.len(),
                        false,
                        entry
                            .candidate()
                            .map_or(0, |candidate| candidate.inputs.len()),
                    )?,
                )
            } else {
                (entry.base_metadata_bytes, entry.metadata_bytes)
            };
            let new_charge_bytes = entry
                .resident_payload_bytes
                .checked_add(new_metadata_bytes)
                .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
            let new_raw_charge_bytes = entry
                .raw_resident_payload_bytes
                .checked_add(new_base_metadata_bytes)
                .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
            (
                entry.source,
                old_charge,
                CoordinatorResidency::new(1, new_charge_bytes),
                new_raw_charge_bytes,
                new_base_metadata_bytes,
                new_metadata_bytes,
                entry.ticket(hash),
                entry.queue_kind(),
                entry.version(),
                entry.uses_active_slot(),
                entry.expires_at.is_some(),
                matches!(entry.location(), CoordinatorLocation::WaitingParents { .. }),
                match &entry.state {
                    EntryState::CandidateVerified {
                        candidate,
                        location,
                        ..
                    } => Some((candidate.clone(), location.clone())),
                    _ => None,
                },
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
            && (queue_kind.is_some() || waiting_parent);
        if current == target && !repeated_proposal && !waiting_parent {
            return Ok(version);
        }
        // Trusted owners have no expiry. A promoted remote orphan therefore
        // cannot remain in WaitingParents: requeue it through Resolve so the
        // unified trusted-parent policy either observes an in-flight parent
        // or terminalizes an unavailable external dependency.
        let target_queue_kind = queue_kind.or(waiting_parent.then_some(QueueKind::Resolve));
        let reticket = target_queue_kind.is_some();
        let queue_sequence = if reticket {
            Some(self.queue_sequence_range(1)?)
        } else {
            None
        };
        if reticket {
            self.ensure_revision_capacity(hash)?;
        }

        // Source trust participates in CandidateRank. Recompute only the
        // bounded direct cohort and reconcile its derived tickets in the same
        // ownership transaction as the attribution change.
        let (conflict_delta, conflict_force) = if let Some((candidate, location)) = &candidate_state
        {
            let next_rank = CandidateRank::from_entry(hash, target, candidate, location);
            let delta = self.preview_conflict_rerank(hash, &next_rank, location.clone())?;
            let force = if *location == CandidateLocation::Verified {
                HashSet::from([hash.clone()])
            } else {
                HashSet::new()
            };
            (Some(delta), force)
        } else {
            (None, HashSet::new())
        };
        let source_overrides = HashMap::from([(hash.clone(), target)]);
        if let Some(peer) = current.peer() {
            let usage = self
                .peer_usage
                .get(&peer)
                .copied()
                .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
            usage
                .checked_sub(old_charge)
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

        let mut undo = vec![hash.clone()];
        if let Some(delta) = &conflict_delta {
            undo.extend(delta.affected().iter().cloned());
        }
        undo.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        undo.dedup();
        self.with_entry_undo(&undo, |coordinator| {
            if reticket {
                coordinator
                    .queue_mut(target_queue_kind.ok_or(CoordinatorError::SourceDowngrade)?)?
                    .reserve_live(
                        target.queue_owner(),
                        old_ticket.verify_schedule.is_large_cycle,
                    )?;
            }
            let mut conflict_ticket_plan = match &conflict_delta {
                Some(delta) => Some(coordinator.prepare_conflict_ticket_plan(
                    delta,
                    &conflict_force,
                    &source_overrides,
                )?),
                None => None,
            };
            if let Some(plan) = &conflict_ticket_plan {
                coordinator.remove_conflict_tickets(plan)?;
            }
            if let Some(peer) = current.peer() {
                let remove_usage = {
                    let usage = coordinator
                        .peer_usage
                        .get_mut(&peer)
                        .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
                    *usage = usage
                        .checked_sub(old_charge)
                        .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
                    *usage == CoordinatorResidency::default()
                };
                if remove_usage {
                    coordinator.peer_usage.remove(&peer);
                }
                let remove_bucket = if let Some(hashes) = coordinator.by_peer.get_mut(&peer) {
                    hashes.remove(hash);
                    hashes.is_empty()
                } else {
                    false
                };
                if remove_bucket {
                    coordinator.by_peer.remove(&peer);
                }
                if active {
                    let remove_active = {
                        let active = coordinator
                            .active_work_by_peer
                            .get_mut(&peer)
                            .ok_or(CoordinatorError::PeerActiveWorkLimitExceeded(peer))?;
                        *active = active
                            .checked_sub(1)
                            .ok_or(CoordinatorError::PeerActiveWorkLimitExceeded(peer))?;
                        *active == 0
                    };
                    if remove_active {
                        coordinator.active_work_by_peer.remove(&peer);
                    }
                }
                coordinator.apply_fault_checkpoint();
            }

            if old_charge != new_charge {
                coordinator.global_usage = coordinator
                    .global_usage
                    .checked_sub(old_charge)
                    .and_then(|usage| usage.checked_add(new_charge))
                    .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
                coordinator.apply_fault_checkpoint();
            }
            if had_expiry {
                coordinator.live_deadlines.remove(hash);
                coordinator.apply_fault_checkpoint();
            }

            let new_ticket = {
                let entry = coordinator.entry_mut(hash)?;
                entry.source = target;
                entry.expires_at = None;
                entry.base_metadata_bytes = new_base_metadata_bytes;
                entry.metadata_bytes = new_metadata_bytes;
                entry.raw_charge_bytes = new_raw_charge_bytes;
                entry.charge_bytes = new_charge.bytes;
                if target_queue_kind.filter(|_| reticket).is_some() {
                    let (sequence, next_sequence) =
                        queue_sequence.ok_or(CoordinatorError::QueueSequenceExhausted)?;
                    if waiting_parent {
                        let EntryState::Raw { location, .. } = &mut entry.state else {
                            return Err(CoordinatorError::ConflictInvariant);
                        };
                        *location = RawLocation::Queued(RawStage::Resolve);
                    }
                    entry.queue_sequence = sequence;
                    entry.revision += 1;
                    Some((entry.ticket(hash), next_sequence))
                } else {
                    None
                }
            };
            if waiting_parent {
                coordinator.leave_waiting_parent()?;
            }
            if let Some((new_ticket, next_sequence)) = new_ticket {
                coordinator.next_queue_sequence = next_sequence;
                if let Some(old_kind) = queue_kind {
                    coordinator
                        .queue_mut(old_kind)?
                        .remove_live(old_kind, &old_ticket)?;
                }
                let kind = target_queue_kind.ok_or(CoordinatorError::SourceDowngrade)?;
                coordinator.queue_mut(kind)?.push_reserved(
                    kind,
                    new_ticket,
                    target.is_proposal(),
                )?;
            }
            if let Some(delta) = &conflict_delta {
                coordinator.apply_conflict_delta(delta)?;
            }
            if let Some(plan) = conflict_ticket_plan.take() {
                coordinator.apply_conflict_ticket_plan(plan)?;
            }
            coordinator.apply_fault_checkpoint();
            coordinator.compact_deadlines();
            coordinator
                .entries
                .get(hash)
                .map(CoordinatorEntry::version)
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))
        })
    }

    /// Install a trusted witness-bearing raw payload for an existing raw-hash
    /// owner and restart its lifecycle from the requested raw stage. A raw
    /// hash deliberately excludes witnesses, so source promotion alone is not
    /// sufficient when a Proposal/Local payload differs from the one first
    /// received from the network.
    ///
    /// The replacement keeps the existing incarnation and dependency graph,
    /// but advances the revision, removes every later-phase/conflict claim and
    /// invalidates any outstanding worker lease. Capacity reconciliation uses
    /// the trusted target strength, allowing a larger authoritative witness to
    /// displace weaker work instead of being pinned by a small remote variant.
    pub(crate) fn replace_raw_payload(
        &mut self,
        hash: &Byte32,
        raw: R,
        raw_payload_bytes: usize,
        promotion: TrustedSource,
        stage: RawStage,
    ) -> Result<(CoordinatorVersion, Vec<TerminalRecord<R>>), CoordinatorError> {
        let target = match promotion {
            TrustedSource::Local => CoordinatorSource::Local,
            TrustedSource::Proposal => CoordinatorSource::Proposal,
        };
        let entry = self
            .entries
            .get(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        if entry.is_committing() {
            return Err(CoordinatorError::CommitInProgress(hash.clone()));
        }
        if entry.source == CoordinatorSource::Proposal && target == CoordinatorSource::Local {
            return Err(CoordinatorError::SourceDowngrade);
        }
        let base_metadata_bytes = self.metadata_charge_bytes(entry.dependencies.len(), false, 0)?;
        let total_charge_bytes = raw_payload_bytes
            .checked_add(base_metadata_bytes)
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        let protected = self.dependency_ancestor_closure(hash, &entry.dependencies)?;
        let victims = self.global_capacity_victims(
            Some(hash),
            target,
            total_charge_bytes,
            &HashSet::new(),
            &protected,
        )?;
        let subject = CapacitySubject::Present(hash.clone());
        let subject_undo = self.causal_undo_hashes(std::slice::from_ref(hash));
        self.with_capacity_victims(subject, victims, subject_undo, move |coordinator| {
            coordinator.replace_raw_payload_inner(hash, raw, raw_payload_bytes, promotion, stage)
        })
    }

    fn replace_raw_payload_inner(
        &mut self,
        hash: &Byte32,
        raw: R,
        raw_payload_bytes: usize,
        promotion: TrustedSource,
        stage: RawStage,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        let target = match promotion {
            TrustedSource::Local => CoordinatorSource::Local,
            TrustedSource::Proposal => CoordinatorSource::Proposal,
        };
        let undo = self.causal_undo_hashes(std::slice::from_ref(hash));
        self.with_entry_undo(&undo, move |coordinator| {
            let current = coordinator
                .entries
                .get(hash)
                .map(|entry| entry.source)
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            if current == CoordinatorSource::Proposal && target == CoordinatorSource::Local {
                return Err(CoordinatorError::SourceDowngrade);
            }
            if coordinator
                .entries
                .get(hash)
                .is_some_and(CoordinatorEntry::is_committing)
            {
                return Err(CoordinatorError::CommitInProgress(hash.clone()));
            }
            if current != target {
                coordinator.promote_source(hash, promotion)?;
            }

            coordinator.ensure_revision_capacity(hash)?;
            let (dependencies, active) = coordinator
                .entries
                .get(hash)
                .map(|entry| (entry.dependencies.len(), entry.uses_active_slot()))
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            let base_metadata_bytes = coordinator.metadata_charge_bytes(dependencies, false, 0)?;
            let total_charge_bytes = raw_payload_bytes
                .checked_add(base_metadata_bytes)
                .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
            coordinator.check_recharge(hash, total_charge_bytes)?;
            coordinator.preflight_remove_conflict_indexes(hash)?;

            let queue_kind = match stage {
                RawStage::PreCheck => QueueKind::PreCheck,
                RawStage::Resolve => QueueKind::Resolve,
            };
            coordinator
                .queue_mut(queue_kind)?
                .reserve_live(target.queue_owner(), false)?;
            let (queue_sequence, next_queue_sequence) = coordinator.queue_sequence_range(1)?;

            coordinator.remove_current_scheduling(hash)?;
            if active {
                coordinator.deactivate_source(target)?;
            }
            coordinator.live_deadlines.remove(hash);
            coordinator.dependency_failure_set.remove(hash);
            coordinator.compact_deadlines();
            coordinator.compact_dependency_failures();
            coordinator.apply_recharge(hash, total_charge_bytes)?;

            let was_waiting = coordinator.entries.get(hash).is_some_and(|entry| {
                matches!(
                    &entry.state,
                    EntryState::Raw {
                        location: RawLocation::WaitingParents { .. },
                        ..
                    }
                )
            });
            if was_waiting {
                coordinator.leave_waiting_parent()?;
            }

            let entry = coordinator.entry_mut(hash)?;
            entry.source = target;
            entry.expires_at = None;
            entry.raw_charge_bytes = total_charge_bytes;
            entry.raw_resident_payload_bytes = raw_payload_bytes;
            entry.resident_payload_bytes = raw_payload_bytes;
            entry.base_metadata_bytes = base_metadata_bytes;
            entry.metadata_bytes = base_metadata_bytes;
            entry.queue_sequence = queue_sequence;
            entry.state = EntryState::Raw {
                raw: Arc::new(raw),
                location: RawLocation::Queued(stage),
            };
            entry.revision += 1;
            let version = entry.version();
            let ticket = entry.ticket(hash);
            coordinator.queue_mut(queue_kind)?.push_reserved(
                queue_kind,
                ticket,
                target.is_proposal(),
            )?;
            coordinator.next_queue_sequence = next_queue_sequence;
            coordinator.apply_fault_checkpoint();
            Ok(version)
        })
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
        self.validate_version_location(&ticket.hash, ticket.version, &expected)?;
        self.ensure_revision_capacity(&ticket.hash)?;
        let source = self
            .entries
            .get(&ticket.hash)
            .map(|entry| entry.source)
            .ok_or_else(|| CoordinatorError::Missing(ticket.hash.clone()))?;
        self.check_activate_source(source)?;
        self.consume_front_ticket(kind, &ticket)?;
        self.activate_source(source)?;
        let entry = self.entry_mut(&ticket.hash)?;
        let EntryState::Raw { raw, location } = &mut entry.state else {
            return Err(CoordinatorError::ConflictInvariant);
        };
        *location = RawLocation::Active(stage);
        let payload = Arc::clone(raw);
        entry.revision += 1;
        Ok(Some(RawWorkLease {
            hash: ticket.hash,
            stage,
            version: entry.version(),
            payload,
        }))
    }

    /// Terminalize exactly the active raw owner represented by `lease`.
    /// Administrative hash-only removal remains separate because it has
    /// different stale-worker semantics.
    pub(crate) fn terminalize_raw(
        &mut self,
        lease: &RawWorkLease<R>,
        disposition: TerminalDisposition,
    ) -> Result<TerminalRecord<R>, CoordinatorError> {
        let expected = CoordinatorLocation::RawActive(lease.stage);
        self.validate_version_location(&lease.hash, lease.version, &expected)?;
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
    ) -> Result<TerminalRecord<R>, CoordinatorError> {
        self.validate_version_location(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::VerifyActive,
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
        self.validate_version_location(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::VerifyActive,
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
            if requeue {
                coordinator
                    .queue_mut(QueueKind::Resolve)?
                    .reserve_live(source.queue_owner(), false)?;
            }
            coordinator.deactivate_source(source)?;
            coordinator.apply_recharge(&lease.hash, raw_charge)?;
            if !requeue {
                coordinator.enter_waiting_parent()?;
            }
            let version = {
                let entry = coordinator.entry_mut(&lease.hash)?;
                let raw = Arc::clone(entry.state.raw());
                let location = if requeue {
                    RawLocation::Queued(RawStage::Resolve)
                } else {
                    RawLocation::WaitingParents { missing }
                };
                entry.state = EntryState::Raw { raw, location };
                entry.resident_payload_bytes = entry.raw_resident_payload_bytes;
                entry.metadata_bytes = entry.base_metadata_bytes;
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
    ) -> Result<(CoordinatorVersion, Vec<TerminalRecord<R>>), CoordinatorError> {
        let expected = CoordinatorLocation::RawActive(lease.stage);
        self.validate_version_location(&lease.hash, lease.version, &expected)?;
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
        self.with_capacity_victims(subject, victims, Vec::new(), move |coordinator| {
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
        self.validate_version_location(&lease.hash, lease.version, &expected)?;
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
        self.queue_mut(QueueKind::Verify)?
            .reserve_live(source.queue_owner(), verify_schedule.is_large_cycle)?;
        let (queue_sequence, next_queue_sequence) = self.queue_sequence_range(1)?;
        self.deactivate_source(source)?;
        self.apply_recharge(&lease.hash, total_charge_bytes)?;
        let entry = self.entry_mut(&lease.hash)?;
        let raw = Arc::clone(entry.state.raw());
        entry.state = EntryState::Unverified {
            raw,
            payload: Arc::new(unverified),
            location: UnverifiedLocation::Queued,
            verify_schedule,
        };
        entry.resident_payload_bytes = charge_bytes;
        entry.metadata_bytes = metadata_bytes;
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

    pub(crate) fn wait_for_parents(
        &mut self,
        lease: &RawWorkLease<R>,
        missing: HashSet<Byte32>,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        let expected = CoordinatorLocation::RawActive(lease.stage);
        self.validate_version_location(&lease.hash, lease.version, &expected)?;
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
        let next_waiting_parent_count = self
            .waiting_parent_count
            .checked_add(1)
            .ok_or(CoordinatorError::ConflictInvariant)?;
        let source = self
            .entries
            .get(&lease.hash)
            .map(|entry| entry.source)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        self.deactivate_source(source)?;
        let entry = self.entry_mut(&lease.hash)?;
        let EntryState::Raw { location, .. } = &mut entry.state else {
            return Err(CoordinatorError::ConflictInvariant);
        };
        *location = RawLocation::WaitingParents { missing };
        entry.revision += 1;
        let version = entry.version();
        self.waiting_parent_count = next_waiting_parent_count;
        Ok(version)
    }

    pub(crate) fn requeue_raw(
        &mut self,
        lease: &RawWorkLease<R>,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        let expected = CoordinatorLocation::RawActive(lease.stage);
        self.validate_version_location(&lease.hash, lease.version, &expected)?;
        let old_victim_keys = self.current_victim_keys(&lease.hash);
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
        let (queue_sequence, next_queue_sequence) = self.queue_sequence_range(1)?;
        let version = self.with_entry_undo(std::slice::from_ref(&lease.hash), |coordinator| {
            coordinator
                .queue_mut(kind)?
                .reserve_live(source.queue_owner(), false)?;
            coordinator.deactivate_source(source)?;
            let entry = coordinator.entry_mut(&lease.hash)?;
            let EntryState::Raw { location, .. } = &mut entry.state else {
                return Err(CoordinatorError::ConflictInvariant);
            };
            *location = RawLocation::Queued(lease.stage);
            entry.queue_sequence = queue_sequence;
            entry.revision += 1;
            let version = entry.version();
            let ticket = entry.ticket(&lease.hash);
            let front = entry.source.is_proposal();
            coordinator
                .queue_mut(kind)?
                .push_reserved(kind, ticket, front)?;
            coordinator.next_queue_sequence = next_queue_sequence;
            Ok(version)
        })?;
        self.refresh_victim_indexes(&lease.hash, old_victim_keys);
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
        let mut ready_owners = Vec::new();
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
                ready_count = ready_count
                    .checked_add(1)
                    .ok_or(CoordinatorError::QueueReservationFailed)?;
                ready_owners.push(entry.source.queue_owner());
            }
            affected.push(child);
        }
        let mut ready = Vec::new();
        ready
            .try_reserve(ready_count)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        let (first_queue_sequence, next_queue_sequence) = self.queue_sequence_range(ready_count)?;

        let undo = affected.clone();
        let mut queue_sequence = first_queue_sequence;
        self.with_entry_undo(&undo, |coordinator| {
            coordinator
                .queue_mut(QueueKind::Resolve)?
                .reserve_many(ready_owners, false)?;
            coordinator.next_queue_sequence = next_queue_sequence;
            for child in affected {
                let entry = coordinator.entry_mut(&child)?;
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
                    queue_sequence = queue_sequence
                        .checked_add(1)
                        .ok_or(CoordinatorError::QueueSequenceExhausted)?;
                    let ticket = entry.ticket(&child);
                    let front = entry.source.is_proposal();
                    coordinator.leave_waiting_parent()?;
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
        let roots: Vec<_> = affected.into_iter().collect();
        self.conflict_undo_hashes(&roots)
    }

    #[cfg(test)]
    pub(crate) fn parent_unavailable(
        &mut self,
        parent: &Byte32,
    ) -> Result<Vec<Byte32>, CoordinatorError> {
        self.parents_unavailable(&HashSet::from([parent.clone()]))
    }

    /// Atomically reclassify every coordinator transaction whose dependency is
    /// in `parents`. Expiring Remote owners may wait for retransmission;
    /// non-expiring Local/Proposal owners become causal terminal work instead
    /// of parking forever. Administrative pool removal uses this before
    /// deleting a root and its accepted descendants, so no already-resolved
    /// consumer can outlive any member of the removed closure. Every child is
    /// transitioned once even when several parents disappear together.
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
        let trusted: Vec<_> = affected
            .iter()
            .filter(|child| {
                self.entries
                    .get(*child)
                    .is_some_and(|entry| !matches!(entry.source, CoordinatorSource::Remote(_)))
            })
            .cloned()
            .collect();
        for child in &affected {
            self.ensure_revision_capacity(child)?;
            self.preflight_remove_conflict_indexes(child)?;
        }
        // A Remote owner may wait for retransmission under its original
        // expiry. Local/Proposal owners deliberately have no expiry, so
        // parking them here would create permanent high-priority residency.
        // Pre-reserve their bounded terminal-maintenance tickets instead.
        self.dependency_failures
            .try_reserve(trusted.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.dependency_failure_set
            .try_reserve(trusted.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        let (first_sequence, next_maintenance_sequence) =
            self.maintenance_sequence_range(trusted.len())?;
        let trusted_sequences: HashMap<_, _> = trusted
            .iter()
            .enumerate()
            .map(|(offset, child)| {
                let offset = u64::try_from(offset)
                    .map_err(|_| CoordinatorError::MaintenanceSequenceExhausted)?;
                let sequence = first_sequence
                    .checked_add(offset)
                    .ok_or(CoordinatorError::MaintenanceSequenceExhausted)?;
                Ok((child.clone(), sequence))
            })
            .collect::<Result<_, CoordinatorError>>()?;

        let mut undo = self.conflict_undo_hashes(&affected);
        undo.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        undo.dedup();
        let result = affected.clone();
        self.with_entry_undo(&undo, |coordinator| {
            coordinator.next_maintenance_sequence = next_maintenance_sequence;
            for child in &affected {
                if let Some(sequence) = trusted_sequences.get(child) {
                    let cause = missing_by_child
                        .get(child)
                        .and_then(|missing| {
                            missing
                                .iter()
                                .min_by(|left, right| left.as_slice().cmp(right.as_slice()))
                        })
                        .cloned()
                        .ok_or(CoordinatorError::ConflictInvariant)?;
                    coordinator.invalidate_present_apply(child, &cause, *sequence)?;
                    continue;
                }

                let active_source = coordinator
                    .entries
                    .get(child)
                    .and_then(|entry| entry.uses_active_slot().then_some(entry.source));
                if let Some(source) = active_source {
                    coordinator.deactivate_source(source)?;
                }
                coordinator.remove_current_scheduling(child)?;
                coordinator.apply_fault_checkpoint();
                let was_waiting = coordinator.entries.get(child).is_some_and(|entry| {
                    matches!(
                        &entry.state,
                        EntryState::Raw {
                            location: RawLocation::WaitingParents { .. },
                            ..
                        }
                    )
                });
                let raw_charge = coordinator
                    .entries
                    .get(child)
                    .ok_or_else(|| CoordinatorError::Missing(child.clone()))?
                    .raw_charge_bytes;
                coordinator.apply_recharge(child, raw_charge)?;
                let entry = coordinator.entry_mut(child)?;
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
                let raw = Arc::clone(entry.state.raw());
                entry.state = EntryState::Raw {
                    raw,
                    location: RawLocation::WaitingParents { missing },
                };
                entry.revision += 1;
                if !was_waiting {
                    coordinator.enter_waiting_parent()?;
                }
                coordinator.apply_fault_checkpoint();
            }
            Ok(result)
        })
    }

    /// Make every direct dependent fail-closed immediately, then defer the
    /// transitive terminal cascade to bounded maintenance slices.
    #[cfg(test)]
    pub(crate) fn schedule_parent_failure(
        &mut self,
        parent: &Byte32,
    ) -> Result<Vec<Byte32>, CoordinatorError> {
        self.mark_children_invalid(parent, parent)
    }

    pub(crate) fn drain_dependency_failures(
        &mut self,
        max: usize,
    ) -> Result<Vec<TerminalRecord<R>>, CoordinatorError> {
        let roots = self.preview_dependency_failure_roots(max);
        let mut affected: HashSet<_> = roots.iter().cloned().collect();
        for root in &roots {
            if let Some(children) = self.by_parent.get(root) {
                affected.extend(children.iter().cloned());
            }
        }
        let roots_and_children: Vec<_> = affected.into_iter().collect();
        let mut affected = self.causal_undo_hashes(&roots_and_children);
        affected.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
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
        self.validate_version_location(
            &ticket.hash,
            ticket.version,
            &CoordinatorLocation::VerifyQueued,
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
        let entry = self.entry_mut(&ticket.hash)?;
        let payload = match &mut entry.state {
            EntryState::Unverified {
                payload, location, ..
            } => {
                *location = UnverifiedLocation::Active;
                Arc::clone(payload)
            }
            _ => return Err(CoordinatorError::ConflictInvariant),
        };
        entry.revision += 1;
        Ok(Some(VerifyWorkLease {
            hash: ticket.hash,
            version: entry.version(),
            payload,
        }))
    }

    /// Test convenience wrapper around the only production verified state.
    /// A unique synthetic input keeps generic state-machine tests on the same
    /// candidate/index path as real transactions without creating conflicts.
    #[cfg(test)]
    pub(crate) fn complete_verification(
        &mut self,
        lease: &VerifyWorkLease<U>,
        verified: V,
        charge_bytes: usize,
    ) -> Result<(CoordinatorVersion, Vec<TerminalRecord<R>>), CoordinatorError> {
        let candidate = VerifiedCandidate {
            inputs: HashSet::from([OutPoint::new(lease.hash.clone(), 0)]),
            fee: 0,
            tx_size: 1,
        };
        self.complete_verification_candidate(lease, verified, charge_bytes, candidate)
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
    ) -> Result<(CoordinatorVersion, Vec<TerminalRecord<R>>), CoordinatorError> {
        // Candidate inputs become keys in shared conflict indexes and can
        // outlive whichever transaction first introduced an outpoint.
        let candidate = VerifiedCandidate {
            inputs: candidate
                .inputs
                .into_iter()
                .map(|input| crate::util::compact_packed(&input))
                .collect(),
            fee: candidate.fee,
            tx_size: candidate.tx_size,
        };
        self.validate_version_location(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::VerifyActive,
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
        let subject_undo: Vec<_> = self
            .conflicting_candidates_for_undo(&lease.hash, &incoming.inputs)?
            .into_iter()
            .collect();
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
        self.with_capacity_victims(subject, victims, subject_undo, move |coordinator| {
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
        self.validate_version_location(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::VerifyActive,
        )?;
        self.ensure_revision_capacity(&lease.hash)?;
        if candidate.inputs.len() > self.limits.max_conflict_inputs_per_entry {
            return Err(CoordinatorError::ConflictInputLimitExceeded);
        }
        let (dependencies, has_deadline, source) = self
            .entries
            .get(&lease.hash)
            .map(|entry| {
                (
                    entry.dependencies.len(),
                    entry.expires_at.is_some(),
                    entry.source,
                )
            })
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
        let delta = self.preview_conflict_insert(&lease.hash, source, &meta)?;
        let undo = delta.affected().to_vec();
        self.with_entry_undo(&undo, |coordinator| {
            let ticket_plan = coordinator.prepare_conflict_ticket_plan(
                &delta,
                &HashSet::new(),
                &HashMap::new(),
            )?;
            coordinator.remove_conflict_tickets(&ticket_plan)?;
            let source = coordinator
                .entries
                .get(&lease.hash)
                .map(|entry| entry.source)
                .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
            coordinator.deactivate_source(source)?;
            coordinator.apply_recharge(&lease.hash, total_charge_bytes)?;
            coordinator.next_arrival = next_arrival;
            coordinator.apply_fault_checkpoint();
            let entry = coordinator.entry_mut(&lease.hash)?;
            let raw = Arc::clone(entry.state.raw());
            entry.state = EntryState::CandidateVerified {
                raw,
                payload: Arc::new(verified),
                candidate: meta,
                location: CandidateLocation::Verified,
            };
            entry.resident_payload_bytes = charge_bytes;
            entry.metadata_bytes = metadata_bytes;
            coordinator.apply_conflict_delta(&delta)?;
            if !ticket_plan.revises(&lease.hash) {
                coordinator.entry_mut(&lease.hash)?.revision += 1;
            }
            coordinator.apply_conflict_ticket_plan(ticket_plan)?;
            coordinator.apply_fault_checkpoint();
            coordinator
                .entries
                .get(&lease.hash)
                .map(CoordinatorEntry::version)
                .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))
        })
    }

    pub(crate) fn begin_next_commit(&mut self) -> Result<Option<CommitLease<V>>, CoordinatorError> {
        if self.conflicts.committing.is_some() {
            return Ok(None);
        }
        let Some(ticket) = self.peek_live_ticket(QueueKind::Commit, WorkerCapability::Any)? else {
            return Ok(None);
        };
        self.validate_version_location(
            &ticket.hash,
            ticket.version,
            &CoordinatorLocation::Verified,
        )?;
        if !self.candidate_is_eligible(&ticket.hash) {
            return Err(CoordinatorError::ConflictInvariant);
        }
        let old_victim_keys = self.current_victim_keys(&ticket.hash);
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
        let (candidate, location) = self
            .entries
            .get(&ticket.hash)
            .and_then(|entry| match &entry.state {
                EntryState::CandidateVerified {
                    candidate,
                    location,
                    ..
                } => Some((candidate.clone(), location.clone())),
                _ => None,
            })
            .ok_or(CoordinatorError::ConflictInvariant)?;
        let next_rank = CandidateRank::from_entry(
            &ticket.hash,
            source,
            &candidate,
            &CandidateLocation::Committing,
        );
        debug_assert_eq!(location, CandidateLocation::Verified);
        let delta =
            self.preview_conflict_rerank(&ticket.hash, &next_rank, CandidateLocation::Committing)?;
        let undo = delta.affected().to_vec();
        let hash = ticket.hash;
        let lease = self.with_entry_undo(&undo, |coordinator| {
            let ticket_plan = coordinator.prepare_conflict_ticket_plan(
                &delta,
                &HashSet::new(),
                &HashMap::new(),
            )?;
            coordinator.remove_conflict_tickets(&ticket_plan)?;
            coordinator.activate_source(source)?;
            // A committing entry is temporarily outside expiry scheduling.
            coordinator.live_deadlines.remove(&hash);
            let entry = coordinator.entry_mut(&hash)?;
            if entry.expires_at.is_some() {
                entry.deadline_generation += 1;
            }
            let payload = match &mut entry.state {
                EntryState::CandidateVerified {
                    payload, location, ..
                } => {
                    *location = CandidateLocation::Committing;
                    Arc::clone(payload)
                }
                _ => return Err(CoordinatorError::ConflictInvariant),
            };
            coordinator.apply_conflict_delta(&delta)?;
            coordinator.apply_conflict_ticket_plan(ticket_plan)?;
            let version = coordinator
                .entries
                .get(&hash)
                .map(CoordinatorEntry::version)
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            Ok(CommitLease {
                hash: hash.clone(),
                version,
                payload,
            })
        })?;
        self.refresh_victim_indexes(&lease.hash, old_victim_keys);
        Ok(Some(lease))
    }

    #[cfg(test)]
    pub(crate) fn abort_commit(
        &mut self,
        lease: &CommitLease<V>,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        self.validate_version_location(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::Committing,
        )?;
        let old_victim_keys = self.current_victim_keys(&lease.hash);
        self.ensure_revision_capacity(&lease.hash)?;
        let (source, candidate) = self
            .entries
            .get(&lease.hash)
            .and_then(|entry| {
                entry
                    .candidate()
                    .cloned()
                    .map(|candidate| (entry.source, candidate))
            })
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        let next_rank = CandidateRank::verified(&lease.hash, source, &candidate);
        let delta =
            self.preview_conflict_rerank(&lease.hash, &next_rank, CandidateLocation::Verified)?;
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
        let undo = delta.affected().to_vec();
        let version = self.with_entry_undo(&undo, |coordinator| {
            let ticket_plan = coordinator.prepare_conflict_ticket_plan(
                &delta,
                &HashSet::new(),
                &HashMap::new(),
            )?;
            coordinator.remove_conflict_tickets(&ticket_plan)?;
            coordinator.deactivate_source(source)?;
            let entry = coordinator.entry_mut(&lease.hash)?;
            match &mut entry.state {
                EntryState::CandidateVerified { location, .. } => {
                    *location = CandidateLocation::Verified;
                }
                _ => return Err(CoordinatorError::ConflictInvariant),
            }
            coordinator.apply_conflict_delta(&delta)?;
            if !ticket_plan.revises(&lease.hash) {
                coordinator.entry_mut(&lease.hash)?.revision += 1;
            }
            coordinator.apply_conflict_ticket_plan(ticket_plan)?;
            if let Some(deadline) = deadline {
                coordinator.deadlines.push(Reverse(deadline.clone()));
                coordinator
                    .live_deadlines
                    .insert(lease.hash.clone(), deadline);
                coordinator.compact_deadlines();
            }
            coordinator
                .entries
                .get(&lease.hash)
                .map(CoordinatorEntry::version)
                .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))
        })?;
        self.refresh_victim_indexes(&lease.hash, old_victim_keys);
        Ok(version)
    }

    /// Definitively fail exactly one committing owner. Unlike
    /// [`Self::abort_commit`], this is a terminal transition: dependents are
    /// invalidated and newly eligible conflict candidates are reconciled in
    /// the same transaction. The versioned commit lease prevents a late
    /// submit task from removing a re-admitted transaction with the same
    /// hash.
    pub(crate) fn fail_commit(
        &mut self,
        lease: &CommitLease<V>,
        disposition: TerminalDisposition,
    ) -> Result<TerminalRecord<R>, CoordinatorError> {
        self.validate_version_location(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::Committing,
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

    pub(crate) fn commit_candidate_handoff(
        &mut self,
        lease: &CommitLease<V>,
    ) -> Result<ConflictCommitHandoff<R>, CoordinatorError> {
        self.validate_version_location(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::Committing,
        )?;
        let winner_inputs = self
            .entries
            .get(&lease.hash)
            .and_then(CoordinatorEntry::candidate)
            .map(|candidate| candidate.inputs.clone())
            .ok_or(CoordinatorError::ConflictInvariant)?;
        let rejected = self.bounded_conflicting_candidates(&lease.hash, &winner_inputs)?;
        if self
            .conflicts
            .relations
            .get(&lease.hash)
            .map(|relation| relation.degree)
            != Some(rejected.len())
        {
            return Err(CoordinatorError::ConflictInvariant);
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
                || entry.location() != CoordinatorLocation::Verified
                || self.candidate_is_eligible(hash)
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
        self.with_entry_undo(&undo, |coordinator| {
            for hash in &rejected {
                coordinator.mark_children_invalid(hash, hash)?;
                coordinator.apply_fault_checkpoint();
            }
            let mut removed_candidates: HashSet<_> = rejected
                .iter()
                .filter(|hash| {
                    coordinator
                        .entries
                        .get(*hash)
                        .and_then(CoordinatorEntry::candidate)
                        .is_some()
                })
                .cloned()
                .collect();
            removed_candidates.insert(lease.hash.clone());
            let delta = coordinator.preview_conflict_remove_many(&removed_candidates)?;
            let ticket_plan = coordinator.prepare_conflict_ticket_plan(
                &delta,
                &HashSet::new(),
                &HashMap::new(),
            )?;
            coordinator.remove_conflict_tickets(&ticket_plan)?;
            coordinator.apply_conflict_delta(&delta)?;
            coordinator.apply_conflict_ticket_plan(ticket_plan)?;
            coordinator.apply_fault_checkpoint();

            for hash in rejected {
                // A direct loser may already have become dependency-invalid
                // while the batch was prepared; its conflict projection is
                // absent in that case, but lifecycle ownership remains.
                coordinator.remove_current_queue_ticket(&hash)?;
                let active_source = coordinator
                    .entries
                    .get(&hash)
                    .and_then(|entry| entry.uses_active_slot().then_some(entry.source));
                let entry =
                    coordinator.remove_present_after_conflicts_apply(&hash, active_source)?;
                terminal.push(Self::terminal_record(
                    hash,
                    entry,
                    TerminalDisposition::Rejected,
                ));
                coordinator.apply_fault_checkpoint();
            }
            let ready_children = coordinator.parent_available(&lease.hash)?;
            let active_source = coordinator
                .entries
                .get(&lease.hash)
                .and_then(|entry| entry.uses_active_slot().then_some(entry.source));
            let entry =
                coordinator.remove_present_after_conflicts_apply(&lease.hash, active_source)?;
            coordinator.apply_fault_checkpoint();
            let EntryState::CandidateVerified {
                raw,
                payload: _,
                location: CandidateLocation::Committing,
                ..
            } = entry.state
            else {
                return Err(CoordinatorError::ConflictInvariant);
            };
            #[cfg(not(test))]
            let _ = &ready_children;
            Ok(ConflictCommitHandoff {
                winner: CommitHandoff {
                    #[cfg(test)]
                    hash: lease.hash.clone(),
                    raw,
                    #[cfg(test)]
                    peer: entry.source.peer(),
                    #[cfg(test)]
                    ready_children,
                },
                rejected: terminal,
            })
        })
    }

    fn commit_handoff_undo_hashes(
        &self,
        lease: &CommitLease<V>,
    ) -> Result<Vec<Byte32>, CoordinatorError> {
        self.validate_version_location(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::Committing,
        )?;
        let mut roots = vec![lease.hash.clone()];
        if let Some(candidate) = self
            .entries
            .get(&lease.hash)
            .and_then(CoordinatorEntry::candidate)
        {
            let rejected = self.bounded_conflicting_candidates(&lease.hash, &candidate.inputs)?;
            roots.extend(rejected);
        }
        Ok(self.causal_undo_hashes(&roots))
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
    ) -> Result<ConflictCommitHandoff<R>, CoordinatorError> {
        let mut undo = self.commit_handoff_undo_hashes(lease)?;
        undo.extend(self.parents_unavailable_undo_hashes(unavailable_parents));
        undo.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        undo.dedup();
        self.with_entry_undo(&undo, |coordinator| {
            coordinator.parents_unavailable(unavailable_parents)?;
            let handoff = coordinator.commit_candidate_handoff(lease)?;
            #[cfg(test)]
            coordinator.handoff_error_checkpoint()?;
            Ok(handoff)
        })
    }

    /// Consume a transaction that became committed through an attached block
    /// rather than this coordinator's submit path. Chain membership is
    /// authoritative: dependents are woken, not invalidated, and every stale
    /// worker/commit lease becomes harmless when the entry is removed.
    #[cfg(test)]
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
    /// journaled pool removal are reclassified in the same transition that
    /// wakes consumers of the committed hash.
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
                let record = coordinator.external_commit_apply(hash)?;
                #[cfg(test)]
                coordinator.handoff_error_checkpoint()?;
                Ok(record)
            })
        } else {
            undo.retain(|affected| affected != hash);
            self.with_absent_entry_undo(hash, &undo, |coordinator| {
                coordinator.parents_unavailable(unavailable_parents)?;
                let record = coordinator.external_commit_apply(hash)?;
                #[cfg(test)]
                coordinator.handoff_error_checkpoint()?;
                Ok(record)
            })
        }
    }

    /// Apply a reorg's complete membership delta to the coordinator in one
    /// targeted transaction. `committed` parents remain available and wake
    /// consumers; `unavailable_parents` were physically removed from the
    /// accepted pool and reclassify consumers. The snapshot records both
    /// present and absent committed hashes because attached transactions need
    /// not have entered this node's pipeline.
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
        let raw = Arc::clone(entry.state.raw());
        #[cfg(not(test))]
        let _ = &ready_children;
        Ok(Some(ExternalCommitRecord {
            raw,
            #[cfg(test)]
            hash: hash.clone(),
            #[cfg(test)]
            ready_children,
        }))
    }

    pub(crate) fn force_terminalize(
        &mut self,
        hash: &Byte32,
        disposition: TerminalDisposition,
    ) -> Result<Option<TerminalRecord<R>>, CoordinatorError> {
        if !self.entries.contains_key(hash) {
            return Ok(None);
        }
        if self.entries.get(hash).is_some_and(|entry| {
            matches!(
                &entry.state,
                EntryState::CandidateVerified {
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

    /// Terminalize one bounded administrative cohort as a single ownership
    /// transaction. Callers may select hashes before acquiring the mutation
    /// lock, so absent entries and owners that have since entered the
    /// non-cancellable commit boundary are deliberately skipped. Every other
    /// selected owner either leaves together or is restored together.
    pub(crate) fn force_terminalize_many(
        &mut self,
        hashes: &[Byte32],
        disposition: TerminalDisposition,
    ) -> Result<Vec<TerminalRecord<R>>, CoordinatorError> {
        let mut roots: Vec<_> = hashes
            .iter()
            .filter(|hash| {
                self.entries.get(*hash).is_some_and(|entry| {
                    !matches!(
                        &entry.state,
                        EntryState::CandidateVerified {
                            location: CandidateLocation::Committing,
                            ..
                        }
                    )
                })
            })
            .cloned()
            .collect();
        roots.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        roots.dedup();
        if roots.is_empty() {
            return Ok(Vec::new());
        }

        let undo = self.causal_undo_hashes(&roots);
        let mut terminal = Vec::new();
        terminal
            .try_reserve(roots.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.with_entry_undo(&undo, |coordinator| {
            for hash in roots {
                coordinator.mark_children_invalid(&hash, &hash)?;
                let entry = coordinator.remove_present_apply(&hash)?;
                coordinator.apply_fault_checkpoint();
                terminal.push(Self::terminal_record(hash, entry, disposition));
            }
            Ok(terminal)
        })
    }

    /// Expiry is incarnation-scoped rather than revision-scoped: ordinary
    /// stage transitions cannot extend a remote transaction's original
    /// lifetime, while removal/re-admission makes the old ticket stale.
    pub(crate) fn expire_due(
        &mut self,
        now: u64,
        max: usize,
    ) -> Result<Vec<TerminalRecord<R>>, CoordinatorError> {
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
                EntryState::CandidateVerified {
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

    pub(crate) fn clear(&mut self) -> Result<Vec<TerminalRecord<R>>, CoordinatorError> {
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
        self.waiting_parent_count = 0;
        self.dependency_failures.clear();
        self.dependency_failure_set.clear();
        self.conflicts.clear();
        self.deadlines.clear();
        self.live_deadlines.clear();
        self.capacity_victim_index.clear();
        self.candidate_victim_index.clear();
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
        let entry = self.entry_mut(hash)?;
        let old_ticket = entry.ticket(hash);
        entry.revision = revision;
        if let Some(kind) = entry.queue_kind() {
            let new_ticket = entry.ticket(hash);
            let front = entry.source.is_proposal();
            let owner = entry.source.queue_owner();
            let is_large_cycle = new_ticket.verify_schedule.is_large_cycle;
            let queue = self.queue_mut(kind)?;
            queue.remove_live(kind, &old_ticket)?;
            queue.reserve_live(owner, is_large_cycle)?;
            queue.push_reserved(kind, new_ticket, front)?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn mutate_outside_undo_cohort_for_test(
        &mut self,
        snapshotted: &Byte32,
        escaped: &Byte32,
    ) -> Result<(), CoordinatorError> {
        self.with_entry_undo(std::slice::from_ref(snapshotted), |coordinator| {
            coordinator.entry_mut(escaped)?.revision += 1;
            Ok(())
        })
    }

    #[cfg(test)]
    pub(crate) fn expand_nested_undo_cohort_for_test(
        &mut self,
        outer: &Byte32,
        escaped: &Byte32,
    ) -> Result<(), CoordinatorError> {
        self.with_entry_undo(std::slice::from_ref(outer), |coordinator| {
            coordinator.with_entry_undo(std::slice::from_ref(escaped), |_| Ok(()))
        })
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
    pub(crate) fn take_queue_selection_probes_for_test(&mut self, kind: QueueKind) -> usize {
        self.queues
            .get_mut(&kind)
            .map_or(0, TicketQueue::take_selection_probes)
    }

    #[cfg(test)]
    pub(crate) fn take_capacity_victim_probes_for_test(&self) -> usize {
        self.capacity_victim_probes.replace(0)
    }

    #[cfg(test)]
    pub(crate) fn take_candidate_victim_probes_for_test(&self) -> usize {
        self.candidate_victim_probes.replace(0)
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
    pub(crate) fn fail_next_handoff_after_apply_for_test(&mut self, error: CoordinatorError) {
        self.fail_next_handoff_after_apply = Some(error);
    }

    #[cfg(test)]
    fn handoff_error_checkpoint(&mut self) -> Result<(), CoordinatorError> {
        self.fail_next_handoff_after_apply
            .take()
            .map_or(Ok(()), Err)
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
        Ok(queue.peek_eligible(capability, |owner| match owner {
            QueueOwner::Trusted => true,
            QueueOwner::Remote(peer) => {
                active_by_peer.get(&peer).copied().unwrap_or(0) < per_peer_limit
            }
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
        let Some(kind) = self.candidate_queue_kind(hash, entry) else {
            return Ok(());
        };
        let ticket = entry.ticket(hash);
        let queue = self.queue_mut(kind)?;
        queue.remove_live(kind, &ticket)?;
        queue.compact();
        Ok(())
    }

    /// Remove whichever scheduling projection the authoritative entry owns.
    /// Candidate tickets are part of the conflict delta and must not be
    /// removed separately, otherwise eligibility reconciliation observes a
    /// false missing-ticket invariant.
    fn remove_current_scheduling(&mut self, hash: &Byte32) -> Result<(), CoordinatorError> {
        if self
            .entries
            .get(hash)
            .and_then(CoordinatorEntry::candidate)
            .is_some()
        {
            self.remove_conflict_indexes(hash)
        } else {
            self.remove_current_queue_ticket(hash)
        }
    }

    fn validate_version_location(
        &self,
        hash: &Byte32,
        version: CoordinatorVersion,
        expected_location: &CoordinatorLocation,
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
        if !entry.state.location_matches(expected_location) {
            return Err(CoordinatorError::LocationMismatch {
                expected: expected_location.clone(),
                actual: entry.location(),
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

    fn capacity_victim_key(
        hash: &Byte32,
        entry: &CoordinatorEntry<R, U, V>,
    ) -> Option<CapacityVictimKey> {
        (!entry.is_committing()).then(|| CapacityVictimKey {
            valid: entry.invalidated_cause().is_none(),
            source_strength: entry.source.trust(),
            charge_bytes: entry.charge_bytes,
            queue_sequence: entry.queue_sequence,
            hash: hash.clone(),
        })
    }

    fn candidate_victim_key(
        hash: &Byte32,
        entry: &CoordinatorEntry<R, U, V>,
    ) -> Option<CandidateVictimKey> {
        let candidate = entry.candidate()?;
        (!entry.is_committing()).then(|| CandidateVictimKey {
            source_strength: entry.source.trust(),
            fee: candidate.fee,
            tx_size: candidate.tx_size,
            arrival: candidate.arrival,
            hash: hash.clone(),
        })
    }

    fn sync_victim_indexes(&mut self, snapshot: &[(Byte32, Option<CoordinatorEntry<R, U, V>>)]) {
        for (hash, old_entry) in snapshot {
            if let Some(key) = old_entry
                .as_ref()
                .and_then(|entry| Self::capacity_victim_key(hash, entry))
            {
                self.capacity_victim_index.remove(&key);
            }
            if let Some(key) = old_entry
                .as_ref()
                .and_then(|entry| Self::candidate_victim_key(hash, entry))
            {
                self.candidate_victim_index.remove(&key);
            }
        }
        for (hash, _) in snapshot {
            if let Some(key) = self
                .entries
                .get(hash)
                .and_then(|entry| Self::capacity_victim_key(hash, entry))
            {
                // Replace keeps publication idempotent when a snapshot lists
                // the same hash through more than one causal relation.
                self.capacity_victim_index.replace(key);
            }
            if let Some(key) = self
                .entries
                .get(hash)
                .and_then(|entry| Self::candidate_victim_key(hash, entry))
            {
                self.candidate_victim_index.replace(key);
            }
        }
    }

    fn current_victim_keys(
        &self,
        hash: &Byte32,
    ) -> (Option<CapacityVictimKey>, Option<CandidateVictimKey>) {
        let entry = self.entries.get(hash);
        (
            entry.and_then(|entry| Self::capacity_victim_key(hash, entry)),
            entry.and_then(|entry| Self::candidate_victim_key(hash, entry)),
        )
    }

    fn refresh_victim_indexes(
        &mut self,
        hash: &Byte32,
        old: (Option<CapacityVictimKey>, Option<CandidateVictimKey>),
    ) {
        if self.entry_transaction_depth != 0 {
            // The outer undo snapshot owns derived-index publication for the
            // complete nested mutation cohort.
            return;
        }
        if let Some(key) = old.0 {
            self.capacity_victim_index.remove(&key);
        }
        if let Some(key) = old.1 {
            self.candidate_victim_index.remove(&key);
        }
        if let Some(key) = self
            .entries
            .get(hash)
            .and_then(|entry| Self::capacity_victim_key(hash, entry))
        {
            self.capacity_victim_index.replace(key);
        }
        if let Some(key) = self
            .entries
            .get(hash)
            .and_then(|entry| Self::candidate_victim_key(hash, entry))
        {
            self.candidate_victim_index.replace(key);
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
            let incoming_strength = source.trust();
            let victim = children
                .iter()
                .filter(|child| !selected.contains(*child))
                .filter(|child| !protected.contains(*child))
                .filter_map(|child| self.entries.get(child).map(|entry| (child, entry)))
                .filter(|(_, entry)| {
                    !entry.is_committing()
                        && (entry.invalidated_cause().is_some()
                            || entry.source.trust() < incoming_strength)
                })
                .min_by(|(left_hash, left), (right_hash, right)| {
                    left.invalidated_cause()
                        .is_none()
                        .cmp(&right.invalidated_cause().is_none())
                        .then_with(|| left.source.trust().cmp(&right.source.trust()))
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
        let incoming_strength = incoming_source.trust();
        for key in &self.capacity_victim_index {
            #[cfg(test)]
            self.capacity_victim_probes
                .set(self.capacity_victim_probes.get().saturating_add(1));
            if projected.fits(self.limits.global) {
                break;
            }
            // Invalidated work sorts first and is always reclaimable. Once
            // the valid suffix reaches the incoming source strength, no later
            // key can be an eligible victim.
            if key.valid && key.source_strength >= incoming_strength {
                break;
            }
            if incoming_hash == Some(&key.hash)
                || selected.contains(&key.hash)
                || protected.contains(&key.hash)
            {
                continue;
            }
            if selected.len() >= self.limits.max_capacity_evictions_per_transition {
                return Err(CoordinatorError::CapacityEvictionLimitExceeded);
            }
            let charge_bytes = self
                .entries
                .get(&key.hash)
                .map(|entry| entry.charge_bytes)
                .ok_or_else(|| CoordinatorError::Missing(key.hash.clone()))?;
            selected.insert(key.hash.clone());
            projected = projected
                .checked_sub(CoordinatorResidency::new(1, charge_bytes))
                .ok_or(CoordinatorError::GlobalBudgetExceeded)?;
            victims.push(key.hash.clone());
        }
        if !projected.fits(self.limits.global) {
            return Err(CoordinatorError::GlobalBudgetExceeded);
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
        CandidateRank::verified(left_hash, left_source, left).cmp(&CandidateRank::verified(
            right_hash,
            right_source,
            right,
        ))
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
            let Some(candidates) = self.conflicts.by_input.get(&input) else {
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
        let mut projected_edges = self.conflicts.input_memberships;
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
        let incoming_key = CandidateVictimKey {
            source_strength: incoming_source.trust(),
            fee: incoming.fee,
            tx_size: incoming.tx_size,
            arrival: incoming.arrival,
            hash: incoming_hash.clone(),
        };
        for key in &self.candidate_victim_index {
            #[cfg(test)]
            self.candidate_victim_probes
                .set(self.candidate_victim_probes.get().saturating_add(1));
            if projected_edges <= self.limits.max_conflict_edges {
                break;
            }
            if key >= &incoming_key {
                break;
            }
            if &key.hash == incoming_hash
                || selected.contains(&key.hash)
                || protected.contains(&key.hash)
            {
                continue;
            }
            if selected.len() >= self.limits.max_capacity_evictions_per_transition {
                return Err(CoordinatorError::CapacityEvictionLimitExceeded);
            }
            let edges = self
                .entries
                .get(&key.hash)
                .and_then(CoordinatorEntry::candidate)
                .map(|candidate| candidate.inputs.len())
                .ok_or_else(|| CoordinatorError::Missing(key.hash.clone()))?;
            selected.insert(key.hash.clone());
            victims.push(key.hash.clone());
            projected_edges = projected_edges
                .checked_sub(edges)
                .ok_or(CoordinatorError::ConflictEdgeLimitExceeded)?;
        }
        if projected_edges > self.limits.max_conflict_edges {
            return Err(CoordinatorError::ConflictEdgeLimitExceeded);
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

    /// Canonical entry charge equation used by both production undo rebuild
    /// and the independent test auditor. Index reconstruction intentionally
    /// remains independent so it can detect implementation drift, while the
    /// accounting contract itself has exactly one definition.
    fn entry_metadata_charge_is_valid(&self, entry: &CoordinatorEntry<R, U, V>) -> bool {
        let conflict_inputs = entry.candidate().map_or(0, |meta| meta.inputs.len());
        let Ok(base_metadata) =
            self.metadata_charge_bytes(entry.dependencies.len(), entry.expires_at.is_some(), 0)
        else {
            return false;
        };
        let Ok(metadata) = self.metadata_charge_bytes(
            entry.dependencies.len(),
            entry.expires_at.is_some(),
            conflict_inputs,
        ) else {
            return false;
        };
        let Some(raw_charge) = entry.raw_resident_payload_bytes.checked_add(base_metadata) else {
            return false;
        };
        let Some(charge) = entry.resident_payload_bytes.checked_add(metadata) else {
            return false;
        };
        entry.base_metadata_bytes == base_metadata
            && entry.metadata_bytes == metadata
            && entry.raw_charge_bytes == raw_charge
            && entry.charge_bytes == charge
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
        self.entry_mut(hash)?.charge_bytes = new_bytes;
        Ok(())
    }

    fn with_capacity_victims<T, F>(
        &mut self,
        subject: CapacitySubject,
        victims: Vec<Byte32>,
        subject_undo: Vec<Byte32>,
        apply_subject: F,
    ) -> Result<(T, Vec<TerminalRecord<R>>), CoordinatorError>
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
        affected.extend(subject_undo);
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
        let mut snapshot = Vec::new();
        for hash in hashes {
            if unique.contains(hash) {
                continue;
            }
            unique
                .try_reserve(1)
                .map_err(|_| CoordinatorError::QueueReservationFailed)?;
            snapshot
                .try_reserve(1)
                .map_err(|_| CoordinatorError::QueueReservationFailed)?;
            unique.insert(hash.clone());
            let entry = self
                .entries
                .get(hash)
                .cloned()
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            snapshot.push((hash.clone(), Some(entry)));
        }
        self.with_entry_snapshot(snapshot, unique, apply)
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
            .try_reserve(1)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        let mut snapshot = Vec::new();
        snapshot
            .try_reserve(1)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        unique.insert(absent.clone());
        snapshot.push((absent.clone(), None));
        for hash in hashes {
            if unique.contains(hash) {
                continue;
            }
            unique
                .try_reserve(1)
                .map_err(|_| CoordinatorError::QueueReservationFailed)?;
            snapshot
                .try_reserve(1)
                .map_err(|_| CoordinatorError::QueueReservationFailed)?;
            unique.insert(hash.clone());
            let entry = self
                .entries
                .get(hash)
                .cloned()
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            snapshot.push((hash.clone(), Some(entry)));
        }
        self.with_entry_snapshot(snapshot, unique, apply)
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
        let mut snapshot = Vec::new();
        for hash in hashes {
            if unique.contains(hash) {
                continue;
            }
            unique
                .try_reserve(1)
                .map_err(|_| CoordinatorError::QueueReservationFailed)?;
            snapshot
                .try_reserve(1)
                .map_err(|_| CoordinatorError::QueueReservationFailed)?;
            unique.insert(hash.clone());
            snapshot.push((hash.clone(), self.entries.get(hash).cloned()));
        }
        self.with_entry_snapshot(snapshot, unique, apply)
    }

    fn with_entry_snapshot<T, F>(
        &mut self,
        snapshot: Vec<(Byte32, Option<CoordinatorEntry<R, U, V>>)>,
        cohort: HashSet<Byte32>,
        apply: F,
    ) -> Result<T, CoordinatorError>
    where
        F: FnOnce(&mut Self) -> Result<T, CoordinatorError>,
    {
        let next_incarnation = self.next_incarnation;
        let next_arrival = self.next_arrival;
        let next_maintenance_sequence = self.next_maintenance_sequence;
        let next_queue_sequence = self.next_queue_sequence;
        let outermost = self.entry_transaction_depth == 0;
        self.begin_entry_transaction(&cohort)?;
        let outcome = catch_unwind(AssertUnwindSafe(|| apply(self)));
        self.end_entry_transaction(&cohort);
        match outcome {
            Ok(Ok(value)) => {
                if outermost {
                    self.sync_victim_indexes(&snapshot);
                }
                Ok(value)
            }
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

    fn begin_entry_transaction(
        &mut self,
        cohort: &HashSet<Byte32>,
    ) -> Result<(), CoordinatorError> {
        let depth = self.entry_transaction_depth;
        // A nested snapshot may narrow an outer cohort, but it cannot add an
        // entry the outer transaction would be unable to restore.
        if let Some(hash) = cohort.iter().find(|hash| {
            self.entry_transaction_membership
                .get(*hash)
                .copied()
                .unwrap_or(0)
                != depth
        }) {
            return Err(CoordinatorError::UndoCohortViolation {
                hash: hash.clone(),
                active_depth: depth,
                snapshotted_depth: self
                    .entry_transaction_membership
                    .get(hash)
                    .copied()
                    .unwrap_or(0),
                mutation_file: "nested undo cohort",
                mutation_line: 0,
                active_members: self
                    .entry_transaction_membership
                    .iter()
                    .filter_map(|(hash, count)| (*count == depth).then(|| hash.clone()))
                    .collect(),
            });
        }
        self.entry_transaction_membership
            .try_reserve(cohort.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        let next_depth = depth
            .checked_add(1)
            .expect("coordinator undo nesting is statically bounded");
        for hash in cohort {
            self.entry_transaction_membership
                .insert(hash.clone(), next_depth);
        }
        self.entry_transaction_depth = next_depth;
        Ok(())
    }

    fn end_entry_transaction(&mut self, cohort: &HashSet<Byte32>) {
        let depth = self.entry_transaction_depth;
        debug_assert_ne!(depth, 0);
        for hash in cohort {
            let remove = {
                let count = self
                    .entry_transaction_membership
                    .get_mut(hash)
                    .expect("active undo cohort membership exists");
                assert_eq!(*count, depth, "undo cohort nesting remains exact");
                *count -= 1;
                *count == 0
            };
            if remove {
                self.entry_transaction_membership.remove(hash);
            }
        }
        self.entry_transaction_depth = depth - 1;
    }

    #[track_caller]
    fn ensure_entry_mutation_is_snapshotted(&self, hash: &Byte32) -> Result<(), CoordinatorError> {
        if self.entry_transaction_depth != 0
            && self.entry_transaction_membership.get(hash).copied()
                != Some(self.entry_transaction_depth)
        {
            let caller = std::panic::Location::caller();
            return Err(CoordinatorError::UndoCohortViolation {
                hash: hash.clone(),
                active_depth: self.entry_transaction_depth,
                snapshotted_depth: self
                    .entry_transaction_membership
                    .get(hash)
                    .copied()
                    .unwrap_or(0),
                mutation_file: caller.file(),
                mutation_line: caller.line(),
                active_members: self
                    .entry_transaction_membership
                    .iter()
                    .filter_map(|(hash, count)| {
                        (*count == self.entry_transaction_depth).then(|| hash.clone())
                    })
                    .collect(),
            });
        }
        Ok(())
    }

    #[track_caller]
    fn entry_mut(
        &mut self,
        hash: &Byte32,
    ) -> Result<&mut CoordinatorEntry<R, U, V>, CoordinatorError> {
        self.ensure_entry_mutation_is_snapshotted(hash)?;
        self.entries
            .get_mut(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))
    }

    fn insert_absent_entry(
        &mut self,
        hash: Byte32,
        entry: CoordinatorEntry<R, U, V>,
    ) -> Result<(), CoordinatorError> {
        self.ensure_entry_mutation_is_snapshotted(&hash)?;
        if self.entries.insert(hash.clone(), entry).is_some() {
            return Err(CoordinatorError::DuplicateHash(hash));
        }
        Ok(())
    }

    fn remove_present_entry(
        &mut self,
        hash: &Byte32,
    ) -> Result<CoordinatorEntry<R, U, V>, CoordinatorError> {
        self.ensure_entry_mutation_is_snapshotted(hash)?;
        self.entries
            .remove(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))
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

    /// Convert one live owner into raw-only terminal maintenance work. Typed
    /// resolved/verified payloads have no consumer after definitive
    /// dependency failure: retaining them would preserve a dead phase and let
    /// an attacker pin its larger residency until the bounded cascade drains.
    fn invalidate_present_apply(
        &mut self,
        hash: &Byte32,
        cause: &Byte32,
        sequence: u64,
    ) -> Result<(), CoordinatorError> {
        let (active_source, raw_charge, was_waiting, already_invalidated) = self
            .entries
            .get(hash)
            .map(|entry| {
                (
                    entry.uses_active_slot().then_some(entry.source),
                    entry.raw_charge_bytes,
                    matches!(
                        &entry.state,
                        EntryState::Raw {
                            location: RawLocation::WaitingParents { .. },
                            ..
                        }
                    ),
                    entry.invalidated_cause().is_some(),
                )
            })
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        if already_invalidated {
            return Err(CoordinatorError::ConflictInvariant);
        }
        if let Some(source) = active_source {
            self.deactivate_source(source)?;
        }
        self.remove_current_scheduling(hash)?;
        self.apply_fault_checkpoint();
        self.apply_recharge(hash, raw_charge)?;
        let entry = self.entry_mut(hash)?;
        let raw = Arc::clone(entry.state.raw());
        entry.state = EntryState::Invalidated {
            raw,
            cause: cause.clone(),
            sequence,
        };
        entry.resident_payload_bytes = entry.raw_resident_payload_bytes;
        entry.metadata_bytes = entry.base_metadata_bytes;
        entry.revision += 1;
        if was_waiting {
            self.leave_waiting_parent()?;
        }
        if !self.dependency_failure_set.insert(hash.clone()) {
            return Err(CoordinatorError::ConflictInvariant);
        }
        self.dependency_failures.push_back(hash.clone());
        self.apply_fault_checkpoint();
        Ok(())
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
            let (uses_active_slot, source, raw_charge) = {
                let entry = self
                    .entries
                    .get(child)
                    .ok_or_else(|| CoordinatorError::Missing(child.clone()))?;
                (
                    entry.uses_active_slot(),
                    entry.source,
                    entry.raw_charge_bytes,
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
            self.check_recharge(child, raw_charge)?;
        }
        self.dependency_failures
            .try_reserve(children.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.dependency_failure_set
            .try_reserve(children.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        let (first_sequence, next_maintenance_sequence) =
            self.maintenance_sequence_range(children.len())?;
        let undo_hashes = self.conflict_undo_hashes(&children);
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
                coordinator.invalidate_present_apply(child, cause, sequence)?;
            }
            Ok(result)
        })
    }

    fn causal_undo_hashes(&self, roots: &[Byte32]) -> Vec<Byte32> {
        let mut affected: HashSet<_> = roots.iter().cloned().collect();
        for root in roots {
            if let Some(children) = self.by_parent.get(root) {
                affected.extend(children.iter().cloned());
            }
        }
        let affected: Vec<_> = affected.into_iter().collect();
        self.conflict_undo_hashes(&affected)
    }

    fn conflict_undo_hashes(&self, roots: &[Byte32]) -> Vec<Byte32> {
        let mut affected: HashSet<_> = roots.iter().cloned().collect();
        // Removing a candidate can change commit eligibility only for its
        // direct staged neighbours; no transitive conflict closure is needed.
        for hash in roots {
            if let Some(candidate) = self.entries.get(hash).and_then(CoordinatorEntry::candidate) {
                for input in &candidate.inputs {
                    if let Some(neighbours) = self.conflicts.by_input.get(input) {
                        affected.extend(neighbours.iter().cloned());
                    }
                }
            }
        }
        let mut affected: Vec<_> = affected.into_iter().collect();
        affected.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
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
        self.remove_current_scheduling(hash)?;
        self.apply_fault_checkpoint();
        self.remove_present_after_conflicts_apply(hash, active_source)
    }

    /// Remove an owner after its queue membership and staged-conflict delta
    /// have already been applied. Commit handoff uses this to remove winner
    /// plus direct losers with one graph delta instead of K sequential
    /// eligibility oscillations.
    fn remove_present_after_conflicts_apply(
        &mut self,
        hash: &Byte32,
        active_source: Option<CoordinatorSource>,
    ) -> Result<CoordinatorEntry<R, U, V>, CoordinatorError> {
        if let Some(source) = active_source {
            self.deactivate_source(source)?;
        }
        let was_waiting = self.entries.get(hash).is_some_and(|entry| {
            matches!(
                &entry.state,
                EntryState::Raw {
                    location: RawLocation::WaitingParents { .. },
                    ..
                }
            )
        });
        let entry = self.remove_present_entry(hash)?;
        if was_waiting {
            self.leave_waiting_parent()?;
        }
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
        if self.dependency_failures.len()
            > lazy_ticket_compaction_limit(self.dependency_failure_set.len())
        {
            self.dependency_failures
                .retain(|hash| self.dependency_failure_set.contains(hash));
        }
    }

    fn preview_dependency_failure_roots(&self, max: usize) -> Vec<Byte32> {
        // `mark_children_invalid` appends the next causal frontier while the
        // selected roots are applied. Previewing only the already-live FIFO
        // frontier keeps every maintenance turn O(max); cloning the complete
        // backlog here made a fixed-size drain quadratic in backlog length.
        self.dependency_failures
            .iter()
            .filter(|hash| self.dependency_failure_set.contains(*hash))
            .take(max)
            .cloned()
            .collect()
    }

    fn compact_deadlines(&mut self) {
        if self.deadlines.len() > lazy_ticket_compaction_limit(self.live_deadlines.len()) {
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
    ) -> TerminalRecord<R> {
        let raw = Arc::clone(entry.state.raw());
        #[cfg(not(test))]
        let _ = disposition;
        TerminalRecord {
            hash,
            raw,
            source: entry.source,
            #[cfg(test)]
            disposition,
        }
    }
}
