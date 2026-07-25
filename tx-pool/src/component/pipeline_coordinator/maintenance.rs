use super::*;

impl<R, U, V> PipelineCoordinator<R, U, V> {
    /// Expiry is incarnation-scoped rather than revision-scoped: ordinary
    /// stage transitions cannot extend a remote transaction's original
    /// lifetime, while removal/re-admission makes the old ticket stale.
    pub(crate) fn expire_due(
        &mut self,
        now: u64,
        max: usize,
    ) -> Result<Vec<TerminalRecord<R>>, CoordinatorError> {
        let capacity = max.min(self.live_deadlines.len());
        let mut selected = Vec::new();
        selected
            .try_reserve(capacity)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        while selected.len() < max {
            let Some(Reverse(ticket)) = self.deadlines.peek().cloned() else {
                break;
            };
            if ticket.expires_at > now {
                break;
            }
            let is_live = self
                .live_deadlines
                .get(&ticket.hash)
                .is_some_and(|live| live == &ticket);
            if !is_live {
                self.deadlines.pop();
                continue;
            }
            let entry = self
                .entries
                .get(&ticket.hash)
                .ok_or_else(|| CoordinatorError::Missing(ticket.hash.clone()))?;
            if entry.incarnation != ticket.incarnation
                || entry.expires_at != Some(ticket.expires_at)
            {
                return Err(CoordinatorError::ConflictInvariant);
            }
            if matches!(
                &entry.state,
                EntryState::CandidateVerified {
                    location: CandidateLocation::Committing,
                    ..
                }
            ) {
                break;
            }
            self.deadlines.pop();
            selected.push(ticket);
        }
        // Restore the selected physical tickets before any fallible undo
        // preparation. Successful removal makes them lazy-stale; rollback can
        // therefore rebuild only logical state without a deadline liveness gap.
        for ticket in &selected {
            self.deadlines.push(Reverse(ticket.clone()));
        }
        let roots: Vec<_> = selected.iter().map(|ticket| ticket.hash.clone()).collect();
        let affected = self.causal_undo_hashes(&roots);
        let mut terminal = Vec::new();
        terminal
            .try_reserve(selected.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.with_entry_undo(&affected, |coordinator| {
            for ticket in selected {
                coordinator.mark_children_invalid(&ticket.hash, &ticket.hash)?;
                let entry = coordinator.remove_present_apply(&ticket.hash)?;
                terminal.push(Self::terminal_record(
                    ticket.hash,
                    entry,
                    TerminalDisposition::Expired,
                ));
                coordinator.apply_fault_checkpoint();
            }
            Ok(terminal)
        })
    }

    pub(crate) fn clear(&mut self) -> Result<Vec<TerminalRecord<R>>, CoordinatorError> {
        // Clear is one ownership transaction, not N conflict removals. It must
        // not wake/revise records that are themselves being cleared, and stale
        // worker leases become harmless because re-admission receives a new
        // incarnation.
        let mut terminal = Vec::new();
        terminal
            .try_reserve(self.entries.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.apply_fault_checkpoint();
        let entries = std::mem::take(&mut self.entries);
        for (hash, entry) in entries {
            terminal.push(Self::terminal_record(
                hash,
                entry,
                TerminalDisposition::Cleared,
            ));
        }
        self.by_short_id.clear();
        self.by_peer.clear();
        self.by_parent.clear();
        self.waiting_parent_count = 0;
        self.dependency_failures.clear();
        self.dependency_failure_set.clear();
        self.conflicts.clear();
        self.deadlines.clear();
        self.live_deadlines.clear();
        self.capacity_victim_index.clear();
        self.candidate_victim_index.clear();
        for queue in self.queues.values_mut() {
            queue.clear();
        }
        self.global_usage = CoordinatorResidency::default();
        self.peer_usage.clear();
        self.active_work = 0;
        self.active_work_by_peer.clear();
        Ok(terminal)
    }
}

#[cfg(test)]
impl<R, U, V> PipelineCoordinator<R, U, V> {
    pub(crate) fn set_revision_for_test(
        &mut self,
        hash: &Byte32,
        revision: u64,
    ) -> Result<(), CoordinatorError> {
        let entry = self.entry_mut(hash)?;
        let old_ticket = entry.ticket(hash);
        entry.revision = revision;
        if let Some(kind) = entry.queue_kind() {
            let new_ticket = entry.ticket(hash);
            let front = entry.source.is_proposal();
            let owner = entry.source.queue_owner();
            let is_large_cycle = new_ticket.verify_schedule.is_large_cycle;
            let queue = self.queue_mut(kind)?;
            queue.remove_live(kind, &old_ticket)?;
            queue.reserve_live(owner, is_large_cycle)?;
            queue.push_reserved(kind, new_ticket, front)?;
        }
        Ok(())
    }

    pub(crate) fn mutate_outside_undo_cohort_for_test(
        &mut self,
        snapshotted: &Byte32,
        escaped: &Byte32,
    ) -> Result<(), CoordinatorError> {
        self.with_entry_undo(std::slice::from_ref(snapshotted), |coordinator| {
            coordinator.entry_mut(escaped)?.revision += 1;
            Ok(())
        })
    }

    pub(crate) fn expand_nested_undo_cohort_for_test(
        &mut self,
        outer: &Byte32,
        escaped: &Byte32,
    ) -> Result<(), CoordinatorError> {
        self.with_entry_undo(std::slice::from_ref(outer), |coordinator| {
            coordinator.with_entry_undo(std::slice::from_ref(escaped), |_| Ok(()))
        })
    }

    pub(crate) fn set_next_maintenance_sequence_for_test(&mut self, sequence: u64) {
        self.next_maintenance_sequence = sequence;
    }

    pub(crate) fn set_next_queue_sequence_for_test(&mut self, sequence: u64) {
        self.next_queue_sequence = sequence;
    }

    pub(crate) fn physical_queue_slots_for_test(&self, kind: QueueKind) -> usize {
        self.queues.get(&kind).map_or(0, TicketQueue::physical_len)
    }

    pub(crate) fn take_queue_selection_probes_for_test(&mut self, kind: QueueKind) -> usize {
        self.queues
            .get_mut(&kind)
            .map_or(0, TicketQueue::take_selection_probes)
    }

    pub(crate) fn take_capacity_victim_probes_for_test(&self) -> usize {
        self.capacity_victim_probes.replace(0)
    }

    pub(crate) fn take_candidate_victim_probes_for_test(&self) -> usize {
        self.candidate_victim_probes.replace(0)
    }

    pub(crate) fn physical_deadline_slots_for_test(&self) -> usize {
        self.deadlines.len()
    }

    pub(crate) fn set_apply_fault_for_test(&mut self, after: Option<usize>) {
        self.fault_after_apply_steps = after;
        self.apply_steps_seen = 0;
    }

    pub(crate) fn fail_next_handoff_after_apply_for_test(&mut self, error: CoordinatorError) {
        self.fail_next_handoff_after_apply = Some(error);
    }

    pub(super) fn handoff_error_checkpoint(&mut self) -> Result<(), CoordinatorError> {
        self.fail_next_handoff_after_apply
            .take()
            .map_or(Ok(()), Err)
    }

    pub(super) fn apply_fault_checkpoint(&mut self) {
        self.apply_steps_seen = self.apply_steps_seen.saturating_add(1);
        if self.fault_after_apply_steps == Some(self.apply_steps_seen) {
            std::panic::panic_any("injected coordinator apply fault");
        }
    }
}

#[cfg(not(test))]
impl<R, U, V> PipelineCoordinator<R, U, V> {
    #[inline(always)]
    pub(super) fn apply_fault_checkpoint(&mut self) {}
}
