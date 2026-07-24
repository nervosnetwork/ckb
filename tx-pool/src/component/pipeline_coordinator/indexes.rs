use super::*;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct ConflictRecheckPlan {
    pub(super) blockers: HashSet<Byte32>,
    pub(super) can_preempt: bool,
    pub(super) inherited_waiters: HashSet<Byte32>,
}

impl<R, U, V> PipelineCoordinator<R, U, V> {
    pub(super) fn preview_candidate_preemption(
        &self,
        hash: &Byte32,
        source: CoordinatorSource,
        candidate: &CandidateMeta,
    ) -> Result<ConflictRecheckPlan, CoordinatorError> {
        let blockers = self.active_blockers_for_inputs(hash, &candidate.inputs);
        let can_preempt = !blockers.is_empty()
            && blockers.iter().all(|blocker| {
                self.entries.get(blocker).is_some_and(|blocker_entry| {
                    matches!(
                        &blocker_entry.state,
                        EntryState::CandidateVerified {
                            candidate: blocker_candidate,
                            location: CandidateLocation::Ready,
                            ..
                        } if Self::compare_candidate_capacity(
                            hash,
                            source,
                            candidate,
                            blocker,
                            blocker_entry.source,
                            blocker_candidate,
                        ) == Ordering::Greater
                    )
                })
            });
        let mut inherited_waiters = HashSet::new();
        if can_preempt {
            for blocker in &blockers {
                if let Some(waiters) = self.waiters_by_blocker.get(blocker) {
                    inherited_waiters.extend(waiters.iter().cloned());
                }
            }
            // One full blocker domain plus one full waiter domain is the
            // largest atomic preemption cohort. Without this union cap, a
            // multi-input candidate can join many individually bounded
            // blocker buckets into an unbounded undo transaction.
            let transition_limit = self
                .limits
                .max_candidates_per_input
                .checked_mul(2)
                .ok_or(CoordinatorError::ConflictCohortLimitExceeded)?;
            let transition_size = blockers
                .len()
                .checked_add(inherited_waiters.len())
                .ok_or(CoordinatorError::ConflictCohortLimitExceeded)?;
            if transition_size > transition_limit {
                return Err(CoordinatorError::ConflictCohortLimitExceeded);
            }
        }
        Ok(ConflictRecheckPlan {
            blockers,
            can_preempt,
            inherited_waiters,
        })
    }

