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
}

#[cfg(test)]
#[path = "../tests/pipeline_coordinator_audit_seam.rs"]
mod test_seam;
