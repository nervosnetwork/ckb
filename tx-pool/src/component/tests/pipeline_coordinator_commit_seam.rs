use super::*;

impl<R, U, V> PipelineCoordinator<R, U, V> {
    pub(crate) fn commit_candidate_handoff(
        &mut self,
        lease: &CommitLease<V>,
    ) -> Result<ConflictCommitHandoff<R>, CoordinatorError> {
        let undo = self.commit_handoff_undo_hashes(lease)?;
        self.with_entry_undo(&undo, |coordinator| {
            coordinator.commit_candidate_handoff_apply(lease)
        })
    }

    pub(crate) fn abort_commit(
        &mut self,
        lease: &CommitLease<V>,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        self.validate_version_location(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::Committing,
        )?;
        let old_victim_keys = self.current_victim_keys(&lease.hash);
        self.ensure_revision_capacity(&lease.hash)?;
        let (source, candidate) = self
            .entries
            .get(&lease.hash)
            .and_then(|entry| {
                entry
                    .candidate()
                    .cloned()
                    .map(|candidate| (entry.source, candidate))
            })
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        let next_rank = CandidateRank::verified(&lease.hash, source, &candidate);
        let delta =
            self.preview_conflict_rerank(&lease.hash, &next_rank, CandidateLocation::Verified)?;
        let deadline = self.entries.get(&lease.hash).and_then(|entry| {
            entry.expires_at.map(|expires_at| DeadlineTicket {
                expires_at,
                hash: lease.hash.clone(),
                incarnation: entry.incarnation,
                generation: entry.deadline_generation,
            })
        });
        if deadline.is_some() {
            self.deadlines
                .try_reserve(1)
                .map_err(|_| CoordinatorError::QueueReservationFailed)?;
            self.live_deadlines
                .try_reserve(1)
                .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        }
        let undo = delta.affected().to_vec();
        let version = self.with_entry_undo(&undo, |coordinator| {
            let ticket_plan = coordinator.prepare_conflict_ticket_plan(
                &delta,
                &HashSet::new(),
                &HashMap::new(),
            )?;
            coordinator.remove_conflict_tickets(&ticket_plan)?;
            coordinator.deactivate_source(source)?;
            let entry = coordinator.entry_mut(&lease.hash)?;
            match &mut entry.state {
                EntryState::CandidateVerified { location, .. } => {
                    *location = CandidateLocation::Verified;
                }
                _ => return Err(CoordinatorError::ConflictInvariant),
            }
            coordinator.apply_conflict_delta(&delta)?;
            if !ticket_plan.revises(&lease.hash) {
                coordinator.entry_mut(&lease.hash)?.revision += 1;
            }
            coordinator.apply_conflict_ticket_plan(ticket_plan)?;
            if let Some(deadline) = deadline {
                coordinator.deadlines.push(Reverse(deadline.clone()));
                coordinator
                    .live_deadlines
                    .insert(lease.hash.clone(), deadline);
                coordinator.compact_deadlines();
            }
            coordinator
                .entries
                .get(&lease.hash)
                .map(CoordinatorEntry::version)
                .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))
        })?;
        self.refresh_victim_indexes(&lease.hash, old_victim_keys);
        Ok(version)
    }

    /// Consume a transaction that became committed through an attached block
    /// rather than this coordinator's submit path. Chain membership is
    /// authoritative: dependents are woken, not invalidated, and every stale
    /// worker/commit lease becomes harmless when the entry is removed.
    pub(crate) fn external_commit(
        &mut self,
        hash: &Byte32,
    ) -> Result<Option<ExternalCommitRecord<R>>, CoordinatorError> {
        let mut undo = self.causal_undo_hashes(std::slice::from_ref(hash));
        if self.entries.contains_key(hash) {
            self.with_entry_undo(&undo, |coordinator| coordinator.external_commit_apply(hash))
        } else {
            undo.retain(|affected| affected != hash);
            self.with_absent_entry_undo(hash, &undo, |coordinator| {
                coordinator.external_commit_apply(hash)
            })
        }
    }
}
