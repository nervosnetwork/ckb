use super::*;

impl<R, U, V> PipelineCoordinator<R, U, V> {
    /// Whether one worker capability can make an immediate checkout from the
    /// authoritative scheduling state.
    ///
    /// Runtime notifications are derived from this predicate rather than
    /// queue non-emptiness. A queue can be non-empty but temporarily
    /// ineligible because its owners exhausted the global/per-peer active
    /// budget, and the verify queue additionally has capability-specific
    /// small-cycle eligibility.
    pub(crate) fn work_is_ready(
        &mut self,
        kind: QueueKind,
        capability: WorkerCapability,
    ) -> Result<bool, CoordinatorError> {
        if kind != QueueKind::Verify && capability != WorkerCapability::Any {
            return Err(CoordinatorError::QueueInvariant(kind));
        }
        if kind == QueueKind::Commit && self.conflicts.committing.is_some() {
            return Ok(false);
        }
        self.peek_live_ticket(kind, capability)
            .map(|ticket| ticket.is_some())
    }

    pub(super) fn queue_mut(&mut self, kind: QueueKind) -> &mut TicketQueue {
        &mut self.queues[kind.index()]
    }

    pub(super) fn peek_live_ticket(
        &mut self,
        kind: QueueKind,
        capability: WorkerCapability,
    ) -> Result<Option<CoordinatorTicket>, CoordinatorError> {
        if self.active_work >= self.limits.max_active_work {
            return Ok(None);
        }
        let per_peer_limit = self.limits.max_active_work_per_peer;
        let active_by_peer = &self.active_work_by_peer;
        let queue = &mut self.queues[kind.index()];
        Ok(queue.peek_eligible(capability, |owner| match owner {
            QueueOwner::Trusted => true,
            QueueOwner::Remote(peer) => {
                active_by_peer.get(&peer).copied().unwrap_or(0) < per_peer_limit
            }
        }))
    }

    pub(super) fn consume_front_ticket(
        &mut self,
        kind: QueueKind,
        ticket: &CoordinatorTicket,
    ) -> Result<(), CoordinatorError> {
        let queue = self.queue_mut(kind);
        queue.consume(kind, ticket)
    }

    pub(super) fn remove_current_queue_ticket(
        &mut self,
        hash: &Byte32,
    ) -> Result<(), CoordinatorError> {
        let Some(entry) = self.entries.get(hash) else {
            return Err(CoordinatorError::Missing(hash.clone()));
        };
        let Some(kind) = self.candidate_queue_kind(hash, entry) else {
            return Ok(());
        };
        let ticket = entry.ticket(hash);
        let queue = self.queue_mut(kind);
        queue.remove_live(kind, &ticket)?;
        queue.compact();
        Ok(())
    }

    /// Remove whichever scheduling projection the authoritative entry owns.
    /// Candidate tickets are part of the conflict delta and must not be
    /// removed separately, otherwise eligibility reconciliation observes a
    /// false missing-ticket invariant.
    pub(super) fn remove_current_scheduling(
        &mut self,
        hash: &Byte32,
    ) -> Result<(), CoordinatorError> {
        if self
            .entries
            .get(hash)
            .and_then(CoordinatorEntry::candidate)
            .is_some()
        {
            self.remove_conflict_indexes(hash)
        } else {
            self.remove_current_queue_ticket(hash)
        }
    }

    pub(super) fn validate_version_location(
        &self,
        hash: &Byte32,
        version: CoordinatorVersion,
        expected_location: &CoordinatorLocation,
    ) -> Result<(), CoordinatorError> {
        let entry = self
            .entries
            .get(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        if entry.incarnation != version.incarnation {
            return Err(CoordinatorError::IncarnationMismatch {
                expected: version.incarnation,
                actual: entry.incarnation,
            });
        }
        if entry.revision != version.revision {
            return Err(CoordinatorError::RevisionMismatch {
                expected: version.revision,
                actual: entry.revision,
            });
        }
        if let Some(parent) = entry.dependencies.iter().find(|parent| {
            self.entries
                .get(*parent)
                .is_some_and(|parent_entry| parent_entry.invalidated_cause().is_some())
        }) {
            return Err(CoordinatorError::DependencyInvalidated {
                child: hash.clone(),
                parent: parent.clone(),
            });
        }
        if !entry.state.location_matches(expected_location) {
            return Err(CoordinatorError::LocationMismatch {
                expected: expected_location.clone(),
                actual: entry.location(),
            });
        }
        Ok(())
    }

    pub(super) fn ensure_revision_capacity(&self, hash: &Byte32) -> Result<(), CoordinatorError> {
        let entry = self
            .entries
            .get(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        if entry.revision == u64::MAX {
            return Err(CoordinatorError::RevisionExhausted(hash.clone()));
        }
        Ok(())
    }

    pub(super) fn maintenance_sequence_range(
        &self,
        count: usize,
    ) -> Result<(u64, u64), CoordinatorError> {
        let count =
            u64::try_from(count).map_err(|_| CoordinatorError::MaintenanceSequenceExhausted)?;
        let first = self.next_maintenance_sequence;
        let next = first
            .checked_add(count)
            .ok_or(CoordinatorError::MaintenanceSequenceExhausted)?;
        Ok((first, next))
    }

    pub(super) fn queue_sequence_range(
        &self,
        count: usize,
    ) -> Result<(u64, u64), CoordinatorError> {
        let count = u64::try_from(count).map_err(|_| CoordinatorError::QueueSequenceExhausted)?;
        let first = self.next_queue_sequence;
        let next = first
            .checked_add(count)
            .ok_or(CoordinatorError::QueueSequenceExhausted)?;
        Ok((first, next))
    }
}