    /// Return the distinct verified conflict cohort across every input, with
    /// an early hard stop at the same bound used by final RBF replacement.
    /// Per-input limits alone are insufficient: disjoint 100-candidate
    /// buckets can otherwise be unioned by one large multi-input transaction.
    pub(super) fn bounded_conflicting_candidates(
        &self,
        hash: &Byte32,
        inputs: &HashSet<OutPoint>,
    ) -> Result<HashSet<Byte32>, CoordinatorError> {
        let mut conflicts = HashSet::new();
        let reservation = self
            .limits
            .max_candidates_per_input
            .checked_add(1)
            .ok_or(CoordinatorError::ConflictCohortLimitExceeded)?;
        conflicts
            .try_reserve(reservation)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        for input in inputs {
            let Some(candidates) = self.candidates_by_input.get(input) else {
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

    pub(super) fn preflight_register_candidate_conflicts(
        &mut self,
        hash: &Byte32,
        inputs: &HashSet<OutPoint>,
    ) -> Result<HashSet<Byte32>, CoordinatorError> {
        if self.candidate_conflict_counts.contains_key(hash) {
            return Err(CoordinatorError::ConflictInvariant);
        }
        let conflicts = self.bounded_conflicting_candidates(hash, inputs)?;
        for conflict in &conflicts {
            let count = self
                .candidate_conflict_counts
                .get(conflict)
                .copied()
                .ok_or(CoordinatorError::ConflictInvariant)?;
            if count >= self.limits.max_candidates_per_input {
                return Err(CoordinatorError::ConflictCohortLimitExceeded);
            }
        }
        self.candidate_conflict_counts
            .try_reserve(1)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        Ok(conflicts)
    }

    pub(super) fn register_candidate_conflicts(
        &mut self,
        hash: &Byte32,
        conflicts: &HashSet<Byte32>,
    ) -> Result<(), CoordinatorError> {
        if self
            .candidate_conflict_counts
            .insert(hash.clone(), conflicts.len())
            .is_some()
        {
            return Err(CoordinatorError::ConflictInvariant);
        }
        for conflict in conflicts {
            let count = self
                .candidate_conflict_counts
                .get_mut(conflict)
                .ok_or(CoordinatorError::ConflictInvariant)?;
            *count = count
                .checked_add(1)
                .ok_or(CoordinatorError::ConflictInvariant)?;
            if *count > self.limits.max_candidates_per_input {
                return Err(CoordinatorError::ConflictCohortLimitExceeded);
            }
        }
        Ok(())
    }

    pub(super) fn unregister_candidate_conflicts(
        &mut self,
        hash: &Byte32,
        inputs: &HashSet<OutPoint>,
    ) -> Result<(), CoordinatorError> {
        let conflicts = self.bounded_conflicting_candidates(hash, inputs)?;
        let recorded = self
            .candidate_conflict_counts
            .remove(hash)
            .ok_or(CoordinatorError::ConflictInvariant)?;
        if recorded != conflicts.len() {
            return Err(CoordinatorError::ConflictInvariant);
        }
        for conflict in conflicts {
            let count = self
                .candidate_conflict_counts
                .get_mut(&conflict)
                .ok_or(CoordinatorError::ConflictInvariant)?;
            *count = count
                .checked_sub(1)
                .ok_or(CoordinatorError::ConflictInvariant)?;
        }
        Ok(())
    }

    pub(super) fn active_blockers_for_inputs(
        &self,
        hash: &Byte32,
        inputs: &HashSet<OutPoint>,
    ) -> HashSet<Byte32> {
        inputs
            .iter()
            .filter_map(|input| self.active_by_input.get(input).cloned())
            .filter(|blocker| blocker != hash)
            .collect()
    }

    pub(super) fn compare_candidates(
        left_hash: &Byte32,
        left: &CandidateMeta,
        right_hash: &Byte32,
        right: &CandidateMeta,
    ) -> Ordering {
        let left_rate = u128::from(left.fee) * right.tx_size as u128;
        let right_rate = u128::from(right.fee) * left.tx_size as u128;
        left_rate
            .cmp(&right_rate)
            .then_with(|| left.fee.cmp(&right.fee))
            .then_with(|| right.arrival.cmp(&left.arrival))
            .then_with(|| right_hash.as_slice().cmp(left_hash.as_slice()))
    }

    pub(super) fn claim_conflict_inputs(&mut self, hash: &Byte32) -> Result<(), CoordinatorError> {
        let inputs = self
            .entries
            .get(hash)
            .and_then(CoordinatorEntry::candidate)
            .map(|candidate| candidate.inputs.clone())
            .ok_or(CoordinatorError::ConflictInvariant)?;
        if inputs
            .iter()
            .any(|input| self.active_by_input.contains_key(input))
        {
            return Err(CoordinatorError::ConflictInvariant);
        }
        for input in inputs {
            self.active_by_input.insert(input, hash.clone());
        }
        Ok(())
    }

    pub(super) fn release_conflict_claims(
        &mut self,
        hash: &Byte32,
    ) -> Result<(), CoordinatorError> {
        let inputs = self
            .entries
            .get(hash)
            .and_then(CoordinatorEntry::candidate)
            .map(|candidate| candidate.inputs.clone())
            .ok_or(CoordinatorError::ConflictInvariant)?;
        for input in inputs {
            if self.active_by_input.get(&input) == Some(hash) {
                self.active_by_input.remove(&input);
            }
        }
        Ok(())
    }

    pub(super) fn release_conflict_claims_if_present(&mut self, hash: &Byte32) {
        let Some(inputs) = self
            .entries
            .get(hash)
            .and_then(CoordinatorEntry::candidate)
            .map(|candidate| candidate.inputs.clone())
        else {
            return;
        };
        for input in inputs {
            if self.active_by_input.get(&input) == Some(hash) {
                self.active_by_input.remove(&input);
            }
        }
    }

    pub(super) fn remove_conflict_waiter_links(
        &mut self,
        hash: &Byte32,
    ) -> Result<(), CoordinatorError> {
        let blockers = self
            .entries
            .get(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?
            .waiting_conflict_blockers()
            .cloned();
        let Some(blockers) = blockers else {
            return Ok(());
        };
        for blocker in blockers {
            if let Some(waiters) = self.waiters_by_blocker.get_mut(&blocker) {
                waiters.remove(hash);
                if waiters.is_empty() {
                    self.waiters_by_blocker.remove(&blocker);
                }
            }
        }
        Ok(())
    }

    pub(super) fn invalidate_conflict_waiters(
        &mut self,
        blocker: &Byte32,
    ) -> Result<(), CoordinatorError> {
        let mut waiters: Vec<_> = self
            .waiters_by_blocker
            .get(blocker)
            .into_iter()
            .flat_map(|waiters| waiters.iter().cloned())
            .collect();
        waiters.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        for waiter in &waiters {
            let entry = self
                .entries
                .get(waiter)
                .ok_or_else(|| CoordinatorError::Missing(waiter.clone()))?;
            if !matches!(
                &entry.state,
                EntryState::CandidateVerified {
                    location: CandidateLocation::WaitingConflict { blockers },
                    ..
                }
                    if blockers.contains(blocker)
            ) {
                return Err(CoordinatorError::ConflictInvariant);
            }
            self.ensure_revision_capacity(waiter)?;
        }
        self.conflict_rechecks
            .try_reserve(waiters.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.conflict_recheck_set
            .try_reserve(waiters.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        let (mut sequence, next_maintenance_sequence) =
            self.maintenance_sequence_range(waiters.len())?;
        self.next_maintenance_sequence = next_maintenance_sequence;
        self.waiters_by_blocker.remove(blocker);
        for waiter in waiters {
            self.remove_conflict_waiter_links(&waiter)?;
            let entry = self.entry_mut(&waiter)?;
            let EntryState::CandidateVerified { location, .. } = &mut entry.state else {
                return Err(CoordinatorError::ConflictInvariant);
            };
            *location = CandidateLocation::Recheck { sequence };
            sequence = sequence
                .checked_add(1)
                .ok_or(CoordinatorError::MaintenanceSequenceExhausted)?;
            entry.revision += 1;
            if self.conflict_recheck_set.insert(waiter.clone()) {
                self.conflict_rechecks.push_back(waiter);
            }
        }
        Ok(())
    }

    pub(super) fn remove_conflict_indexes(
        &mut self,
        hash: &Byte32,
    ) -> Result<(), CoordinatorError> {
        self.invalidate_conflict_waiters(hash)?;
        self.remove_conflict_waiter_links(hash)?;
        self.release_conflict_claims_if_present(hash);
        self.conflict_recheck_set.remove(hash);
        let Some(candidate) = self
            .entries
            .get(hash)
            .and_then(CoordinatorEntry::candidate)
            .cloned()
        else {
            return Ok(());
        };
        self.unregister_candidate_conflicts(hash, &candidate.inputs)?;
        self.conflict_edge_count = self
            .conflict_edge_count
            .checked_sub(candidate.inputs.len())
            .ok_or(CoordinatorError::ConflictInvariant)?;
        for input in candidate.inputs {
            if let Some(candidates) = self.candidates_by_input.get_mut(&input) {
                candidates.remove(hash);
                if candidates.is_empty() {
                    self.candidates_by_input.remove(&input);
                }
            }
        }
        Ok(())
    }

    pub(super) fn preflight_remove_conflict_indexes(
        &mut self,
        hash: &Byte32,
    ) -> Result<(), CoordinatorError> {
        let entry = self
            .entries
            .get(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        if let Some(candidate) = entry.candidate()
            && self.conflict_edge_count < candidate.inputs.len()
        {
            return Err(CoordinatorError::ConflictInvariant);
        }
        if let Some(candidate) = entry.candidate() {
            let conflicts = self.bounded_conflicting_candidates(hash, &candidate.inputs)?;
            if self.candidate_conflict_counts.get(hash).copied() != Some(conflicts.len()) {
                return Err(CoordinatorError::ConflictInvariant);
            }
        }
        if let Some(blockers) = entry.waiting_conflict_blockers() {
            for blocker in blockers {
                if !self
                    .waiters_by_blocker
                    .get(blocker)
                    .is_some_and(|waiters| waiters.contains(hash))
                {
                    return Err(CoordinatorError::ConflictInvariant);
                }
            }
        }
        let waiters = self
            .waiters_by_blocker
            .get(hash)
            .cloned()
            .unwrap_or_default();
        for waiter in &waiters {
            let waiter_entry = self
                .entries
                .get(waiter)
                .ok_or_else(|| CoordinatorError::Missing(waiter.clone()))?;
            if !matches!(
                &waiter_entry.state,
                EntryState::CandidateVerified {
                    location: CandidateLocation::WaitingConflict { blockers },
                    ..
                }
                    if blockers.contains(hash)
            ) {
                return Err(CoordinatorError::ConflictInvariant);
            }
            self.ensure_revision_capacity(waiter)?;
        }
        self.conflict_rechecks
            .try_reserve(waiters.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.conflict_recheck_set
            .try_reserve(waiters.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)
    }

    pub(super) fn preview_conflict_recheck(
        &self,
        hash: &Byte32,
    ) -> Result<ConflictRecheckPlan, CoordinatorError> {
        let entry = self
            .entries
            .get(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        if !matches!(
            &entry.state,
            EntryState::CandidateVerified {
                location: CandidateLocation::Recheck { .. },
                ..
            }
        ) {
            return Err(CoordinatorError::LocationMismatch {
                expected: CoordinatorLocation::ConflictRecheck,
                actual: entry.location(),
            });
        }
        let candidate = entry
            .candidate()
            .cloned()
            .ok_or(CoordinatorError::ConflictInvariant)?;
        let source = entry.source;
        self.preview_candidate_preemption(hash, source, &candidate)
    }

    pub(super) fn prepare_conflict_recheck(
        &mut self,
        hash: &Byte32,
        plan: &ConflictRecheckPlan,
    ) -> Result<(), CoordinatorError> {
        if &self.preview_conflict_recheck(hash)? != plan {
            return Err(CoordinatorError::ConflictInvariant);
        }
        self.ensure_revision_capacity(hash)?;
        for blocker in &plan.blockers {
            self.ensure_revision_capacity(blocker)?;
        }
        for waiter in &plan.inherited_waiters {
            self.ensure_revision_capacity(waiter)?;
        }
        if plan.blockers.is_empty() || plan.can_preempt {
            let source = self
                .entries
                .get(hash)
                .map(|entry| entry.source)
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            self.queue_mut(QueueKind::Commit)?
                .reserve_live(source.queue_owner(), false)?;
        }
        self.conflict_rechecks
            .try_reserve(plan.inherited_waiters.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.conflict_recheck_set
            .try_reserve(plan.inherited_waiters.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        Ok(())
    }

    pub(super) fn apply_conflict_recheck(
        &mut self,
        hash: &Byte32,
        plan: &ConflictRecheckPlan,
    ) -> Result<Option<CoordinatorTicket>, CoordinatorError> {
        let ready = plan.blockers.is_empty() || plan.can_preempt;
        let queue_sequence = if ready {
            Some(self.queue_sequence_range(1)?)
        } else {
            None
        };
        if plan.can_preempt {
            for blocker in &plan.blockers {
                self.invalidate_conflict_waiters(blocker)?;
                self.remove_current_queue_ticket(blocker)?;
                self.release_conflict_claims(blocker)?;
                let blocker_entry = self.entry_mut(blocker)?;
                let EntryState::CandidateVerified { location, .. } = &mut blocker_entry.state
                else {
                    return Err(CoordinatorError::ConflictInvariant);
                };
                *location = CandidateLocation::WaitingConflict {
                    blockers: HashSet::from([hash.clone()]),
                };
                blocker_entry.revision += 1;
                self.waiters_by_blocker
                    .entry(hash.clone())
                    .or_default()
                    .insert(blocker.clone());
                self.apply_fault_checkpoint();
            }
        }

        if ready {
            let (ticket, front) = {
                let entry = self.entry_mut(hash)?;
                let EntryState::CandidateVerified { location, .. } = &mut entry.state else {
                    return Err(CoordinatorError::ConflictInvariant);
                };
                *location = CandidateLocation::Ready;
                entry.queue_sequence = queue_sequence
                    .map(|(sequence, _)| sequence)
                    .ok_or(CoordinatorError::QueueSequenceExhausted)?;
                entry.revision += 1;
                (entry.ticket(hash), entry.source.is_proposal())
            };
            self.claim_conflict_inputs(hash)?;
            self.queue_mut(QueueKind::Commit)?.push_reserved(
                QueueKind::Commit,
                ticket.clone(),
                front,
            )?;
            self.next_queue_sequence = queue_sequence
                .map(|(_, next_sequence)| next_sequence)
                .ok_or(CoordinatorError::QueueSequenceExhausted)?;
            self.apply_fault_checkpoint();
            Ok(Some(ticket))
        } else {
            let entry = self.entry_mut(hash)?;
            let EntryState::CandidateVerified { location, .. } = &mut entry.state else {
                return Err(CoordinatorError::ConflictInvariant);
            };
            *location = CandidateLocation::WaitingConflict {
                blockers: plan.blockers.clone(),
            };
            entry.revision += 1;
            for blocker in &plan.blockers {
                self.waiters_by_blocker
                    .entry(blocker.clone())
                    .or_default()
                    .insert(hash.clone());
            }
            self.apply_fault_checkpoint();
            Ok(None)
        }
    }
}
