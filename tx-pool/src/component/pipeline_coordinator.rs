//! Single-authority model for the target tx-pool pipeline.
//!
//! This module is intentionally isolated from the production hot path while
//! the legacy queue/wait/conflict owners are replaced. Unlike the earlier
//! split prototypes, lifecycle state, payload phase, worker leases,
//! dependency edges, queue tickets and residency accounting live in one
//! authoritative store and use one incarnation/revision.
#![allow(dead_code)]

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
    pool_waiters_by_input: HashMap<OutPoint, HashSet<Byte32>>,
    pool_input_edge_count: usize,
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
    #[cfg(test)]
    fault_after_apply_steps: Option<usize>,
    #[cfg(test)]
    apply_steps_seen: usize,
}

impl<R, U, V> PipelineCoordinator<R, U, V> {
    pub(crate) fn new(limits: CoordinatorLimits) -> Self {
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
            pool_waiters_by_input: HashMap::new(),
            pool_input_edge_count: 0,
            queues: HashMap::from([
                (QueueKind::PreCheck, TicketQueue::default()),
                (QueueKind::Resolve, TicketQueue::default()),
                (QueueKind::Verify, TicketQueue::default()),
                (QueueKind::Commit, TicketQueue::default()),
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

    pub(crate) fn view(&self, hash: &Byte32) -> Option<CoordinatorView> {
        self.entries.get(hash).map(CoordinatorEntry::view)
    }

    pub(crate) fn hash_by_short_id(&self, short_id: &ProposalShortId) -> Option<&Byte32> {
        self.by_short_id.get(short_id)
    }

    pub(crate) fn queue_len(&self, kind: QueueKind) -> usize {
        self.queues.get(&kind).map_or(0, |queue| queue.live.len())
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
    ) -> Result<CoordinatorVersion, CoordinatorError> {
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
            self.metadata_charge_bytes(dependencies.len(), expires_at.is_some(), 0, 0)?;
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
        let location = CoordinatorLocation::RawQueued(initial_stage);
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
            raw: Arc::new(raw),
            phase: ResidentPhase::Raw,
            location,
            source,
            expires_at,
            raw_charge_bytes: total_charge_bytes,
            raw_payload_bytes: charge_bytes,
            payload_bytes: charge_bytes,
            base_metadata_bytes,
            metadata_bytes: base_metadata_bytes,
            charge_bytes: total_charge_bytes,
            dependencies: dependencies.clone(),
            candidate: None,
            incarnation,
            revision: 0,
        };
        let ticket = entry.ticket(&hash);
        self.next_incarnation = next_incarnation;
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
                entry.location.queue_kind(),
                entry.version(),
                entry.location.uses_active_slot(),
            )
        };
        let target = match promotion {
            TrustedSource::Local => CoordinatorSource::Local,
            TrustedSource::Proposal => CoordinatorSource::Proposal,
        };
        if current == target || current == CoordinatorSource::Proposal {
            if current == CoordinatorSource::Proposal && target == CoordinatorSource::Local {
                return Err(CoordinatorError::SourceDowngrade);
            }
            return Ok(version);
        }
        let reticket = queue_kind.is_some();
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
            entry.revision += 1;
            let new_ticket = entry.ticket(hash);
            let queue = self.queue_mut(kind)?;
            queue.remove_live(&old_ticket);
            queue.push_reserved(kind, new_ticket, target.is_proposal())?;
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
        let Some(ticket) = self.peek_live_ticket(kind)? else {
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
        entry.location = CoordinatorLocation::RawActive(stage);
        entry.revision += 1;
        Ok(Some(RawWorkLease {
            hash: ticket.hash,
            stage,
            version: entry.version(),
            payload: Arc::clone(&entry.raw),
        }))
    }

    pub(crate) fn complete_raw(
        &mut self,
        lease: &RawWorkLease<R>,
        unverified: U,
        charge_bytes: usize,
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
        self.deactivate_source(source)?;
        self.apply_recharge(&lease.hash, total_charge_bytes)?;
        let entry = self
            .entries
            .get_mut(&lease.hash)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        entry.phase = ResidentPhase::Unverified(Arc::new(unverified));
        entry.payload_bytes = charge_bytes;
        entry.metadata_bytes = metadata_bytes;
        entry.location = CoordinatorLocation::VerifyQueued;
        entry.revision += 1;
        let version = entry.version();
        let ticket = entry.ticket(&lease.hash);
        let front = entry.source.is_proposal();
        self.queue_mut(QueueKind::Verify)?
            .push_reserved(QueueKind::Verify, ticket, front)?;
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
        entry.location = CoordinatorLocation::WaitingParents { missing };
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
        self.deactivate_source(source)?;
        let entry = self
            .entries
            .get_mut(&lease.hash)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        entry.location = CoordinatorLocation::RawQueued(lease.stage);
        entry.revision += 1;
        let version = entry.version();
        let ticket = entry.ticket(&lease.hash);
        let front = entry.source.is_proposal();
        self.queue_mut(kind)?.push_reserved(kind, ticket, front)?;
        Ok(version)
    }

    pub(crate) fn parent_available(
        &mut self,
        parent: &Byte32,
    ) -> Result<Vec<CoordinatorTicket>, CoordinatorError> {
        let children = self.by_parent.get(parent).cloned().unwrap_or_default();
        let mut affected = Vec::new();
        let mut ready_count = 0usize;
        let mut priority_ready_count = 0usize;
        let mut priority_owners = Vec::new();
        let mut normal_owners = Vec::new();
        for child in children {
            let Some(entry) = self.entries.get(&child) else {
                continue;
            };
            let CoordinatorLocation::WaitingParents { missing } = &entry.location else {
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

        let undo = affected.clone();
        self.with_entry_undo(&undo, |coordinator| {
            for child in affected {
                let entry = coordinator
                    .entries
                    .get_mut(&child)
                    .ok_or_else(|| CoordinatorError::Missing(child.clone()))?;
                let CoordinatorLocation::WaitingParents { missing } = &mut entry.location else {
                    return Err(CoordinatorError::LocationMismatch {
                        expected: CoordinatorLocation::WaitingParents {
                            missing: HashSet::from([parent.clone()]),
                        },
                        actual: entry.location.clone(),
                    });
                };
                missing.remove(parent);
                entry.revision += 1;
                if missing.is_empty() {
                    entry.location = CoordinatorLocation::RawQueued(RawStage::Resolve);
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

    pub(crate) fn parent_unavailable(
        &mut self,
        parent: &Byte32,
    ) -> Result<Vec<Byte32>, CoordinatorError> {
        let children = self.by_parent.get(parent).cloned().unwrap_or_default();
        let mut affected = Vec::new();
        for child in children {
            let Some(entry) = self.entries.get(&child) else {
                continue;
            };
            if matches!(
                &entry.location,
                CoordinatorLocation::WaitingParents { missing } if missing.contains(parent)
            ) {
                continue;
            }
            self.ensure_revision_capacity(&child)?;
            self.preflight_remove_conflict_indexes(&child)?;
            self.preflight_remove_pool_input_indexes(&child)?;
            affected.push(child);
        }

        let mut undo = affected.clone();
        for child in &affected {
            if let Some(waiters) = self.waiters_by_blocker.get(child) {
                undo.extend(waiters.iter().cloned());
            }
        }
        let result = affected.clone();
        self.with_entry_undo(&undo, |coordinator| {
            for child in &affected {
                let active_source = coordinator
                    .entries
                    .get(child)
                    .and_then(|entry| entry.location.uses_active_slot().then_some(entry.source));
                if let Some(source) = active_source {
                    coordinator.deactivate_source(source)?;
                }
                coordinator.remove_current_queue_ticket(child)?;
                coordinator.remove_pool_input_indexes(child)?;
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
                let mut missing = match &entry.location {
                    CoordinatorLocation::WaitingParents { missing } => missing.clone(),
                    _ => HashSet::new(),
                };
                missing.insert(parent.clone());
                entry.phase = ResidentPhase::Raw;
                entry.payload_bytes = entry.raw_payload_bytes;
                entry.metadata_bytes = entry.base_metadata_bytes;
                entry.candidate = None;
                entry.location = CoordinatorLocation::WaitingParents { missing };
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
                let cause = match coordinator.entries.get(&hash).map(|entry| &entry.location) {
                    Some(CoordinatorLocation::Invalidated { cause }) => cause.clone(),
                    Some(_) => return Err(CoordinatorError::ConflictInvariant),
                    None => return Err(CoordinatorError::Missing(hash)),
                };
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
    ) -> Result<Option<VerifyWorkLease<U>>, CoordinatorError> {
        let Some(ticket) = self.peek_live_ticket(QueueKind::Verify)? else {
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
        entry.location = CoordinatorLocation::VerifyActive;
        entry.revision += 1;
        let ResidentPhase::Unverified(payload) = &entry.phase else {
            return Err(CoordinatorError::PhaseMismatch {
                expected: PayloadPhase::Unverified,
                actual: entry.phase.kind(),
            });
        };
        Ok(Some(VerifyWorkLease {
            hash: ticket.hash,
            version: entry.version(),
            payload: Arc::clone(payload),
        }))
    }

    pub(crate) fn complete_verification(
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
        self.deactivate_source(source)?;
        self.apply_recharge(&lease.hash, total_charge_bytes)?;
        let entry = self
            .entries
            .get_mut(&lease.hash)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        entry.phase = ResidentPhase::Verified(Arc::new(verified));
        entry.payload_bytes = charge_bytes;
        entry.metadata_bytes = metadata_bytes;
        entry.location = CoordinatorLocation::ReadyToCommit;
        entry.revision += 1;
        let version = entry.version();
        let ticket = entry.ticket(&lease.hash);
        let front = entry.source.is_proposal();
        self.queue_mut(QueueKind::Commit)?
            .push_reserved(QueueKind::Commit, ticket, front)?;
        Ok(version)
    }

    pub(crate) fn complete_verification_candidate(
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
            self.metadata_charge_bytes(dependencies, has_deadline, candidate.inputs.len(), 0)?;
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
                    entry.location == CoordinatorLocation::ReadyToCommit
                        && entry.candidate.as_ref().is_some_and(|blocker_meta| {
                            Self::compare_candidates(&lease.hash, &meta, blocker, blocker_meta)
                                == Ordering::Greater
                        })
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

        if blockers.is_empty() || can_preempt {
            let source = self
                .entries
                .get(&lease.hash)
                .map(|entry| entry.source)
                .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
            self.queue_mut(QueueKind::Commit)?
                .reserve_live(source.is_proposal(), source.queue_owner())?;
        }
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
                    entry.location = CoordinatorLocation::WaitingConflict {
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
            entry.phase = ResidentPhase::Verified(Arc::new(verified));
            entry.payload_bytes = charge_bytes;
            entry.metadata_bytes = metadata_bytes;
            entry.candidate = Some(meta);
            entry.revision += 1;
            if blockers.is_empty() || can_preempt {
                entry.location = CoordinatorLocation::ReadyToCommit;
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
                entry.location = CoordinatorLocation::WaitingConflict {
                    blockers: blockers.clone(),
                };
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

    /// Park a verified entry on conflicts owned by the accepted `TxPool`.
    /// Speculative ranking metadata is retained, but active claims are
    /// relinquished until every accepted input is reported free.
    pub(crate) fn wait_for_pool_inputs(
        &mut self,
        hash: &Byte32,
        version: CoordinatorVersion,
        inputs: HashSet<OutPoint>,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        if inputs.is_empty() || inputs.len() > self.limits.max_pool_inputs_per_entry {
            return Err(CoordinatorError::PoolInputLimitExceeded);
        }
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
        if entry.phase.kind() != PayloadPhase::Verified {
            return Err(CoordinatorError::PhaseMismatch {
                expected: PayloadPhase::Verified,
                actual: entry.phase.kind(),
            });
        }
        if !matches!(
            entry.location,
            CoordinatorLocation::ReadyToCommit
                | CoordinatorLocation::WaitingConflict { .. }
                | CoordinatorLocation::ConflictRecheck
        ) {
            return Err(CoordinatorError::LocationMismatch {
                expected: CoordinatorLocation::ReadyToCommit,
                actual: entry.location.clone(),
            });
        }
        self.ensure_revision_capacity(hash)?;
        let next_edges = self
            .pool_input_edge_count
            .checked_add(inputs.len())
            .ok_or(CoordinatorError::PoolInputEdgeLimitExceeded)?;
        if next_edges > self.limits.max_pool_input_edges {
            return Err(CoordinatorError::PoolInputEdgeLimitExceeded);
        }
        for input in &inputs {
            if self
                .pool_waiters_by_input
                .get(input)
                .map_or(0, HashSet::len)
                .saturating_add(1)
                > self.limits.max_pool_waiters_per_input
            {
                return Err(CoordinatorError::PoolInputWaiterLimitExceeded(
                    input.clone(),
                ));
            }
        }
        let (payload_bytes, metadata_bytes) = self
            .entries
            .get(hash)
            .map(|entry| (entry.payload_bytes, entry.metadata_bytes))
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        let added_metadata = inputs
            .len()
            .checked_mul(self.limits.metadata_cost.pool_input_edge_bytes)
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        let next_metadata_bytes = metadata_bytes
            .checked_add(added_metadata)
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        let next_charge_bytes = payload_bytes
            .checked_add(next_metadata_bytes)
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        self.check_recharge(hash, next_charge_bytes)?;
        self.preflight_deactivate_conflict_indexes(hash)?;
        let mut undo = vec![hash.clone()];
        if let Some(waiters) = self.waiters_by_blocker.get(hash) {
            undo.extend(waiters.iter().cloned());
        }
        self.with_entry_undo(&undo, |coordinator| {
            coordinator.apply_recharge(hash, next_charge_bytes)?;
            coordinator.remove_current_queue_ticket(hash)?;
            coordinator.deactivate_conflict_indexes(hash)?;
            coordinator.apply_fault_checkpoint();
            for input in &inputs {
                coordinator
                    .pool_waiters_by_input
                    .entry(input.clone())
                    .or_default()
                    .insert(hash.clone());
            }
            coordinator.pool_input_edge_count = next_edges;
            let entry = coordinator
                .entries
                .get_mut(hash)
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            entry.location = CoordinatorLocation::WaitingPoolInputs { inputs };
            entry.metadata_bytes = next_metadata_bytes;
            entry.revision += 1;
            let version = entry.version();
            coordinator.apply_fault_checkpoint();
            Ok(version)
        })
    }

    /// Consume at most `max` waiters for one accepted input. A candidate that
    /// becomes unblocked enters the bounded conflict-recheck deque and reuses
    /// its verified proof instead of re-verifying.
    pub(crate) fn pool_input_freed(
        &mut self,
        input: &OutPoint,
        max: usize,
    ) -> Result<Vec<Byte32>, CoordinatorError> {
        let mut affected: Vec<_> = self
            .pool_waiters_by_input
            .get(input)
            .into_iter()
            .flat_map(|waiters| waiters.iter().cloned())
            .collect();
        affected.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        affected.truncate(max);

        let mut ready_priority = 0usize;
        let mut ready_normal = 0usize;
        let mut recheck_count = 0usize;
        let mut priority_owners = Vec::new();
        let mut normal_owners = Vec::new();
        for hash in &affected {
            let entry = self
                .entries
                .get(hash)
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            let CoordinatorLocation::WaitingPoolInputs { inputs } = &entry.location else {
                return Err(CoordinatorError::PoolInputEdgeLimitExceeded);
            };
            if !inputs.contains(input) {
                return Err(CoordinatorError::PoolInputEdgeLimitExceeded);
            }
            self.ensure_revision_capacity(hash)?;
            let next_metadata_bytes = entry
                .metadata_bytes
                .checked_sub(self.limits.metadata_cost.pool_input_edge_bytes)
                .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
            let next_charge_bytes = entry
                .payload_bytes
                .checked_add(next_metadata_bytes)
                .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
            self.check_recharge(hash, next_charge_bytes)?;
            if inputs.len() == 1 {
                if entry.candidate.is_some() {
                    recheck_count = recheck_count.saturating_add(1);
                } else if entry.source.is_proposal() {
                    ready_priority = ready_priority.saturating_add(1);
                    priority_owners.push(entry.source.queue_owner());
                } else {
                    ready_normal = ready_normal.saturating_add(1);
                    normal_owners.push(entry.source.queue_owner());
                }
            }
        }
        let queue = self.queue_mut(QueueKind::Commit)?;
        queue.reserve_many(true, priority_owners, ready_priority)?;
        queue.reserve_many(false, normal_owners, ready_normal)?;
        self.conflict_rechecks
            .try_reserve(recheck_count)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.conflict_recheck_set
            .try_reserve(recheck_count)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        if self.pool_input_edge_count < affected.len() {
            return Err(CoordinatorError::PoolInputEdgeLimitExceeded);
        }

        let undo = affected.clone();
        let result = affected.clone();
        self.with_entry_undo(&undo, |coordinator| {
            for hash in &affected {
                let (next_metadata_bytes, next_charge_bytes) = coordinator
                    .entries
                    .get(hash)
                    .map(|entry| {
                        let metadata = entry
                            .metadata_bytes
                            .checked_sub(coordinator.limits.metadata_cost.pool_input_edge_bytes)?;
                        Some((metadata, entry.payload_bytes.checked_add(metadata)?))
                    })
                    .flatten()
                    .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
                coordinator.apply_recharge(hash, next_charge_bytes)?;
                let (ticket, priority, recheck) = {
                    let entry = coordinator
                        .entries
                        .get_mut(hash)
                        .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
                    let CoordinatorLocation::WaitingPoolInputs { inputs } = &mut entry.location
                    else {
                        return Err(CoordinatorError::PoolInputEdgeLimitExceeded);
                    };
                    inputs.remove(input);
                    entry.metadata_bytes = next_metadata_bytes;
                    entry.revision += 1;
                    if inputs.is_empty() {
                        if entry.candidate.is_some() {
                            entry.location = CoordinatorLocation::ConflictRecheck;
                            (None, false, true)
                        } else {
                            entry.location = CoordinatorLocation::ReadyToCommit;
                            (Some(entry.ticket(hash)), entry.source.is_proposal(), false)
                        }
                    } else {
                        (None, false, false)
                    }
                };
                coordinator.pool_input_edge_count -= 1;
                if let Some(waiters) = coordinator.pool_waiters_by_input.get_mut(input) {
                    waiters.remove(hash);
                }
                if recheck && coordinator.conflict_recheck_set.insert(hash.clone()) {
                    coordinator.conflict_rechecks.push_back(hash.clone());
                }
                if let Some(ticket) = ticket {
                    coordinator.queue_mut(QueueKind::Commit)?.push_reserved(
                        QueueKind::Commit,
                        ticket,
                        priority,
                    )?;
                }
                coordinator.apply_fault_checkpoint();
            }
            if coordinator
                .pool_waiters_by_input
                .get(input)
                .is_some_and(HashSet::is_empty)
            {
                coordinator.pool_waiters_by_input.remove(input);
            }
            Ok(result)
        })
    }

    pub(crate) fn begin_next_commit(&mut self) -> Result<Option<CommitLease<V>>, CoordinatorError> {
        let Some(ticket) = self.peek_live_ticket(QueueKind::Commit)? else {
            return Ok(None);
        };
        self.validate_version_location_phase(
            &ticket.hash,
            ticket.version,
            &CoordinatorLocation::ReadyToCommit,
            PayloadPhase::Verified,
        )?;
        self.ensure_revision_capacity(&ticket.hash)?;
        let source = self
            .entries
            .get(&ticket.hash)
            .map(|entry| entry.source)
            .ok_or_else(|| CoordinatorError::Missing(ticket.hash.clone()))?;
        self.check_activate_source(source)?;
        self.consume_front_ticket(QueueKind::Commit, &ticket)?;
        self.activate_source(source)?;
        let entry = self
            .entries
            .get_mut(&ticket.hash)
            .ok_or_else(|| CoordinatorError::Missing(ticket.hash.clone()))?;
        entry.location = CoordinatorLocation::Committing;
        entry.revision += 1;
        let ResidentPhase::Verified(payload) = &entry.phase else {
            return Err(CoordinatorError::PhaseMismatch {
                expected: PayloadPhase::Verified,
                actual: entry.phase.kind(),
            });
        };
        Ok(Some(CommitLease {
            hash: ticket.hash,
            version: entry.version(),
            payload: Arc::clone(payload),
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
                .and_then(|entry| entry.candidate.as_ref())
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
        self.deactivate_source(source)?;
        let entry = self
            .entries
            .get_mut(&lease.hash)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        entry.location = CoordinatorLocation::ReadyToCommit;
        entry.revision += 1;
        let version = entry.version();
        let ticket = entry.ticket(&lease.hash);
        let front = entry.source.is_proposal();
        self.queue_mut(QueueKind::Commit)?
            .push_reserved(QueueKind::Commit, ticket, front)?;
        Ok(version)
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
            .is_some_and(|entry| entry.candidate.is_some())
        {
            return Err(CoordinatorError::ConflictInvariant);
        }
        let entry = self.remove_present(&lease.hash)?;
        let ResidentPhase::Verified(verified) = entry.phase else {
            return Err(CoordinatorError::PhaseMismatch {
                expected: PayloadPhase::Verified,
                actual: entry.phase.kind(),
            });
        };
        Ok(CommitHandoff {
            hash: lease.hash.clone(),
            short_id: entry.short_id,
            raw: entry.raw,
            verified,
            peer: entry.source.peer(),
            source: entry.source,
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
            .and_then(|entry| entry.candidate.as_ref())
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
                .candidate
                .as_ref()
                .ok_or(CoordinatorError::ConflictInvariant)?;
            if candidate.inputs.is_disjoint(&winner_inputs)
                || matches!(
                    entry.location,
                    CoordinatorLocation::ReadyToCommit | CoordinatorLocation::Committing
                )
            {
                return Err(CoordinatorError::ConflictInvariant);
            }
            self.preflight_remove_conflict_indexes(hash)?;
            self.preflight_remove_pool_input_indexes(hash)?;
        }
        self.preflight_remove_conflict_indexes(&lease.hash)?;
        self.preflight_remove_pool_input_indexes(&lease.hash)?;

        let mut rejected: Vec<_> = rejected.into_iter().collect();
        rejected.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        let mut terminal = Vec::new();
        terminal
            .try_reserve(rejected.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        let mut undo = rejected.clone();
        undo.push(lease.hash.clone());
        let directly_affected = undo.clone();
        for hash in &directly_affected {
            if let Some(waiters) = self.waiters_by_blocker.get(hash) {
                undo.extend(waiters.iter().cloned());
            }
        }
        self.with_entry_undo(&undo, |coordinator| {
            for hash in rejected {
                let entry = coordinator.remove_present_apply(&hash)?;
                terminal.push(Self::terminal_record(
                    hash,
                    entry,
                    TerminalDisposition::Rejected,
                ));
                coordinator.apply_fault_checkpoint();
            }
            let entry = coordinator.remove_present_apply(&lease.hash)?;
            coordinator.apply_fault_checkpoint();
            let ResidentPhase::Verified(verified) = entry.phase else {
                return Err(CoordinatorError::PhaseMismatch {
                    expected: PayloadPhase::Verified,
                    actual: entry.phase.kind(),
                });
            };
            Ok(ConflictCommitHandoff {
                winner: CommitHandoff {
                    hash: lease.hash.clone(),
                    short_id: entry.short_id,
                    raw: entry.raw,
                    verified,
                    peer: entry.source.peer(),
                    source: entry.source,
                },
                rejected: terminal,
            })
        })
    }

    pub(crate) fn force_terminalize(
        &mut self,
        hash: &Byte32,
        disposition: TerminalDisposition,
    ) -> Result<Option<TerminalRecord<R, U, V>>, CoordinatorError> {
        if !self.entries.contains_key(hash) {
            return Ok(None);
        }
        if self
            .entries
            .get(hash)
            .is_some_and(|entry| entry.location == CoordinatorLocation::Committing)
        {
            return Err(CoordinatorError::CommitInProgress(hash.clone()));
        }
        let entry = self.remove_present(hash)?;
        Ok(Some(Self::terminal_record(
            hash.clone(),
            entry,
            disposition,
        )))
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
            if entry.location == CoordinatorLocation::Committing {
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
        let mut affected: Vec<_> = selected.iter().map(|ticket| ticket.hash.clone()).collect();
        let directly_affected = affected.clone();
        for hash in &directly_affected {
            if let Some(waiters) = self.waiters_by_blocker.get(hash) {
                affected.extend(waiters.iter().cloned());
            }
        }
        let mut terminal = Vec::new();
        terminal
            .try_reserve(selected.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.with_entry_undo(&affected, |coordinator| {
            for ticket in selected {
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
        self.pool_waiters_by_input.clear();
        self.pool_input_edge_count = 0;
        self.deadlines.clear();
        self.live_deadlines.clear();
        for queue in self.queues.values_mut() {
            queue.priority = TicketLane::default();
            queue.normal = TicketLane::default();
            queue.live.clear();
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
        if let Some(kind) = entry.location.queue_kind() {
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
        Ok(queue.peek_eligible(|owner| match owner {
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
        let Some(kind) = entry.location.queue_kind() else {
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
            self.entries.get(*parent).is_some_and(|parent_entry| {
                matches!(
                    parent_entry.location,
                    CoordinatorLocation::Invalidated { .. }
                )
            })
        }) {
            return Err(CoordinatorError::DependencyInvalidated {
                child: hash.clone(),
                parent: parent.clone(),
            });
        }
        if entry.location != *expected_location {
            return Err(CoordinatorError::LocationMismatch {
                expected: expected_location.clone(),
                actual: entry.location.clone(),
            });
        }
        let actual_phase = entry.phase.kind();
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
        pool_inputs: usize,
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
        bytes
            .checked_add(
                pool_inputs
                    .checked_mul(cost.pool_input_edge_bytes)
                    .ok_or(CoordinatorError::ResidencyChargeOverflow)?,
            )
            .ok_or(CoordinatorError::ResidencyChargeOverflow)
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
                snapshot.push((hash.clone(), entry));
            }
        }
        let next_incarnation = self.next_incarnation;
        let next_arrival = self.next_arrival;
        let outcome = catch_unwind(AssertUnwindSafe(|| apply(self)));
        match outcome {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => {
                self.restore_entry_snapshot(snapshot, next_incarnation, next_arrival)?;
                Err(error)
            }
            Err(payload) => {
                if let Err(error) =
                    self.restore_entry_snapshot(snapshot, next_incarnation, next_arrival)
                {
                    std::panic::panic_any(error);
                }
                resume_unwind(payload)
            }
        }
    }

    fn restore_entry_snapshot(
        &mut self,
        snapshot: Vec<(Byte32, CoordinatorEntry<R, U, V>)>,
        next_incarnation: u64,
        next_arrival: u64,
    ) -> Result<(), CoordinatorError> {
        for (hash, entry) in snapshot {
            self.entries.insert(hash, entry);
        }
        self.next_incarnation = next_incarnation;
        self.next_arrival = next_arrival;
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
            self.entries.get(child).is_some_and(|entry| {
                !matches!(entry.location, CoordinatorLocation::Invalidated { .. })
            })
        });
        for child in &children {
            let entry = self
                .entries
                .get(child)
                .ok_or_else(|| CoordinatorError::Missing(child.clone()))?;
            self.ensure_revision_capacity(child)?;
            if entry.location.uses_active_slot()
                && (self.active_work == 0
                    || entry
                        .source
                        .peer()
                        .is_some_and(|peer| self.peer_active_work(peer) == 0))
            {
                return Err(CoordinatorError::ActiveWorkLimitExceeded);
            }
            self.preflight_remove_conflict_indexes(child)?;
            self.preflight_remove_pool_input_indexes(child)?;
        }
        self.dependency_failures
            .try_reserve(children.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.dependency_failure_set
            .try_reserve(children.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        let mut undo_hashes = children.clone();
        for child in &children {
            if let Some(waiters) = self.waiters_by_blocker.get(child) {
                undo_hashes.extend(waiters.iter().cloned());
            }
        }
        let result = children.clone();
        self.with_entry_undo(&undo_hashes, |coordinator| {
            for child in &children {
                let active_source = coordinator
                    .entries
                    .get(child)
                    .and_then(|entry| entry.location.uses_active_slot().then_some(entry.source));
                if let Some(source) = active_source {
                    coordinator.deactivate_source(source)?;
                }
                coordinator.remove_current_queue_ticket(child)?;
                coordinator.remove_pool_input_indexes(child)?;
                coordinator.remove_conflict_indexes(child)?;
                coordinator.apply_fault_checkpoint();
                let entry = coordinator
                    .entries
                    .get_mut(child)
                    .ok_or_else(|| CoordinatorError::Missing(child.clone()))?;
                entry.location = CoordinatorLocation::Invalidated {
                    cause: cause.clone(),
                };
                entry.revision += 1;
                if coordinator.dependency_failure_set.insert(child.clone()) {
                    coordinator.dependency_failures.push_back(child.clone());
                }
                coordinator.apply_fault_checkpoint();
            }
            Ok(result)
        })
    }

    fn remove_present(
        &mut self,
        hash: &Byte32,
    ) -> Result<CoordinatorEntry<R, U, V>, CoordinatorError> {
        let mut undo = vec![hash.clone()];
        if let Some(waiters) = self.waiters_by_blocker.get(hash) {
            undo.extend(waiters.iter().cloned());
        }
        self.with_entry_undo(&undo, |coordinator| {
            let entry = coordinator.remove_present_apply(hash)?;
            coordinator.apply_fault_checkpoint();
            Ok(entry)
        })
    }

    fn remove_present_apply(
        &mut self,
        hash: &Byte32,
    ) -> Result<CoordinatorEntry<R, U, V>, CoordinatorError> {
        let active_source = self
            .entries
            .get(hash)
            .and_then(|entry| entry.location.uses_active_slot().then_some(entry.source));
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
        self.preflight_remove_pool_input_indexes(hash)?;
        self.remove_current_queue_ticket(hash)?;
        self.remove_pool_input_indexes(hash)?;
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
                    self.entries.get(child).is_some_and(|entry| {
                        !matches!(entry.location, CoordinatorLocation::Invalidated { .. })
                    })
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
        let later_phase = match entry.phase {
            ResidentPhase::Raw => None,
            ResidentPhase::Unverified(payload) => Some(TerminalPhase::Unverified(payload)),
            ResidentPhase::Verified(payload) => Some(TerminalPhase::Verified(payload)),
        };
        TerminalRecord {
            hash,
            short_id: entry.short_id,
            raw: entry.raw,
            later_phase,
            peer: entry.source.peer(),
            source: entry.source,
            disposition,
        }
    }

    fn phase_location_valid(phase: PayloadPhase, location: &CoordinatorLocation) -> bool {
        matches!(
            (phase, location),
            (
                PayloadPhase::Raw,
                CoordinatorLocation::RawQueued(_)
                    | CoordinatorLocation::RawActive(_)
                    | CoordinatorLocation::WaitingParents { .. }
                    | CoordinatorLocation::Invalidated { .. }
            ) | (
                PayloadPhase::Unverified,
                CoordinatorLocation::VerifyQueued
                    | CoordinatorLocation::VerifyActive
                    | CoordinatorLocation::Invalidated { .. }
            ) | (
                PayloadPhase::Verified,
                CoordinatorLocation::ReadyToCommit
                    | CoordinatorLocation::WaitingPoolInputs { .. }
                    | CoordinatorLocation::WaitingConflict { .. }
                    | CoordinatorLocation::ConflictRecheck
                    | CoordinatorLocation::Committing
                    | CoordinatorLocation::Invalidated { .. }
            )
        )
    }
}
