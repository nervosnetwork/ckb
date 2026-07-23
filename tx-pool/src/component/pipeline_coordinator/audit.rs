use super::*;

impl<R, U, V> PipelineCoordinator<R, U, V> {
    pub(super) fn rebuild_derived_indexes(&mut self) -> Result<(), CoordinatorError> {
        self.by_short_id.clear();
        self.by_peer.clear();
        self.by_parent.clear();
        self.candidates_by_input.clear();
        self.active_by_input.clear();
        self.waiters_by_blocker.clear();
        self.conflict_recheck_set.clear();
        self.conflict_edge_count = 0;
        self.pool_waiters_by_input.clear();
        self.pool_input_edge_count = 0;
        self.live_deadlines.clear();
        self.dependency_failure_set.clear();
        self.global_usage = CoordinatorResidency::default();
        self.peer_usage.clear();
        self.active_work = 0;
        self.active_work_by_peer.clear();
        let mut expected_queues: HashMap<QueueKind, Vec<CoordinatorTicket>> = HashMap::new();
        let mut maintenance_sequences = HashSet::new();
        let mut queue_sequences = HashSet::new();
        let mut conflict_recheck_order = Vec::new();
        let mut dependency_failure_order = Vec::new();

        for (hash, entry) in &self.entries {
            if !entry.state_shape_valid(hash, &self.limits) {
                return Err(CoordinatorError::ConflictInvariant);
            }
            if let Some(sequence) = entry.maintenance_sequence()
                && (sequence >= self.next_maintenance_sequence
                    || !maintenance_sequences.insert(sequence))
            {
                return Err(CoordinatorError::ConflictInvariant);
            }
            if entry.queue_sequence >= self.next_queue_sequence
                || !queue_sequences.insert(entry.queue_sequence)
            {
                return Err(CoordinatorError::ConflictInvariant);
            }
            if self
                .by_short_id
                .insert(entry.short_id.clone(), hash.clone())
                .is_some()
            {
                return Err(CoordinatorError::ConflictInvariant);
            }
            let charge = CoordinatorResidency::new(1, entry.charge_bytes);
            self.global_usage = self
                .global_usage
                .checked_add(charge)
                .ok_or(CoordinatorError::GlobalBudgetExceeded)?;
            if let Some(peer) = entry.source.peer() {
                let usage = self.peer_usage.entry(peer).or_default();
                *usage = usage
                    .checked_add(charge)
                    .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
                self.by_peer.entry(peer).or_default().insert(hash.clone());
            }
            if entry.uses_active_slot() {
                self.active_work = self
                    .active_work
                    .checked_add(1)
                    .ok_or(CoordinatorError::ActiveWorkLimitExceeded)?;
                if let Some(peer) = entry.source.peer() {
                    let active = self.active_work_by_peer.entry(peer).or_default();
                    *active = active
                        .checked_add(1)
                        .ok_or(CoordinatorError::PeerActiveWorkLimitExceeded(peer))?;
                }
            }
            for parent in &entry.dependencies {
                self.by_parent
                    .entry(parent.clone())
                    .or_default()
                    .insert(hash.clone());
            }
            if let Some(kind) = entry.queue_kind() {
                expected_queues
                    .entry(kind)
                    .or_default()
                    .push(entry.ticket(hash));
            }
            if let Some(expires_at) = entry.expires_at.filter(|_| !entry.is_committing()) {
                self.live_deadlines.insert(
                    hash.clone(),
                    DeadlineTicket {
                        expires_at,
                        hash: hash.clone(),
                        incarnation: entry.incarnation,
                        generation: entry.deadline_generation,
                    },
                );
            }
            if let Some(inputs) = entry.waiting_pool_inputs() {
                self.pool_input_edge_count = self
                    .pool_input_edge_count
                    .checked_add(inputs.len())
                    .ok_or(CoordinatorError::PoolInputEdgeLimitExceeded)?;
                for input in inputs {
                    self.pool_waiters_by_input
                        .entry(input.clone())
                        .or_default()
                        .insert(hash.clone());
                }
            }
            if entry.invalidated_cause().is_some() {
                self.dependency_failure_set.insert(hash.clone());
                dependency_failure_order.push((
                    entry
                        .maintenance_sequence()
                        .ok_or(CoordinatorError::ConflictInvariant)?,
                    hash.clone(),
                ));
                continue;
            }
            if let EntryState::CandidateVerified {
                candidate,
                location,
                ..
            } = &entry.state
            {
                self.conflict_edge_count = self
                    .conflict_edge_count
                    .checked_add(candidate.inputs.len())
                    .ok_or(CoordinatorError::ConflictEdgeLimitExceeded)?;
                for input in &candidate.inputs {
                    self.candidates_by_input
                        .entry(input.clone())
                        .or_default()
                        .insert(hash.clone());
                }
                match location {
                    CandidateLocation::Ready | CandidateLocation::Committing => {
                        for input in &candidate.inputs {
                            if self
                                .active_by_input
                                .insert(input.clone(), hash.clone())
                                .is_some()
                            {
                                return Err(CoordinatorError::ConflictInvariant);
                            }
                        }
                    }
                    CandidateLocation::WaitingConflict { blockers } => {
                        for blocker in blockers {
                            self.waiters_by_blocker
                                .entry(blocker.clone())
                                .or_default()
                                .insert(hash.clone());
                        }
                    }
                    CandidateLocation::Recheck { sequence } => {
                        self.conflict_recheck_set.insert(hash.clone());
                        conflict_recheck_order.push((*sequence, hash.clone()));
                    }
                    CandidateLocation::WaitingPoolInputs { .. } => {}
                }
            }
        }

        for kind in [
            QueueKind::PreCheck,
            QueueKind::Resolve,
            QueueKind::Verify,
            QueueKind::Commit,
        ] {
            let tickets = expected_queues.remove(&kind).unwrap_or_default();
            let expected_ordering = match kind {
                QueueKind::Verify => match self.limits.verify_ordering {
                    CoordinatorVerifyOrdering::ArrivalTime => QueueOrdering::Fifo,
                    CoordinatorVerifyOrdering::FeeRate => QueueOrdering::FeeRate,
                },
                QueueKind::PreCheck | QueueKind::Resolve | QueueKind::Commit => QueueOrdering::Fifo,
            };
            let queue = self.queue_mut(kind)?;
            if queue.ordering() != expected_ordering {
                return Err(CoordinatorError::QueueInvariant(kind));
            }
            queue.rebuild_live(kind, tickets)?;
        }
        self.deadlines.retain(|Reverse(ticket)| {
            self.live_deadlines
                .get(&ticket.hash)
                .is_some_and(|live| live == ticket)
        });
        for ticket in self.live_deadlines.values() {
            if !self
                .deadlines
                .iter()
                .any(|Reverse(physical)| physical == ticket)
            {
                self.deadlines.push(Reverse(ticket.clone()));
            }
        }
        conflict_recheck_order.sort_by_key(|(sequence, _)| *sequence);
        self.conflict_rechecks = conflict_recheck_order
            .into_iter()
            .map(|(_, hash)| hash)
            .collect();
        dependency_failure_order.sort_by_key(|(sequence, _)| *sequence);
        self.dependency_failures = dependency_failure_order
            .into_iter()
            .map(|(_, hash)| hash)
            .collect();
        Ok(())
    }

