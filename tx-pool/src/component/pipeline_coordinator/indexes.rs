use super::*;

pub(super) struct ConflictRecheckPlan {
    blockers: HashSet<Byte32>,
    can_preempt: bool,
}

impl<R, U, V> PipelineCoordinator<R, U, V> {
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
            let entry = self
                .entries
                .get_mut(&waiter)
                .ok_or_else(|| CoordinatorError::Missing(waiter.clone()))?;
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

    pub(super) fn prepare_conflict_recheck(
        &mut self,
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
        self.ensure_revision_capacity(hash)?;
        let candidate = entry
            .candidate()
            .cloned()
            .ok_or(CoordinatorError::ConflictInvariant)?;
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
                        } if Self::compare_candidates(
                            hash,
                            &candidate,
                            blocker,
                            blocker_candidate,
                        ) == Ordering::Greater
                    )
                })
            });
        let mut inherited_waiters = HashSet::new();
        if can_preempt {
            for blocker in &blockers {
                self.ensure_revision_capacity(blocker)?;
                if let Some(waiters) = self.waiters_by_blocker.get(blocker) {
                    inherited_waiters.extend(waiters.iter().cloned());
                }
            }
            for waiter in &inherited_waiters {
                self.ensure_revision_capacity(waiter)?;
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
                    return Err(CoordinatorError::ConflictInvariant);
                }
            }
        }
        if blockers.is_empty() || can_preempt {
            let source = self
                .entries
                .get(hash)
                .map(|entry| entry.source)
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            self.queue_mut(QueueKind::Commit)?
                .reserve_live(source.is_proposal(), source.queue_owner())?;
        }
        self.conflict_rechecks
            .try_reserve(inherited_waiters.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.conflict_recheck_set
            .try_reserve(inherited_waiters.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        Ok(ConflictRecheckPlan {
            blockers,
            can_preempt,
        })
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
                let blocker_entry = self
                    .entries
                    .get_mut(blocker)
                    .ok_or_else(|| CoordinatorError::Missing(blocker.clone()))?;
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
                let entry = self
                    .entries
                    .get_mut(hash)
                    .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
                let EntryState::CandidateVerified { location, .. } = &mut entry.state else {
                    return Err(CoordinatorError::ConflictInvariant);
                };
                *location = CandidateLocation::Ready;
                entry.queue_sequence = queue_sequence
                    .map(|(sequence, _)| sequence)
                    .ok_or(CoordinatorError::QueueSequenceExhausted)?;
                entry.verify_schedule = VerifySchedule::default();
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
            let entry = self
                .entries
                .get_mut(hash)
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
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
