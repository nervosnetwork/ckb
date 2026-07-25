use super::*;

#[cfg(test)]
#[path = "../tests/pipeline_coordinator_commit_seam.rs"]
mod test_seam;

impl<R, U, V> PipelineCoordinator<R, U, V> {
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
}
