use super::*;

impl<R, U, V> PipelineCoordinator<R, U, V> {
    /// Rebuild every derived projection from authoritative lifecycle entries.
    /// This is the rollback/recovery path, never the production scheduling hot
    /// path. Conflict relations are reconstructed independently from input
    /// buckets so a failed delta cannot survive an undo boundary.
    pub(crate) fn rebuild_derived_indexes(&mut self) -> Result<(), CoordinatorError> {
        self.by_short_id.clear();
        self.by_peer.clear();
        self.by_parent.clear();
        self.waiting_parent_count = 0;
        self.conflicts.clear();
        self.live_deadlines.clear();
        self.capacity_victim_index.clear();
        self.candidate_victim_index.clear();
        self.dependency_failure_set.clear();
        self.global_usage = CoordinatorResidency::default();
        self.peer_usage.clear();
        self.active_work = 0;
        self.active_work_by_peer.clear();

        let mut expected_queues: HashMap<QueueKind, Vec<CoordinatorTicket>> = HashMap::new();
        let mut maintenance_sequences = HashSet::new();
        let mut queue_sequences = HashSet::new();
        let mut dependency_failure_order = Vec::new();
        let mut candidate_hashes = Vec::new();
        let mut committing = 0usize;

        for (hash, entry) in &self.entries {
            if !entry.state_shape_valid(hash, &self.limits)
                || !self.entry_metadata_charge_is_valid(entry)
            {
                return Err(CoordinatorError::ConflictInvariant);
            }
            if let Some(key) = Self::capacity_victim_key(hash, entry)
                && !self.capacity_victim_index.insert(key)
            {
                return Err(CoordinatorError::ConflictInvariant);
            }
            if let Some(key) = Self::candidate_victim_key(hash, entry)
                && !self.candidate_victim_index.insert(key)
            {
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
                let children = self.by_parent.entry(parent.clone()).or_default();
                children.insert(hash.clone());
                if children.len() > self.limits.max_dependents_per_parent {
                    return Err(CoordinatorError::ParentFanoutLimitExceeded(parent.clone()));
                }
            }
            if matches!(
                &entry.state,
                EntryState::Raw {
                    location: RawLocation::WaitingParents { .. },
                    ..
                }
            ) {
                self.waiting_parent_count = self
                    .waiting_parent_count
                    .checked_add(1)
                    .ok_or(CoordinatorError::ConflictInvariant)?;
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
                committing += usize::from(*location == CandidateLocation::Committing);
                if *location == CandidateLocation::Committing
                    && self.conflicts.committing.replace(hash.clone()).is_some()
                {
                    return Err(CoordinatorError::ConflictInvariant);
                }
                self.conflicts.input_memberships = self
                    .conflicts
                    .input_memberships
                    .checked_add(candidate.inputs.len())
                    .ok_or(CoordinatorError::ConflictEdgeLimitExceeded)?;
                if self.conflicts.input_memberships > self.limits.max_conflict_edges {
                    return Err(CoordinatorError::ConflictEdgeLimitExceeded);
                }
                for input in &candidate.inputs {
                    let candidates = self.conflicts.by_input.entry(input.clone()).or_default();
                    candidates.insert(hash.clone());
                    if candidates.len() > self.limits.max_candidates_per_input {
                        return Err(CoordinatorError::ConflictCandidateLimitExceeded(
                            input.clone(),
                        ));
                    }
                }
                self.conflicts
                    .relations
                    .insert(hash.clone(), CandidateRelation::default());
                candidate_hashes.push(hash.clone());
            }
        }
        if committing > 1
            || !self.global_usage.fits(self.limits.global)
            || self
                .peer_usage
                .values()
                .any(|usage| self.limits.per_peer.is_some_and(|limit| !usage.fits(limit)))
            || self.active_work > self.limits.max_active_work
            || self
                .active_work_by_peer
                .values()
                .any(|active| *active > self.limits.max_active_work_per_peer)
        {
            return Err(CoordinatorError::ConflictInvariant);
        }

        candidate_hashes.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        for hash in &candidate_hashes {
            let inputs = self
                .entries
                .get(hash)
                .and_then(CoordinatorEntry::candidate)
                .map(|candidate| candidate.inputs.clone())
                .ok_or(CoordinatorError::ConflictInvariant)?;
            let mut neighbours: Vec<_> = self
                .bounded_conflicting_candidates(hash, &inputs)?
                .into_iter()
                .filter(|neighbour| hash.as_slice() < neighbour.as_slice())
                .collect();
            neighbours.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
            for neighbour in neighbours {
                let left_rank = self.candidate_rank(hash)?;
                let right_rank = self.candidate_rank(&neighbour)?;
                {
                    let left = self
                        .conflicts
                        .relations
                        .get_mut(hash)
                        .ok_or(CoordinatorError::ConflictInvariant)?;
                    left.degree = left
                        .degree
                        .checked_add(1)
                        .ok_or(CoordinatorError::ConflictInvariant)?;
                    if right_rank > left_rank {
                        left.stronger_count = left
                            .stronger_count
                            .checked_add(1)
                            .ok_or(CoordinatorError::ConflictInvariant)?;
                    }
                }
                {
                    let right = self
                        .conflicts
                        .relations
                        .get_mut(&neighbour)
                        .ok_or(CoordinatorError::ConflictInvariant)?;
                    right.degree = right
                        .degree
                        .checked_add(1)
                        .ok_or(CoordinatorError::ConflictInvariant)?;
                    if left_rank > right_rank {
                        right.stronger_count = right
                            .stronger_count
                            .checked_add(1)
                            .ok_or(CoordinatorError::ConflictInvariant)?;
                    }
                }
            }
        }
        if self
            .conflicts
            .relations
            .values()
            .any(|relation| relation.degree > self.limits.max_candidates_per_input)
        {
            return Err(CoordinatorError::ConflictCohortLimitExceeded);
        }
        for hash in candidate_hashes {
            if self.candidate_is_eligible(&hash) {
                expected_queues.entry(QueueKind::Commit).or_default().push(
                    self.entries
                        .get(&hash)
                        .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?
                        .ticket(&hash),
                );
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
                QueueKind::Commit => QueueOrdering::Candidate,
                QueueKind::PreCheck | QueueKind::Resolve => QueueOrdering::Fifo,
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
        dependency_failure_order.sort_by_key(|(sequence, _)| *sequence);
        self.dependency_failures = dependency_failure_order
            .into_iter()
            .map(|(_, hash)| hash)
            .collect();
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn audit(&self) -> Result<(), CoordinatorAuditError> {
        if self.entry_transaction_depth != 0 || !self.entry_transaction_membership.is_empty() {
            return Err(CoordinatorAuditError::EntryTransactionDepth);
        }
        let mut global_usage = CoordinatorResidency::default();
        let mut peer_usage: HashMap<PeerIndex, CoordinatorResidency> = HashMap::new();
        let mut by_short_id = HashMap::new();
        let mut by_peer: HashMap<PeerIndex, HashSet<Byte32>> = HashMap::new();
        let mut by_parent: HashMap<Byte32, HashSet<Byte32>> = HashMap::new();
        let mut waiting_parent_count = 0usize;
        let mut expected_live: HashMap<QueueKind, HashSet<CoordinatorTicket>> = HashMap::new();
        let mut expected_priority: HashMap<QueueKind, HashSet<CoordinatorTicket>> = HashMap::new();
        let mut input_memberships = 0usize;
        let mut by_input: HashMap<OutPoint, HashSet<Byte32>> = HashMap::new();
        let mut relations: HashMap<Byte32, CandidateRelation> = HashMap::new();
        let mut live_deadlines = HashMap::new();
        let mut active_work = 0usize;
        let mut active_work_by_peer: HashMap<PeerIndex, usize> = HashMap::new();
        let mut dependency_failures = HashSet::new();
        let mut capacity_victim_index = BTreeSet::new();
        let mut candidate_victim_index = BTreeSet::new();
        let mut maintenance_sequences = HashSet::new();
        let mut queue_sequences = HashSet::new();
        let mut expected_dependency_failure_order = Vec::new();
        let mut candidate_hashes = Vec::new();
        let mut committing = 0usize;

        for (hash, entry) in &self.entries {
            if !entry.state_shape_valid(hash, &self.limits) {
                return Err(CoordinatorAuditError::StateInvariant(hash.clone()));
            }
            if let Some(key) = Self::capacity_victim_key(hash, entry)
                && !capacity_victim_index.insert(key)
            {
                return Err(CoordinatorAuditError::StateInvariant(hash.clone()));
            }
            if let Some(key) = Self::candidate_victim_key(hash, entry)
                && !candidate_victim_index.insert(key)
            {
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
            if !self.entry_metadata_charge_is_valid(entry) {
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
                let children = by_parent.entry(parent.clone()).or_default();
                children.insert(hash.clone());
                if children.len() > self.limits.max_dependents_per_parent {
                    return Err(CoordinatorAuditError::ParentIndex);
                }
            }
            if matches!(
                &entry.state,
                EntryState::Raw {
                    location: RawLocation::WaitingParents { .. },
                    ..
                }
            ) {
                waiting_parent_count = waiting_parent_count
                    .checked_add(1)
                    .ok_or(CoordinatorAuditError::WaitingParentCount)?;
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
                committing += usize::from(*location == CandidateLocation::Committing);
                input_memberships = input_memberships
                    .checked_add(candidate.inputs.len())
                    .ok_or(CoordinatorAuditError::ConflictEdgeCount)?;
                for input in &candidate.inputs {
                    let candidates = by_input.entry(input.clone()).or_default();
                    candidates.insert(hash.clone());
                    if candidates.len() > self.limits.max_candidates_per_input {
                        return Err(CoordinatorAuditError::ConflictCandidateIndex);
                    }
                }
                relations.insert(hash.clone(), CandidateRelation::default());
                candidate_hashes.push(hash.clone());
            }
        }
        if committing > 1 || input_memberships > self.limits.max_conflict_edges {
            return Err(CoordinatorAuditError::StateInvariant(
                candidate_hashes.first().cloned().unwrap_or_default(),
            ));
        }
        candidate_hashes.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        for hash in &candidate_hashes {
            let entry = self
                .entries
                .get(hash)
                .ok_or_else(|| CoordinatorAuditError::StateInvariant(hash.clone()))?;
            let candidate = entry
                .candidate()
                .ok_or_else(|| CoordinatorAuditError::StateInvariant(hash.clone()))?;
            let left_rank = match &entry.state {
                EntryState::CandidateVerified { location, .. } => {
                    CandidateRank::from_entry(hash, entry.source, candidate, location)
                }
                _ => return Err(CoordinatorAuditError::StateInvariant(hash.clone())),
            };
            let mut neighbours = HashSet::new();
            for input in &candidate.inputs {
                let bucket = by_input
                    .get(input)
                    .ok_or(CoordinatorAuditError::ConflictCandidateIndex)?;
                neighbours.extend(bucket.iter().filter(|other| *other != hash).cloned());
            }
            if neighbours.len() > self.limits.max_candidates_per_input {
                return Err(CoordinatorAuditError::ConflictCohortIndex);
            }
            for neighbour in neighbours
                .into_iter()
                .filter(|neighbour| hash.as_slice() < neighbour.as_slice())
            {
                let right_entry = self
                    .entries
                    .get(&neighbour)
                    .ok_or_else(|| CoordinatorAuditError::StateInvariant(neighbour.clone()))?;
                let right_candidate = right_entry
                    .candidate()
                    .ok_or_else(|| CoordinatorAuditError::StateInvariant(neighbour.clone()))?;
                let right_location = match &right_entry.state {
                    EntryState::CandidateVerified { location, .. } => location,
                    _ => {
                        return Err(CoordinatorAuditError::StateInvariant(neighbour.clone()));
                    }
                };
                let right_rank = CandidateRank::from_entry(
                    &neighbour,
                    right_entry.source,
                    right_candidate,
                    right_location,
                );
                let left = relations
                    .get_mut(hash)
                    .ok_or(CoordinatorAuditError::ConflictRelationIndex)?;
                left.degree += 1;
                if right_rank > left_rank {
                    left.stronger_count += 1;
                }
                let right = relations
                    .get_mut(&neighbour)
                    .ok_or(CoordinatorAuditError::ConflictRelationIndex)?;
                right.degree += 1;
                if left_rank > right_rank {
                    right.stronger_count += 1;
                }
            }
        }
        for hash in &candidate_hashes {
            let entry = self
                .entries
                .get(hash)
                .ok_or_else(|| CoordinatorAuditError::StateInvariant(hash.clone()))?;
            let relation = relations
                .get(hash)
                .ok_or(CoordinatorAuditError::ConflictRelationIndex)?;
            if relation.degree > self.limits.max_candidates_per_input {
                return Err(CoordinatorAuditError::ConflictCohortIndex);
            }
            if relation.stronger_count == 0
                && matches!(
                    &entry.state,
                    EntryState::CandidateVerified {
                        location: CandidateLocation::Verified,
                        ..
                    }
                )
            {
                let ticket = entry.ticket(hash);
                expected_live
                    .entry(QueueKind::Commit)
                    .or_default()
                    .insert(ticket.clone());
                if entry.source.is_proposal() {
                    expected_priority
                        .entry(QueueKind::Commit)
                        .or_default()
                        .insert(ticket);
                }
            }
        }

        if global_usage != self.global_usage {
            return Err(CoordinatorAuditError::GlobalUsage);
        }
        if capacity_victim_index != self.capacity_victim_index
            || candidate_victim_index != self.candidate_victim_index
        {
            return Err(CoordinatorAuditError::VictimPriorityIndex);
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
        if waiting_parent_count != self.waiting_parent_count {
            return Err(CoordinatorAuditError::WaitingParentCount);
        }
        if input_memberships != self.conflicts.input_memberships {
            return Err(CoordinatorAuditError::ConflictEdgeCount);
        }
        if by_input != self.conflicts.by_input {
            return Err(CoordinatorAuditError::ConflictCandidateIndex);
        }
        if relations != self.conflicts.relations {
            return Err(CoordinatorAuditError::ConflictRelationIndex);
        }
        let expected_committing = candidate_hashes.iter().find_map(|hash| {
            self.entries
                .get(hash)
                .and_then(|entry| entry.is_committing().then_some(hash.clone()))
        });
        if expected_committing != self.conflicts.committing {
            return Err(CoordinatorAuditError::ConflictRelationIndex);
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
                QueueKind::Commit => QueueOrdering::Candidate,
                QueueKind::PreCheck | QueueKind::Resolve => QueueOrdering::Fifo,
            };
            if queue.ordering() != expected_ordering
                || &queue.live != expected
                || !queue.structure_valid()
            {
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