    pub(crate) fn audit(&self) -> Result<(), CoordinatorAuditError> {
        let mut global_usage = CoordinatorResidency::default();
        let mut peer_usage: HashMap<PeerIndex, CoordinatorResidency> = HashMap::new();
        let mut by_short_id = HashMap::new();
        let mut by_peer: HashMap<PeerIndex, HashSet<Byte32>> = HashMap::new();
        let mut by_parent: HashMap<Byte32, HashSet<Byte32>> = HashMap::new();
        let mut expected_live: HashMap<QueueKind, HashSet<CoordinatorTicket>> = HashMap::new();
        let mut expected_priority: HashMap<QueueKind, HashSet<CoordinatorTicket>> = HashMap::new();
        let mut conflict_edges = 0usize;
        let mut candidates_by_input: HashMap<OutPoint, HashSet<Byte32>> = HashMap::new();
        let mut active_by_input: HashMap<OutPoint, Byte32> = HashMap::new();
        let mut waiters_by_blocker: HashMap<Byte32, HashSet<Byte32>> = HashMap::new();
        let mut conflict_rechecks = HashSet::new();
        let mut live_deadlines = HashMap::new();
        let mut pool_waiters_by_input: HashMap<OutPoint, HashSet<Byte32>> = HashMap::new();
        let mut pool_input_edges = 0usize;
        let mut active_work = 0usize;
        let mut active_work_by_peer: HashMap<PeerIndex, usize> = HashMap::new();
        let mut dependency_failures = HashSet::new();
        let mut maintenance_sequences = HashSet::new();
        let mut queue_sequences = HashSet::new();
        let mut expected_conflict_recheck_order = Vec::new();
        let mut expected_dependency_failure_order = Vec::new();

        for (hash, entry) in &self.entries {
            if !entry.state_shape_valid(hash, &self.limits) {
                return Err(CoordinatorAuditError::StateInvariant(hash.clone()));
            }
            if let Some(sequence) = entry.maintenance_sequence()
                && (sequence >= self.next_maintenance_sequence
                    || !maintenance_sequences.insert(sequence))
            {
                return Err(CoordinatorAuditError::StateInvariant(hash.clone()));
            }
            if entry.queue_sequence >= self.next_queue_sequence
                || !queue_sequences.insert(entry.queue_sequence)
            {
                return Err(CoordinatorAuditError::StateInvariant(hash.clone()));
            }
            let conflict_inputs = entry.candidate().map_or(0, |meta| meta.inputs.len());
            let pool_inputs = entry.waiting_pool_inputs().map_or(0, HashSet::len);
            let base_metadata = self
                .metadata_charge_bytes(entry.dependencies.len(), entry.expires_at.is_some(), 0, 0)
                .map_err(|_| CoordinatorAuditError::MetadataCharge)?;
            let metadata = self
                .metadata_charge_bytes(
                    entry.dependencies.len(),
                    entry.expires_at.is_some(),
                    conflict_inputs,
                    pool_inputs,
                )
                .map_err(|_| CoordinatorAuditError::MetadataCharge)?;
            if entry.base_metadata_bytes != base_metadata
                || entry.metadata_bytes != metadata
                || entry.raw_charge_bytes
                    != entry
                        .raw_resident_payload_bytes
                        .checked_add(base_metadata)
                        .ok_or(CoordinatorAuditError::MetadataCharge)?
                || entry.charge_bytes
                    != entry
                        .resident_payload_bytes
                        .checked_add(metadata)
                        .ok_or(CoordinatorAuditError::MetadataCharge)?
            {
                return Err(CoordinatorAuditError::MetadataCharge);
            }
            let charge = CoordinatorResidency::new(1, entry.charge_bytes);
            global_usage = global_usage
                .checked_add(charge)
                .ok_or(CoordinatorAuditError::GlobalUsage)?;
            if by_short_id
                .insert(entry.short_id.clone(), hash.clone())
                .is_some()
            {
                return Err(CoordinatorAuditError::ShortIdIndex);
            }
            if let Some(peer) = entry.source.peer() {
                let usage = peer_usage.entry(peer).or_default();
                *usage = usage
                    .checked_add(charge)
                    .ok_or(CoordinatorAuditError::PeerUsage)?;
                by_peer.entry(peer).or_default().insert(hash.clone());
            }
            if entry.uses_active_slot() {
                active_work = active_work
                    .checked_add(1)
                    .ok_or(CoordinatorAuditError::ActiveWork)?;
                if let Some(peer) = entry.source.peer() {
                    let peer_active = active_work_by_peer.entry(peer).or_default();
                    *peer_active = peer_active
                        .checked_add(1)
                        .ok_or(CoordinatorAuditError::ActiveWork)?;
                }
            }
            if let Some(expires_at) = entry.expires_at.filter(|_| !entry.is_committing()) {
                live_deadlines.insert(
                    hash.clone(),
                    DeadlineTicket {
                        expires_at,
                        hash: hash.clone(),
                        incarnation: entry.incarnation,
                        generation: entry.deadline_generation,
                    },
                );
            }
            for parent in &entry.dependencies {
                by_parent
                    .entry(parent.clone())
                    .or_default()
                    .insert(hash.clone());
            }
            if let Some(kind) = entry.queue_kind() {
                let ticket = entry.ticket(hash);
                expected_live
                    .entry(kind)
                    .or_default()
                    .insert(ticket.clone());
                if entry.source.is_proposal() {
                    expected_priority.entry(kind).or_default().insert(ticket);
                }
            }
            if let Some(inputs) = entry.waiting_pool_inputs() {
                if inputs.is_empty() {
                    return Err(CoordinatorAuditError::PoolInputIndex);
                }
                pool_input_edges = pool_input_edges
                    .checked_add(inputs.len())
                    .ok_or(CoordinatorAuditError::PoolInputEdgeCount)?;
                for input in inputs {
                    pool_waiters_by_input
                        .entry(input.clone())
                        .or_default()
                        .insert(hash.clone());
                }
            }
            if entry.invalidated_cause().is_some() {
                dependency_failures.insert(hash.clone());
                expected_dependency_failure_order.push((
                    entry
                        .maintenance_sequence()
                        .ok_or_else(|| CoordinatorAuditError::StateInvariant(hash.clone()))?,
                    hash.clone(),
                ));
            }
            if let EntryState::CandidateVerified {
                candidate,
                location,
                ..
            } = &entry.state
            {
                conflict_edges = conflict_edges
                    .checked_add(candidate.inputs.len())
                    .ok_or(CoordinatorAuditError::ConflictEdgeCount)?;
                for input in &candidate.inputs {
                    candidates_by_input
                        .entry(input.clone())
                        .or_default()
                        .insert(hash.clone());
                }
                match location {
                    CandidateLocation::Ready | CandidateLocation::Committing => {
                        for input in &candidate.inputs {
                            if active_by_input
                                .insert(input.clone(), hash.clone())
                                .is_some()
                            {
                                return Err(CoordinatorAuditError::ConflictActiveIndex);
                            }
                        }
                    }
                    CandidateLocation::WaitingConflict { blockers } => {
                        if blockers.is_empty() {
                            return Err(CoordinatorAuditError::ConflictWaiterIndex);
                        }
                        for blocker in blockers {
                            let Some(blocker_entry) = self.entries.get(blocker) else {
                                return Err(CoordinatorAuditError::ConflictWaiterIndex);
                            };
                            if !matches!(
                                &blocker_entry.state,
                                EntryState::CandidateVerified {
                                    candidate: blocker_candidate,
                                    location: CandidateLocation::Ready
                                        | CandidateLocation::Committing,
                                    ..
                                } if !candidate.inputs.is_disjoint(&blocker_candidate.inputs)
                            ) {
                                return Err(CoordinatorAuditError::ConflictWaiterIndex);
                            }
                            waiters_by_blocker
                                .entry(blocker.clone())
                                .or_default()
                                .insert(hash.clone());
                        }
                    }
                    CandidateLocation::Recheck { sequence } => {
                        conflict_rechecks.insert(hash.clone());
                        expected_conflict_recheck_order.push((*sequence, hash.clone()));
                    }
                    CandidateLocation::WaitingPoolInputs { .. } => {}
                }
            }
        }

        if global_usage != self.global_usage {
            return Err(CoordinatorAuditError::GlobalUsage);
        }
        if !global_usage.fits(self.limits.global)
            || peer_usage
                .values()
                .any(|usage| self.limits.per_peer.is_some_and(|limit| !usage.fits(limit)))
        {
            return Err(CoordinatorAuditError::BudgetExceeded);
        }
        if peer_usage != self.peer_usage {
            return Err(CoordinatorAuditError::PeerUsage);
        }
        if active_work != self.active_work
            || active_work_by_peer != self.active_work_by_peer
            || active_work > self.limits.max_active_work
            || active_work_by_peer
                .values()
                .any(|active| *active > self.limits.max_active_work_per_peer)
        {
            return Err(CoordinatorAuditError::ActiveWork);
        }
        if by_short_id != self.by_short_id {
            return Err(CoordinatorAuditError::ShortIdIndex);
        }
        if by_peer != self.by_peer {
            return Err(CoordinatorAuditError::PeerIndex);
        }
        if by_parent != self.by_parent {
            return Err(CoordinatorAuditError::ParentIndex);
        }
        if conflict_edges != self.conflict_edge_count {
            return Err(CoordinatorAuditError::ConflictEdgeCount);
        }
        if candidates_by_input != self.candidates_by_input {
            return Err(CoordinatorAuditError::ConflictCandidateIndex);
        }
        if active_by_input != self.active_by_input {
            return Err(CoordinatorAuditError::ConflictActiveIndex);
        }
        if waiters_by_blocker != self.waiters_by_blocker {
            return Err(CoordinatorAuditError::ConflictWaiterIndex);
        }
        if conflict_rechecks != self.conflict_recheck_set {
            return Err(CoordinatorAuditError::ConflictMaintenanceIndex);
        }
        if dependency_failures != self.dependency_failure_set {
            return Err(CoordinatorAuditError::DependencyMaintenanceIndex);
        }
        expected_dependency_failure_order.sort_by_key(|(sequence, _)| *sequence);
        let expected_dependency_failure_order: Vec<_> = expected_dependency_failure_order
            .into_iter()
            .map(|(_, hash)| hash)
            .collect();
        let physical_dependency_order: Vec<_> = self
            .dependency_failures
            .iter()
            .filter(|hash| self.dependency_failure_set.contains(*hash))
            .cloned()
            .collect();
        if physical_dependency_order != expected_dependency_failure_order {
            return Err(CoordinatorAuditError::DependencyMaintenanceIndex);
        }
        if pool_input_edges != self.pool_input_edge_count {
            return Err(CoordinatorAuditError::PoolInputEdgeCount);
        }
        if pool_waiters_by_input != self.pool_waiters_by_input {
            return Err(CoordinatorAuditError::PoolInputIndex);
        }
        if live_deadlines != self.live_deadlines {
            return Err(CoordinatorAuditError::DeadlineIndex);
        }
        let mut physical_deadline_counts: HashMap<&DeadlineTicket, usize> = HashMap::new();
        for Reverse(ticket) in &self.deadlines {
            if self
                .live_deadlines
                .get(&ticket.hash)
                .is_some_and(|live| live == ticket)
            {
                *physical_deadline_counts.entry(ticket).or_default() += 1;
            }
        }
        if self
            .live_deadlines
            .values()
            .any(|ticket| physical_deadline_counts.get(ticket) != Some(&1))
        {
            return Err(CoordinatorAuditError::DeadlineIndex);
        }
        expected_conflict_recheck_order.sort_by_key(|(sequence, _)| *sequence);
        let expected_conflict_recheck_order: Vec<_> = expected_conflict_recheck_order
            .into_iter()
            .map(|(_, hash)| hash)
            .collect();
        let physical_conflict_recheck_order: Vec<_> = self
            .conflict_rechecks
            .iter()
            .filter(|hash| self.conflict_recheck_set.contains(*hash))
            .cloned()
            .collect();
        if physical_conflict_recheck_order != expected_conflict_recheck_order {
            return Err(CoordinatorAuditError::ConflictMaintenanceIndex);
        }

        for kind in [
            QueueKind::PreCheck,
            QueueKind::Resolve,
            QueueKind::Verify,
            QueueKind::Commit,
        ] {
            let empty = HashSet::new();
            let expected = expected_live.get(&kind).unwrap_or(&empty);
            let Some(queue) = self.queues.get(&kind) else {
                return Err(CoordinatorAuditError::QueueLogicalIndex);
            };
            let expected_ordering = match kind {
                QueueKind::Verify => match self.limits.verify_ordering {
                    CoordinatorVerifyOrdering::ArrivalTime => QueueOrdering::Fifo,
                    CoordinatorVerifyOrdering::FeeRate => QueueOrdering::FeeRate,
                },
                QueueKind::PreCheck | QueueKind::Resolve | QueueKind::Commit => QueueOrdering::Fifo,
            };
            if queue.ordering() != expected_ordering {
                return Err(CoordinatorAuditError::QueueLogicalIndex);
            }
            if &queue.live != expected || !queue.structure_valid() {
                return Err(CoordinatorAuditError::QueueLogicalIndex);
            }
            let mut physical_live_counts: HashMap<&CoordinatorTicket, usize> = HashMap::new();
            let empty_priority = HashSet::new();
            let priority = expected_priority.get(&kind).unwrap_or(&empty_priority);
            for ticket in queue.tickets() {
                if queue.live.contains(ticket) {
                    if ticket.priority != priority.contains(ticket) {
                        return Err(CoordinatorAuditError::QueuePhysicalIndex);
                    }
                    *physical_live_counts.entry(ticket).or_default() += 1;
                }
            }
            if expected
                .iter()
                .any(|ticket| physical_live_counts.get(ticket) != Some(&1))
            {
                return Err(CoordinatorAuditError::QueuePhysicalIndex);
            }
        }
        Ok(())
    }
}
