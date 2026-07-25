use super::*;

/// The bounded, executable conflict projection. Candidate lifecycle remains
/// in `entries`; this index stores only input membership and the relation
/// counters needed to derive commit eligibility in O(local cohort) time.
#[derive(Debug, Default)]
pub(super) struct StagedConflictIndex {
    pub(super) by_input: HashMap<OutPoint, HashSet<Byte32>>,
    pub(super) relations: HashMap<Byte32, CandidateRelation>,
    /// Number of candidate/input memberships. This is the charged and bounded
    /// resource historically named `conflict_edge_count`.
    pub(super) input_memberships: usize,
    /// Global commit is serialized. Keeping the sole committing identity in
    /// the derived projection makes the coordinator enforce that invariant in
    /// O(1), independently of the production runtime's outer mutex.
    pub(super) committing: Option<Byte32>,
}

impl StagedConflictIndex {
    pub(super) fn clear(&mut self) {
        self.by_input.clear();
        self.relations.clear();
        self.input_memberships = 0;
        self.committing = None;
    }
}

/// One prevalidated local graph mutation. It contains complete post-state for
/// every affected relation, so application has no fallible counter arithmetic
/// and rollback can rebuild the derived projection from authoritative entries.
#[derive(Debug)]
pub(super) struct ConflictDelta {
    expected_input_memberships: usize,
    next_input_memberships: usize,
    expected_committing: Option<Byte32>,
    next_committing: Option<Byte32>,
    insert: Option<(Byte32, HashSet<OutPoint>)>,
    remove: Vec<(Byte32, HashSet<OutPoint>)>,
    relations_after: HashMap<Byte32, CandidateRelation>,
    removed_relations: HashSet<Byte32>,
    affected: Vec<Byte32>,
    eligible_before: HashSet<Byte32>,
    eligible_after: HashSet<Byte32>,
}

impl ConflictDelta {
    pub(super) fn affected(&self) -> &[Byte32] {
        &self.affected
    }

    pub(super) fn removes(&self, hash: &Byte32) -> bool {
        self.removed_relations.contains(hash)
    }
}

#[derive(Debug)]
pub(super) struct ConflictTicketPlan {
    remove: Vec<CoordinatorTicket>,
    add: Vec<(Byte32, u64)>,
    revise: Vec<Byte32>,
    next_queue_sequence: u64,
}

impl ConflictTicketPlan {
    pub(super) fn revises(&self, hash: &Byte32) -> bool {
        self.revise
            .binary_search_by(|item| item.as_slice().cmp(hash.as_slice()))
            .is_ok()
    }
}

