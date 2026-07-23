use super::*;

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
            .and_then(|entry| entry.candidate.as_ref())
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
            .and_then(|entry| entry.candidate.as_ref())
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
            .and_then(|entry| entry.candidate.as_ref())
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
        let blockers = match self.entries.get(hash).map(|entry| &entry.location) {
            Some(CoordinatorLocation::WaitingConflict { blockers }) => blockers.clone(),
            Some(_) => return Ok(()),
            None => return Err(CoordinatorError::Missing(hash.clone())),
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
        let waiters = self
            .waiters_by_blocker
            .get(blocker)
            .cloned()
            .unwrap_or_default();
        for waiter in &waiters {
            let entry = self
                .entries
                .get(waiter)
                .ok_or_else(|| CoordinatorError::Missing(waiter.clone()))?;
            if !matches!(
                &entry.location,
                CoordinatorLocation::WaitingConflict { blockers }
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
        self.waiters_by_blocker.remove(blocker);
        for waiter in waiters {
            self.remove_conflict_waiter_links(&waiter)?;
            let entry = self
                .entries
                .get_mut(&waiter)
                .ok_or_else(|| CoordinatorError::Missing(waiter.clone()))?;
            entry.location = CoordinatorLocation::ConflictRecheck;
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
            .and_then(|entry| entry.candidate.as_ref())
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

    pub(super) fn preflight_deactivate_conflict_indexes(
        &mut self,
        hash: &Byte32,
    ) -> Result<(), CoordinatorError> {
        self.preflight_remove_conflict_indexes(hash)
    }

    pub(super) fn deactivate_conflict_indexes(
        &mut self,
        hash: &Byte32,
    ) -> Result<(), CoordinatorError> {
        self.invalidate_conflict_waiters(hash)?;
        self.remove_conflict_waiter_links(hash)?;
        self.release_conflict_claims_if_present(hash);
        self.conflict_recheck_set.remove(hash);
        Ok(())
    }

    pub(super) fn preflight_remove_pool_input_indexes(
        &self,
        hash: &Byte32,
    ) -> Result<(), CoordinatorError> {
        let entry = self
            .entries
            .get(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        let CoordinatorLocation::WaitingPoolInputs { inputs } = &entry.location else {
            return Ok(());
        };
        if self.pool_input_edge_count < inputs.len() {
            return Err(CoordinatorError::PoolInputEdgeLimitExceeded);
        }
        for input in inputs {
            if !self
                .pool_waiters_by_input
                .get(input)
                .is_some_and(|waiters| waiters.contains(hash))
            {
                return Err(CoordinatorError::PoolInputEdgeLimitExceeded);
            }
        }
        Ok(())
    }

    pub(super) fn remove_pool_input_indexes(
        &mut self,
        hash: &Byte32,
    ) -> Result<(), CoordinatorError> {
        let inputs = match self.entries.get(hash).map(|entry| &entry.location) {
            Some(CoordinatorLocation::WaitingPoolInputs { inputs }) => inputs.clone(),
            Some(_) => return Ok(()),
            None => return Err(CoordinatorError::Missing(hash.clone())),
        };
        self.pool_input_edge_count = self
            .pool_input_edge_count
            .checked_sub(inputs.len())
            .ok_or(CoordinatorError::PoolInputEdgeLimitExceeded)?;
        for input in inputs {
            if let Some(waiters) = self.pool_waiters_by_input.get_mut(&input) {
                waiters.remove(hash);
                if waiters.is_empty() {
                    self.pool_waiters_by_input.remove(&input);
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
        if let Some(candidate) = &entry.candidate
            && self.conflict_edge_count < candidate.inputs.len()
        {
            return Err(CoordinatorError::ConflictInvariant);
        }
        if let CoordinatorLocation::WaitingConflict { blockers } = &entry.location {
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
                &waiter_entry.location,
                CoordinatorLocation::WaitingConflict { blockers }
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

    pub(super) fn recheck_conflict_candidate(
        &mut self,
        hash: &Byte32,
    ) -> Result<Option<CoordinatorTicket>, CoordinatorError> {
        let entry = self
            .entries
            .get(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        if entry.location != CoordinatorLocation::ConflictRecheck {
            return Err(CoordinatorError::LocationMismatch {
                expected: CoordinatorLocation::ConflictRecheck,
                actual: entry.location.clone(),
            });
        }
        if entry.phase.kind() != PayloadPhase::Verified {
            return Err(CoordinatorError::PhaseMismatch {
                expected: PayloadPhase::Verified,
                actual: entry.phase.kind(),
            });
        }
        self.ensure_revision_capacity(hash)?;
        let candidate = entry
            .candidate
            .as_ref()
            .cloned()
            .ok_or(CoordinatorError::ConflictInvariant)?;
        let blockers = self.active_blockers_for_inputs(hash, &candidate.inputs);
        let can_preempt = !blockers.is_empty()
            && blockers.iter().all(|blocker| {
                self.entries.get(blocker).is_some_and(|blocker_entry| {
                    blocker_entry.location == CoordinatorLocation::ReadyToCommit
                        && blocker_entry
                            .candidate
                            .as_ref()
                            .is_some_and(|blocker_candidate| {
                                Self::compare_candidates(
                                    hash,
                                    &candidate,
                                    blocker,
                                    blocker_candidate,
                                ) == Ordering::Greater
                            })
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

        if can_preempt {
            for blocker in &blockers {
                self.invalidate_conflict_waiters(blocker)?;
                self.remove_current_queue_ticket(blocker)?;
                self.release_conflict_claims(blocker)?;
                let blocker_entry = self
                    .entries
                    .get_mut(blocker)
                    .ok_or_else(|| CoordinatorError::Missing(blocker.clone()))?;
                blocker_entry.location = CoordinatorLocation::WaitingConflict {
                    blockers: HashSet::from([hash.clone()]),
                };
                blocker_entry.revision += 1;
                self.waiters_by_blocker
                    .entry(hash.clone())
                    .or_default()
                    .insert(blocker.clone());
            }
        }

        if blockers.is_empty() || can_preempt {
            let (version, ticket, front) = {
                let entry = self
                    .entries
                    .get_mut(hash)
                    .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
                entry.location = CoordinatorLocation::ReadyToCommit;
                entry.revision += 1;
                (
                    entry.version(),
                    entry.ticket(hash),
                    entry.source.is_proposal(),
                )
            };
            self.claim_conflict_inputs(hash)?;
            self.queue_mut(QueueKind::Commit)?.push_reserved(
                QueueKind::Commit,
                ticket.clone(),
                front,
            )?;
            let _ = version;
            Ok(Some(ticket))
        } else {
            let entry = self
                .entries
                .get_mut(hash)
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            entry.location = CoordinatorLocation::WaitingConflict {
                blockers: blockers.clone(),
            };
            entry.revision += 1;
            for blocker in blockers {
                self.waiters_by_blocker
                    .entry(blocker)
                    .or_default()
                    .insert(hash.clone());
            }
            Ok(None)
        }
    }
}
