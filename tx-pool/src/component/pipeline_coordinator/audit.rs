use super::*;

impl<R, U, V> PipelineCoordinator<R, U, V> {
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

        for (hash, entry) in &self.entries {
            if !Self::phase_location_valid(entry.phase.kind(), &entry.location) {
                return Err(CoordinatorAuditError::InvalidPhaseLocation(hash.clone()));
            }
            let conflict_inputs = entry.candidate.as_ref().map_or(0, |meta| meta.inputs.len());
            let pool_inputs = match &entry.location {
                CoordinatorLocation::WaitingPoolInputs { inputs } => inputs.len(),
                _ => 0,
            };
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
                        .raw_payload_bytes
                        .checked_add(base_metadata)
                        .ok_or(CoordinatorAuditError::MetadataCharge)?
                || entry.charge_bytes
                    != entry
                        .payload_bytes
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
            if entry.location.uses_active_slot() {
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
            if let Some(expires_at) = entry.expires_at {
                live_deadlines.insert(
                    hash.clone(),
                    DeadlineTicket {
                        expires_at,
                        hash: hash.clone(),
                        incarnation: entry.incarnation,
                    },
                );
            }
            for parent in &entry.dependencies {
                by_parent
                    .entry(parent.clone())
                    .or_default()
                    .insert(hash.clone());
            }
            if let Some(kind) = entry.location.queue_kind() {
                let ticket = entry.ticket(hash);
                expected_live
                    .entry(kind)
                    .or_default()
                    .insert(ticket.clone());
                if entry.source.is_proposal() {
                    expected_priority.entry(kind).or_default().insert(ticket);
                }
            }
            if let CoordinatorLocation::WaitingPoolInputs { inputs } = &entry.location {
                if inputs.is_empty() || entry.phase.kind() != PayloadPhase::Verified {
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
            if let Some(candidate) = &entry.candidate {
                if entry.phase.kind() != PayloadPhase::Verified {
                    return Err(CoordinatorAuditError::InvalidPhaseLocation(hash.clone()));
                }
                conflict_edges = conflict_edges
                    .checked_add(candidate.inputs.len())
                    .ok_or(CoordinatorAuditError::ConflictEdgeCount)?;
                for input in &candidate.inputs {
                    candidates_by_input
                        .entry(input.clone())
                        .or_default()
                        .insert(hash.clone());
                }
                match &entry.location {
                    CoordinatorLocation::ReadyToCommit | CoordinatorLocation::Committing => {
                        for input in &candidate.inputs {
                            if active_by_input
                                .insert(input.clone(), hash.clone())
                                .is_some()
                            {
                                return Err(CoordinatorAuditError::ConflictActiveIndex);
                            }
                        }
                    }
                    CoordinatorLocation::WaitingConflict { blockers } => {
                        if blockers.is_empty() {
                            return Err(CoordinatorAuditError::ConflictWaiterIndex);
                        }
                        for blocker in blockers {
                            let Some(blocker_entry) = self.entries.get(blocker) else {
                                return Err(CoordinatorAuditError::ConflictWaiterIndex);
                            };
                            if !matches!(
                                blocker_entry.location,
                                CoordinatorLocation::ReadyToCommit
                                    | CoordinatorLocation::Committing
                            ) || blocker_entry.candidate.as_ref().is_none_or(
                                |blocker_candidate| {
                                    candidate.inputs.is_disjoint(&blocker_candidate.inputs)
                                },
                            ) {
                                return Err(CoordinatorAuditError::ConflictWaiterIndex);
                            }
                            waiters_by_blocker
                                .entry(blocker.clone())
                                .or_default()
                                .insert(hash.clone());
                        }
                    }
                    CoordinatorLocation::ConflictRecheck => {
                        conflict_rechecks.insert(hash.clone());
                    }
                    CoordinatorLocation::WaitingPoolInputs { .. } => {}
                    _ => return Err(CoordinatorAuditError::InvalidPhaseLocation(hash.clone())),
                }
            } else if matches!(
                entry.location,
                CoordinatorLocation::WaitingConflict { .. } | CoordinatorLocation::ConflictRecheck
            ) {
                return Err(CoordinatorAuditError::ConflictCandidateIndex);
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
        let mut physical_rechecks = HashMap::new();
        for hash in &self.conflict_rechecks {
            if self.conflict_recheck_set.contains(hash) {
                *physical_rechecks.entry(hash).or_insert(0usize) += 1;
            }
        }
        if self
            .conflict_recheck_set
            .iter()
            .any(|hash| physical_rechecks.get(hash) != Some(&1))
        {
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