impl<R, U, V> PipelineCoordinator<R, U, V> {
    pub(super) fn candidate_rank(&self, hash: &Byte32) -> Result<CandidateRank, CoordinatorError> {
        let entry = self
            .entries
            .get(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        let EntryState::CandidateVerified {
            candidate,
            location,
            ..
        } = &entry.state
        else {
            return Err(CoordinatorError::ConflictInvariant);
        };
        Ok(CandidateRank::from_entry(
            hash,
            entry.source,
            candidate,
            location,
        ))
    }

    pub(super) fn candidate_is_eligible(&self, hash: &Byte32) -> bool {
        self.entries.get(hash).is_some_and(|entry| {
            matches!(
                &entry.state,
                EntryState::CandidateVerified {
                    location: CandidateLocation::Verified,
                    ..
                }
            ) && self
                .conflicts
                .relations
                .get(hash)
                .is_some_and(|relation| relation.stronger_count == 0)
        })
    }

    pub(super) fn candidate_queue_kind(
        &self,
        hash: &Byte32,
        entry: &CoordinatorEntry<R, U, V>,
    ) -> Option<QueueKind> {
        entry.queue_kind().or_else(|| {
            self.candidate_is_eligible(hash)
                .then_some(QueueKind::Commit)
        })
    }

    /// Return the distinct staged conflict cohort across every input. The
    /// query touches only local input buckets and stops at the same hard bound
    /// used by final RBF replacement.
    pub(super) fn bounded_conflicting_candidates(
        &self,
        hash: &Byte32,
        inputs: &HashSet<OutPoint>,
    ) -> Result<HashSet<Byte32>, CoordinatorError> {
        let mut conflicts = HashSet::new();
        conflicts
            .try_reserve(self.limits.max_candidates_per_input.saturating_add(1))
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        for input in inputs {
            let Some(candidates) = self.conflicts.by_input.get(input) else {
                continue;
            };
            for candidate in candidates {
                if candidate != hash
                    && conflicts.insert(candidate.clone())
                    && conflicts.len() > self.limits.max_candidates_per_input
                {
                    return Err(CoordinatorError::ConflictCohortLimitExceeded);
                }
            }
        }
        Ok(conflicts)
    }

    /// Undo preparation runs before capacity victims are removed, so its
    /// cohort may temporarily exceed the final direct-degree bound. It is
    /// still strictly bounded by input_count * per_input_limit and never scans
    /// unrelated candidates.
    pub(super) fn conflicting_candidates_for_undo(
        &self,
        hash: &Byte32,
        inputs: &HashSet<OutPoint>,
    ) -> Result<HashSet<Byte32>, CoordinatorError> {
        let limit = inputs
            .len()
            .checked_mul(self.limits.max_candidates_per_input)
            .ok_or(CoordinatorError::ConflictCohortLimitExceeded)?;
        let mut conflicts = HashSet::new();
        conflicts
            .try_reserve(limit)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        for input in inputs {
            if let Some(candidates) = self.conflicts.by_input.get(input) {
                conflicts.extend(candidates.iter().filter(|other| *other != hash).cloned());
            }
        }
        if conflicts.len() > limit {
            return Err(CoordinatorError::ConflictCohortLimitExceeded);
        }
        Ok(conflicts)
    }

    fn current_candidate_eligible(&self, hash: &Byte32, relation: CandidateRelation) -> bool {
        relation.stronger_count == 0
            && self.entries.get(hash).is_some_and(|entry| {
                matches!(
                    &entry.state,
                    EntryState::CandidateVerified {
                        location: CandidateLocation::Verified,
                        ..
                    }
                )
            })
    }

    pub(super) fn preview_conflict_insert(
        &self,
        hash: &Byte32,
        source: CoordinatorSource,
        candidate: &CandidateMeta,
    ) -> Result<ConflictDelta, CoordinatorError> {
        if self.conflicts.relations.contains_key(hash) {
            return Err(CoordinatorError::ConflictInvariant);
        }
        let next_input_memberships = self
            .conflicts
            .input_memberships
            .checked_add(candidate.inputs.len())
            .ok_or(CoordinatorError::ConflictEdgeLimitExceeded)?;
        if next_input_memberships > self.limits.max_conflict_edges {
            return Err(CoordinatorError::ConflictEdgeLimitExceeded);
        }
        for input in &candidate.inputs {
            if self.conflicts.by_input.get(input).map_or(0, HashSet::len)
                >= self.limits.max_candidates_per_input
            {
                return Err(CoordinatorError::ConflictCandidateLimitExceeded(
                    input.clone(),
                ));
            }
        }
        let neighbours = self.bounded_conflicting_candidates(hash, &candidate.inputs)?;
        let mut relations_after = HashMap::new();
        relations_after
            .try_reserve(neighbours.len().saturating_add(1))
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        let mut eligible_before = HashSet::new();
        eligible_before
            .try_reserve(neighbours.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        let incoming_rank = CandidateRank::verified(hash, source, candidate);
        let mut incoming_relation = CandidateRelation::default();
        for neighbour in &neighbours {
            let relation = self
                .conflicts
                .relations
                .get(neighbour)
                .copied()
                .ok_or(CoordinatorError::ConflictInvariant)?;
            if relation.degree >= self.limits.max_candidates_per_input {
                return Err(CoordinatorError::ConflictCohortLimitExceeded);
            }
            if self.current_candidate_eligible(neighbour, relation) {
                eligible_before.insert(neighbour.clone());
            }
            let mut next = relation;
            next.degree = next
                .degree
                .checked_add(1)
                .ok_or(CoordinatorError::ConflictInvariant)?;
            incoming_relation.degree = incoming_relation
                .degree
                .checked_add(1)
                .ok_or(CoordinatorError::ConflictInvariant)?;
            if incoming_rank > self.candidate_rank(neighbour)? {
                next.stronger_count = next
                    .stronger_count
                    .checked_add(1)
                    .ok_or(CoordinatorError::ConflictInvariant)?;
            } else {
                incoming_relation.stronger_count = incoming_relation
                    .stronger_count
                    .checked_add(1)
                    .ok_or(CoordinatorError::ConflictInvariant)?;
            }
            relations_after.insert(neighbour.clone(), next);
        }
        relations_after.insert(hash.clone(), incoming_relation);
        let mut eligible_after = HashSet::new();
        eligible_after
            .try_reserve(neighbours.len().saturating_add(1))
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        for (candidate_hash, relation) in &relations_after {
            if relation.stronger_count == 0
                && (candidate_hash == hash
                    || self.entries.get(candidate_hash).is_some_and(|entry| {
                        matches!(
                            &entry.state,
                            EntryState::CandidateVerified {
                                location: CandidateLocation::Verified,
                                ..
                            }
                        )
                    }))
            {
                eligible_after.insert(candidate_hash.clone());
            }
        }
        let mut affected: Vec<_> = neighbours.into_iter().collect();
        affected.push(hash.clone());
        affected.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        affected.dedup();
        Ok(ConflictDelta {
            expected_input_memberships: self.conflicts.input_memberships,
            next_input_memberships,
            expected_committing: self.conflicts.committing.clone(),
            next_committing: self.conflicts.committing.clone(),
            insert: Some((hash.clone(), candidate.inputs.clone())),
            remove: Vec::new(),
            relations_after,
            removed_relations: HashSet::new(),
            affected,
            eligible_before,
            eligible_after,
        })
    }

    pub(super) fn preview_conflict_remove_many(
        &self,
        removed: &HashSet<Byte32>,
    ) -> Result<ConflictDelta, CoordinatorError> {
        let mut remove = Vec::new();
        remove
            .try_reserve(removed.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        let mut removed_memberships = 0usize;
        let mut survivor_relations = HashMap::new();
        let mut eligible_before = HashSet::new();
        let mut affected: HashSet<Byte32> = removed.iter().cloned().collect();
        for hash in removed {
            let entry = self
                .entries
                .get(hash)
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            let Some(candidate) = entry.candidate() else {
                continue;
            };
            let relation = self
                .conflicts
                .relations
                .get(hash)
                .copied()
                .ok_or(CoordinatorError::ConflictInvariant)?;
            if self.current_candidate_eligible(hash, relation) {
                eligible_before.insert(hash.clone());
            }
            let neighbours = self.bounded_conflicting_candidates(hash, &candidate.inputs)?;
            if neighbours.len() != relation.degree {
                return Err(CoordinatorError::ConflictInvariant);
            }
            let removed_rank = self.candidate_rank(hash)?;
            for neighbour in neighbours {
                if removed.contains(&neighbour) {
                    continue;
                }
                affected.insert(neighbour.clone());
                let relation = survivor_relations
                    .entry(neighbour.clone())
                    .or_insert_with(|| {
                        self.conflicts
                            .relations
                            .get(&neighbour)
                            .copied()
                            .unwrap_or_default()
                    });
                if !self.conflicts.relations.contains_key(&neighbour) {
                    return Err(CoordinatorError::ConflictInvariant);
                }
                relation.degree = relation
                    .degree
                    .checked_sub(1)
                    .ok_or(CoordinatorError::ConflictInvariant)?;
                if removed_rank > self.candidate_rank(&neighbour)? {
                    relation.stronger_count = relation
                        .stronger_count
                        .checked_sub(1)
                        .ok_or(CoordinatorError::ConflictInvariant)?;
                }
            }
            removed_memberships = removed_memberships
                .checked_add(candidate.inputs.len())
                .ok_or(CoordinatorError::ConflictInvariant)?;
            remove.push((hash.clone(), candidate.inputs.clone()));
        }
        let next_input_memberships = self
            .conflicts
            .input_memberships
            .checked_sub(removed_memberships)
            .ok_or(CoordinatorError::ConflictInvariant)?;
        for hash in survivor_relations.keys() {
            let old = self
                .conflicts
                .relations
                .get(hash)
                .copied()
                .ok_or(CoordinatorError::ConflictInvariant)?;
            if self.current_candidate_eligible(hash, old) {
                eligible_before.insert(hash.clone());
            }
        }
        let mut eligible_after = HashSet::new();
        for (hash, relation) in &survivor_relations {
            if self.current_candidate_eligible(hash, *relation) {
                eligible_after.insert(hash.clone());
            }
        }
        // Unaffected candidates retain eligibility and intentionally are not
        // represented in either set; ticket reconciliation is delta-local.
        let mut affected: Vec<_> = affected.into_iter().collect();
        affected.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        let mut removed_relations = HashSet::new();
        removed_relations
            .try_reserve(remove.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        removed_relations.extend(remove.iter().map(|(hash, _)| hash.clone()));
        Ok(ConflictDelta {
            expected_input_memberships: self.conflicts.input_memberships,
            next_input_memberships,
            expected_committing: self.conflicts.committing.clone(),
            next_committing: self
                .conflicts
                .committing
                .as_ref()
                .filter(|committing| !removed.contains(*committing))
                .cloned(),
            insert: None,
            remove,
            relations_after: survivor_relations,
            removed_relations,
            affected,
            eligible_before,
            eligible_after,
        })
    }

    pub(super) fn preview_conflict_rerank(
        &self,
        hash: &Byte32,
        next_rank: &CandidateRank,
        next_location: CandidateLocation,
    ) -> Result<ConflictDelta, CoordinatorError> {
        let entry = self
            .entries
            .get(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        let candidate = entry
            .candidate()
            .ok_or(CoordinatorError::ConflictInvariant)?;
        let next_committing = match next_location {
            CandidateLocation::Committing => {
                if self
                    .conflicts
                    .committing
                    .as_ref()
                    .is_some_and(|committing| committing != hash)
                {
                    return Err(CoordinatorError::CommitInProgress(
                        self.conflicts
                            .committing
                            .clone()
                            .ok_or(CoordinatorError::ConflictInvariant)?,
                    ));
                }
                Some(hash.clone())
            }
            CandidateLocation::Verified => self
                .conflicts
                .committing
                .as_ref()
                .filter(|committing| *committing != hash)
                .cloned(),
        };
        let neighbours = self.bounded_conflicting_candidates(hash, &candidate.inputs)?;
        let old_rank = self.candidate_rank(hash)?;
        let mut relations_after = HashMap::new();
        relations_after
            .try_reserve(neighbours.len().saturating_add(1))
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        let subject_relation = self
            .conflicts
            .relations
            .get(hash)
            .copied()
            .ok_or(CoordinatorError::ConflictInvariant)?;
        if subject_relation.degree != neighbours.len() {
            return Err(CoordinatorError::ConflictInvariant);
        }
        relations_after.insert(hash.clone(), subject_relation);
        for neighbour in &neighbours {
            let neighbour_rank = self.candidate_rank(neighbour)?;
            let old_order = old_rank.cmp(&neighbour_rank);
            let next_order = next_rank.cmp(&neighbour_rank);
            let mut subject = relations_after
                .get(hash)
                .copied()
                .ok_or(CoordinatorError::ConflictInvariant)?;
            let mut neighbour_relation = self
                .conflicts
                .relations
                .get(neighbour)
                .copied()
                .ok_or(CoordinatorError::ConflictInvariant)?;
            match (old_order, next_order) {
                (Ordering::Greater, Ordering::Less) => {
                    neighbour_relation.stronger_count = neighbour_relation
                        .stronger_count
                        .checked_sub(1)
                        .ok_or(CoordinatorError::ConflictInvariant)?;
                    subject.stronger_count = subject
                        .stronger_count
                        .checked_add(1)
                        .ok_or(CoordinatorError::ConflictInvariant)?;
                }
                (Ordering::Less, Ordering::Greater) => {
                    subject.stronger_count = subject
                        .stronger_count
                        .checked_sub(1)
                        .ok_or(CoordinatorError::ConflictInvariant)?;
                    neighbour_relation.stronger_count = neighbour_relation
                        .stronger_count
                        .checked_add(1)
                        .ok_or(CoordinatorError::ConflictInvariant)?;
                }
                (Ordering::Equal, _) | (_, Ordering::Equal) => {
                    return Err(CoordinatorError::ConflictInvariant);
                }
                _ => {}
            }
            relations_after.insert(hash.clone(), subject);
            relations_after.insert(neighbour.clone(), neighbour_relation);
        }
        let mut eligible_before = HashSet::new();
        let mut eligible_after = HashSet::new();
        for (candidate_hash, next_relation) in &relations_after {
            let old_relation = self
                .conflicts
                .relations
                .get(candidate_hash)
                .copied()
                .ok_or(CoordinatorError::ConflictInvariant)?;
            if self.current_candidate_eligible(candidate_hash, old_relation) {
                eligible_before.insert(candidate_hash.clone());
            }
            let verified_after = if candidate_hash == hash {
                next_location == CandidateLocation::Verified
            } else {
                self.entries.get(candidate_hash).is_some_and(|entry| {
                    matches!(
                        &entry.state,
                        EntryState::CandidateVerified {
                            location: CandidateLocation::Verified,
                            ..
                        }
                    )
                })
            };
            if verified_after && next_relation.stronger_count == 0 {
                eligible_after.insert(candidate_hash.clone());
            }
        }
        let mut affected: Vec<_> = relations_after.keys().cloned().collect();
        affected.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        Ok(ConflictDelta {
            expected_input_memberships: self.conflicts.input_memberships,
            next_input_memberships: self.conflicts.input_memberships,
            expected_committing: self.conflicts.committing.clone(),
            next_committing,
            insert: None,
            remove: Vec::new(),
            relations_after,
            removed_relations: HashSet::new(),
            affected,
            eligible_before,
            eligible_after,
        })
    }

    pub(super) fn prepare_conflict_ticket_plan(
        &mut self,
        delta: &ConflictDelta,
        force_reticket: &HashSet<Byte32>,
        source_overrides: &HashMap<Byte32, CoordinatorSource>,
    ) -> Result<ConflictTicketPlan, CoordinatorError> {
        let queue = self
            .queues
            .get(&QueueKind::Commit)
            .ok_or(CoordinatorError::QueueInvariant(QueueKind::Commit))?;
        let mut remove = Vec::new();
        let mut add_hashes = Vec::new();
        let mut revise = HashSet::new();
        for hash in delta.affected() {
            let before = delta.eligible_before.contains(hash);
            let after = delta.eligible_after.contains(hash);
            let force = force_reticket.contains(hash) && !delta.removes(hash);
            let live = self
                .entries
                .get(hash)
                .map(|entry| queue.live.contains(&entry.ticket(hash)))
                .unwrap_or(false);
            if live != before {
                return Err(CoordinatorError::QueueInvariant(QueueKind::Commit));
            }
            if before && (!after || force) {
                let ticket = self
                    .entries
                    .get(hash)
                    .map(|entry| entry.ticket(hash))
                    .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
                remove.push(ticket);
            }
            if after && (!before || force) {
                add_hashes.push(hash.clone());
            }
            if !delta.removes(hash) && (before != after || force) {
                self.ensure_revision_capacity(hash)?;
                revise.insert(hash.clone());
            }
        }
        add_hashes.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        let owners = add_hashes
            .iter()
            .map(|hash| {
                self.entries
                    .get(hash)
                    .map(|entry| {
                        source_overrides
                            .get(hash)
                            .copied()
                            .unwrap_or(entry.source)
                            .queue_owner()
                    })
                    .ok_or_else(|| CoordinatorError::Missing(hash.clone()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let (first_sequence, next_queue_sequence) = self.queue_sequence_range(add_hashes.len())?;
        let add = add_hashes
            .into_iter()
            .enumerate()
            .map(|(offset, hash)| {
                let offset =
                    u64::try_from(offset).map_err(|_| CoordinatorError::QueueSequenceExhausted)?;
                let sequence = first_sequence
                    .checked_add(offset)
                    .ok_or(CoordinatorError::QueueSequenceExhausted)?;
                Ok((hash, sequence))
            })
            .collect::<Result<Vec<_>, CoordinatorError>>()?;
        // Reservation is the final fallible preparation step. Callers create
        // the plan inside an undo transaction, so any later failure rebuilds
        // the queue and cannot strand reservation-only metadata.
        self.queue_mut(QueueKind::Commit)?
            .reserve_many(owners, false)?;
        let mut revise: Vec<_> = revise.into_iter().collect();
        revise.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        Ok(ConflictTicketPlan {
            remove,
            add,
            revise,
            next_queue_sequence,
        })
    }

    pub(super) fn remove_conflict_tickets(
        &mut self,
        plan: &ConflictTicketPlan,
    ) -> Result<(), CoordinatorError> {
        for ticket in &plan.remove {
            self.queue_mut(QueueKind::Commit)?
                .remove_live(QueueKind::Commit, ticket)?;
        }
        Ok(())
    }

    pub(super) fn apply_conflict_delta(
        &mut self,
        delta: &ConflictDelta,
    ) -> Result<(), CoordinatorError> {
        if self.conflicts.input_memberships != delta.expected_input_memberships
            || self.conflicts.committing != delta.expected_committing
        {
            return Err(CoordinatorError::ConflictInvariant);
        }
        self.conflicts
            .relations
            .try_reserve(delta.relations_after.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        if let Some((_, inputs)) = &delta.insert {
            self.conflicts
                .by_input
                .try_reserve(inputs.len())
                .map_err(|_| CoordinatorError::QueueReservationFailed)?;
            for input in inputs {
                self.conflicts
                    .by_input
                    .entry(input.clone())
                    .or_default()
                    .try_reserve(1)
                    .map_err(|_| CoordinatorError::QueueReservationFailed)?;
            }
        }
        for (hash, inputs) in &delta.remove {
            for input in inputs {
                let bucket = self
                    .conflicts
                    .by_input
                    .get_mut(input)
                    .ok_or(CoordinatorError::ConflictInvariant)?;
                if !bucket.remove(hash) {
                    return Err(CoordinatorError::ConflictInvariant);
                }
                if bucket.is_empty() {
                    self.conflicts.by_input.remove(input);
                }
            }
        }
        if let Some((hash, inputs)) = &delta.insert {
            for input in inputs {
                if !self
                    .conflicts
                    .by_input
                    .entry(input.clone())
                    .or_default()
                    .insert(hash.clone())
                {
                    return Err(CoordinatorError::ConflictInvariant);
                }
            }
        }
        for hash in &delta.removed_relations {
            if self.conflicts.relations.remove(hash).is_none() {
                return Err(CoordinatorError::ConflictInvariant);
            }
        }
        for (hash, relation) in &delta.relations_after {
            self.conflicts.relations.insert(hash.clone(), *relation);
        }
        self.conflicts.input_memberships = delta.next_input_memberships;
        self.conflicts.committing = delta.next_committing.clone();
        Ok(())
    }

    pub(super) fn apply_conflict_ticket_plan(
        &mut self,
        plan: ConflictTicketPlan,
    ) -> Result<(), CoordinatorError> {
        for hash in &plan.revise {
            let entry = self.entry_mut(hash)?;
            entry.revision += 1;
        }
        let mut tickets = Vec::new();
        tickets
            .try_reserve(plan.add.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        for (hash, sequence) in plan.add {
            if !self.candidate_is_eligible(&hash) {
                return Err(CoordinatorError::ConflictInvariant);
            }
            let entry = self.entry_mut(&hash)?;
            entry.queue_sequence = sequence;
            tickets.push((entry.ticket(&hash), entry.source.is_proposal()));
        }
        for (ticket, priority) in tickets {
            self.queue_mut(QueueKind::Commit)?.push_reserved(
                QueueKind::Commit,
                ticket,
                priority,
            )?;
        }
        self.next_queue_sequence = plan.next_queue_sequence;
        Ok(())
    }

    pub(super) fn preflight_remove_conflict_indexes(
        &self,
        hash: &Byte32,
    ) -> Result<(), CoordinatorError> {
        let Some(entry) = self.entries.get(hash) else {
            return Err(CoordinatorError::Missing(hash.clone()));
        };
        let Some(candidate) = entry.candidate() else {
            return Ok(());
        };
        let relation = self
            .conflicts
            .relations
            .get(hash)
            .copied()
            .ok_or(CoordinatorError::ConflictInvariant)?;
        if self.conflicts.input_memberships < candidate.inputs.len()
            || self
                .bounded_conflicting_candidates(hash, &candidate.inputs)?
                .len()
                != relation.degree
        {
            return Err(CoordinatorError::ConflictInvariant);
        }
        Ok(())
    }

    /// Compatibility helper for single-owner transitions. Callers must have
    /// snapshotted the candidate's direct neighbours; the delta updates their
    /// eligibility and commit tickets synchronously, with no maintenance gap.
    pub(super) fn remove_conflict_indexes(
        &mut self,
        hash: &Byte32,
    ) -> Result<(), CoordinatorError> {
        if self
            .entries
            .get(hash)
            .and_then(CoordinatorEntry::candidate)
            .is_none()
        {
            return Ok(());
        }
        let delta = self.preview_conflict_remove_many(&HashSet::from([hash.clone()]))?;
        let ticket_plan =
            self.prepare_conflict_ticket_plan(&delta, &HashSet::new(), &HashMap::new())?;
        self.remove_conflict_tickets(&ticket_plan)?;
        self.apply_conflict_delta(&delta)?;
        self.apply_conflict_ticket_plan(ticket_plan)
    }
}
