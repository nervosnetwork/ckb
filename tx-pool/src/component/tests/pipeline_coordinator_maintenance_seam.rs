use super::*;

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
            let queue = self.queue_mut(kind);
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
        self.queues[kind.index()].physical_len()
    }

    pub(crate) fn take_queue_selection_probes_for_test(&mut self, kind: QueueKind) -> usize {
        self.queues[kind.index()].take_selection_probes()
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

    pub(in crate::component::pipeline_coordinator) fn handoff_error_checkpoint(
        &mut self,
    ) -> Result<(), CoordinatorError> {
        self.fail_next_handoff_after_apply
            .take()
            .map_or(Ok(()), Err)
    }

    pub(in crate::component::pipeline_coordinator) fn apply_fault_checkpoint(&mut self) {
        self.apply_steps_seen = self.apply_steps_seen.saturating_add(1);
        if self.fault_after_apply_steps == Some(self.apply_steps_seen) {
            std::panic::panic_any("injected coordinator apply fault");
        }
    }
}
