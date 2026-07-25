use super::*;

impl<R, U, V> PipelineCoordinator<R, U, V> {
    pub(super) fn with_entry_undo<T, F>(
        &mut self,
        hashes: &[Byte32],
        apply: F,
    ) -> Result<T, CoordinatorError>
    where
        F: FnOnce(&mut Self) -> Result<T, CoordinatorError>,
    {
        let mut unique = HashSet::new();
        let mut snapshot = Vec::new();
        for hash in hashes {
            if unique.contains(hash) {
                continue;
            }
            unique
                .try_reserve(1)
                .map_err(|_| CoordinatorError::QueueReservationFailed)?;
            snapshot
                .try_reserve(1)
                .map_err(|_| CoordinatorError::QueueReservationFailed)?;
            unique.insert(hash.clone());
            let entry = self
                .entries
                .get(hash)
                .cloned()
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            snapshot.push((hash.clone(), Some(entry)));
        }
        self.with_entry_snapshot(snapshot, unique, apply)
    }

    pub(super) fn with_absent_entry_undo<T, F>(
        &mut self,
        absent: &Byte32,
        hashes: &[Byte32],
        apply: F,
    ) -> Result<T, CoordinatorError>
    where
        F: FnOnce(&mut Self) -> Result<T, CoordinatorError>,
    {
        if self.entries.contains_key(absent) {
            return Err(CoordinatorError::DuplicateHash(absent.clone()));
        }
        let mut unique = HashSet::new();
        unique
            .try_reserve(1)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        let mut snapshot = Vec::new();
        snapshot
            .try_reserve(1)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        unique.insert(absent.clone());
        snapshot.push((absent.clone(), None));
        for hash in hashes {
            if unique.contains(hash) {
                continue;
            }
            unique
                .try_reserve(1)
                .map_err(|_| CoordinatorError::QueueReservationFailed)?;
            snapshot
                .try_reserve(1)
                .map_err(|_| CoordinatorError::QueueReservationFailed)?;
            unique.insert(hash.clone());
            let entry = self
                .entries
                .get(hash)
                .cloned()
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            snapshot.push((hash.clone(), Some(entry)));
        }
        self.with_entry_snapshot(snapshot, unique, apply)
    }

    /// Snapshot the current presence/absence of an arbitrary bounded cohort.
    /// Reorg membership deltas legitimately mix coordinator-resident and
    /// never-admitted hashes, so neither `with_entry_undo` nor
    /// `with_absent_entry_undo` can express the transaction alone.
    pub(super) fn with_mixed_entry_undo<T, F>(
        &mut self,
        hashes: &[Byte32],
        apply: F,
    ) -> Result<T, CoordinatorError>
    where
        F: FnOnce(&mut Self) -> Result<T, CoordinatorError>,
    {
        let mut unique = HashSet::new();
        let mut snapshot = Vec::new();
        for hash in hashes {
            if unique.contains(hash) {
                continue;
            }
            unique
                .try_reserve(1)
                .map_err(|_| CoordinatorError::QueueReservationFailed)?;
            snapshot
                .try_reserve(1)
                .map_err(|_| CoordinatorError::QueueReservationFailed)?;
            unique.insert(hash.clone());
            snapshot.push((hash.clone(), self.entries.get(hash).cloned()));
        }
        self.with_entry_snapshot(snapshot, unique, apply)
    }

    pub(super) fn with_entry_snapshot<T, F>(
        &mut self,
        snapshot: Vec<EntrySnapshot<R, U, V>>,
        cohort: HashSet<Byte32>,
        apply: F,
    ) -> Result<T, CoordinatorError>
    where
        F: FnOnce(&mut Self) -> Result<T, CoordinatorError>,
    {
        let next_incarnation = self.next_incarnation;
        let next_arrival = self.next_arrival;
        let next_maintenance_sequence = self.next_maintenance_sequence;
        let next_queue_sequence = self.next_queue_sequence;
        let outermost = self.entry_transaction_depth == 0;
        self.begin_entry_transaction(&cohort)?;
        let outcome = catch_unwind(AssertUnwindSafe(|| apply(self)));
        self.end_entry_transaction(&cohort);
        let restore_or_panic = |coordinator: &mut Self, snapshot| {
            if let Err(error) = coordinator.restore_entry_snapshot(
                snapshot,
                next_incarnation,
                next_arrival,
                next_maintenance_sequence,
                next_queue_sequence,
            ) {
                std::panic::panic_any(error);
            }
        };
        match outcome {
            Ok(Ok(value)) => {
                if outermost {
                    self.sync_victim_indexes(&snapshot);
                }
                Ok(value)
            }
            Ok(Err(error)) => {
                restore_or_panic(self, snapshot);
                Err(error)
            }
            Err(payload) => {
                restore_or_panic(self, snapshot);
                resume_unwind(payload)
            }
        }
    }

    pub(super) fn begin_entry_transaction(
        &mut self,
        cohort: &HashSet<Byte32>,
    ) -> Result<(), CoordinatorError> {
        let depth = self.entry_transaction_depth;
        // A nested snapshot may narrow an outer cohort, but it cannot add an
        // entry the outer transaction would be unable to restore.
        if let Some(hash) = cohort.iter().find(|hash| {
            self.entry_transaction_membership
                .get(*hash)
                .copied()
                .unwrap_or(0)
                != depth
        }) {
            return Err(CoordinatorError::UndoCohortViolation {
                hash: hash.clone(),
                active_depth: depth,
                snapshotted_depth: self
                    .entry_transaction_membership
                    .get(hash)
                    .copied()
                    .unwrap_or(0),
                mutation_file: "nested undo cohort",
                mutation_line: 0,
                active_members: self
                    .entry_transaction_membership
                    .iter()
                    .filter(|(_, count)| **count == depth)
                    .map(|(hash, _)| hash.clone())
                    .collect(),
            });
        }
        self.entry_transaction_membership
            .try_reserve(cohort.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        let next_depth = depth
            .checked_add(1)
            .expect("coordinator undo nesting is statically bounded");
        for hash in cohort {
            self.entry_transaction_membership
                .insert(hash.clone(), next_depth);
        }
        self.entry_transaction_depth = next_depth;
        Ok(())
    }

    pub(super) fn end_entry_transaction(&mut self, cohort: &HashSet<Byte32>) {
        let depth = self.entry_transaction_depth;
        debug_assert_ne!(depth, 0);
        for hash in cohort {
            let remove = {
                let count = self
                    .entry_transaction_membership
                    .get_mut(hash)
                    .expect("active undo cohort membership exists");
                assert_eq!(*count, depth, "undo cohort nesting remains exact");
                *count -= 1;
                *count == 0
            };
            if remove {
                self.entry_transaction_membership.remove(hash);
            }
        }
        self.entry_transaction_depth = depth - 1;
    }

    #[track_caller]
    pub(super) fn ensure_entry_mutation_is_snapshotted(
        &self,
        hash: &Byte32,
    ) -> Result<(), CoordinatorError> {
        if self.entry_transaction_depth != 0
            && self.entry_transaction_membership.get(hash).copied()
                != Some(self.entry_transaction_depth)
        {
            let caller = std::panic::Location::caller();
            return Err(CoordinatorError::UndoCohortViolation {
                hash: hash.clone(),
                active_depth: self.entry_transaction_depth,
                snapshotted_depth: self
                    .entry_transaction_membership
                    .get(hash)
                    .copied()
                    .unwrap_or(0),
                mutation_file: caller.file(),
                mutation_line: caller.line(),
                active_members: self
                    .entry_transaction_membership
                    .iter()
                    .filter(|(_, count)| **count == self.entry_transaction_depth)
                    .map(|(hash, _)| hash.clone())
                    .collect(),
            });
        }
        Ok(())
    }

    #[track_caller]
    pub(super) fn entry_mut(
        &mut self,
        hash: &Byte32,
    ) -> Result<&mut CoordinatorEntry<R, U, V>, CoordinatorError> {
        self.ensure_entry_mutation_is_snapshotted(hash)?;
        self.entries
            .get_mut(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))
    }

    pub(super) fn insert_absent_entry(
        &mut self,
        hash: Byte32,
        entry: CoordinatorEntry<R, U, V>,
    ) -> Result<(), CoordinatorError> {
        self.ensure_entry_mutation_is_snapshotted(&hash)?;
        if self.entries.insert(hash.clone(), entry).is_some() {
            return Err(CoordinatorError::DuplicateHash(hash));
        }
        Ok(())
    }

    pub(super) fn remove_present_entry(
        &mut self,
        hash: &Byte32,
    ) -> Result<CoordinatorEntry<R, U, V>, CoordinatorError> {
        self.ensure_entry_mutation_is_snapshotted(hash)?;
        self.entries
            .remove(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))
    }

    pub(super) fn restore_entry_snapshot(
        &mut self,
        snapshot: Vec<EntrySnapshot<R, U, V>>,
        next_incarnation: u64,
        next_arrival: u64,
        next_maintenance_sequence: u64,
        next_queue_sequence: u64,
    ) -> Result<(), CoordinatorError> {
        for (hash, entry) in snapshot {
            if let Some(entry) = entry {
                self.entries.insert(hash, entry);
            } else {
                self.entries.remove(&hash);
            }
        }
        self.next_incarnation = next_incarnation;
        self.next_arrival = next_arrival;
        self.next_maintenance_sequence = next_maintenance_sequence;
        self.next_queue_sequence = next_queue_sequence;
        self.rebuild_derived_indexes()
    }

    /// Convert one live owner into raw-only terminal maintenance work. Typed
    /// resolved/verified payloads have no consumer after definitive
    /// dependency failure: retaining them would preserve a dead phase and let
    /// an attacker pin its larger residency until the bounded cascade drains.
    pub(super) fn invalidate_present_apply(
        &mut self,
        hash: &Byte32,
        cause: &Byte32,
        sequence: u64,
    ) -> Result<(), CoordinatorError> {
        let (active_source, raw_charge, was_waiting, already_invalidated) = self
            .entries
            .get(hash)
            .map(|entry| {
                (
                    entry.uses_active_slot().then_some(entry.source),
                    entry.raw_charge_bytes,
                    matches!(
                        &entry.state,
                        EntryState::Raw {
                            location: RawLocation::WaitingParents { .. },
                            ..
                        }
                    ),
                    entry.invalidated_cause().is_some(),
                )
            })
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        if already_invalidated {
            return Err(CoordinatorError::ConflictInvariant);
        }
        if let Some(source) = active_source {
            self.deactivate_source(source)?;
        }
        self.remove_current_scheduling(hash)?;
        self.apply_fault_checkpoint();
        self.apply_recharge(hash, raw_charge)?;
        let entry = self.entry_mut(hash)?;
        let raw = Arc::clone(entry.state.raw());
        entry.state = EntryState::Invalidated {
            raw,
            cause: cause.clone(),
            sequence,
        };
        entry.resident_payload_bytes = entry.raw_resident_payload_bytes;
        entry.metadata_bytes = entry.base_metadata_bytes;
        entry.revision += 1;
        if was_waiting {
            self.leave_waiting_parent()?;
        }
        if !self.dependency_failure_set.insert(hash.clone()) {
            return Err(CoordinatorError::ConflictInvariant);
        }
        self.dependency_failures.push_back(hash.clone());
        self.apply_fault_checkpoint();
        Ok(())
    }

    pub(super) fn mark_children_invalid(
        &mut self,
        parent: &Byte32,
        cause: &Byte32,
    ) -> Result<Vec<Byte32>, CoordinatorError> {
        let mut children: Vec<_> = self
            .by_parent
            .get(parent)
            .into_iter()
            .flat_map(|children| children.iter().cloned())
            .collect();
        children.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        children.retain(|child| {
            self.entries
                .get(child)
                .is_some_and(|entry| entry.invalidated_cause().is_none())
        });
        for child in &children {
            let (uses_active_slot, source, raw_charge) = {
                let entry = self
                    .entries
                    .get(child)
                    .ok_or_else(|| CoordinatorError::Missing(child.clone()))?;
                (
                    entry.uses_active_slot(),
                    entry.source,
                    entry.raw_charge_bytes,
                )
            };
            self.ensure_revision_capacity(child)?;
            if uses_active_slot
                && (self.active_work == 0
                    || source
                        .peer()
                        .is_some_and(|peer| self.peer_active_work(peer) == 0))
            {
                return Err(CoordinatorError::ActiveWorkLimitExceeded);
            }
            self.preflight_remove_conflict_indexes(child)?;
            self.check_recharge(child, raw_charge)?;
        }
        self.dependency_failures
            .try_reserve(children.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.dependency_failure_set
            .try_reserve(children.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        let (first_sequence, next_maintenance_sequence) =
            self.maintenance_sequence_range(children.len())?;
        let undo_hashes = self.conflict_undo_hashes(&children);
        let result = children.clone();
        self.with_entry_undo(&undo_hashes, |coordinator| {
            coordinator.next_maintenance_sequence = next_maintenance_sequence;
            for (offset, child) in children.iter().enumerate() {
                let sequence = first_sequence
                    .checked_add(
                        u64::try_from(offset)
                            .map_err(|_| CoordinatorError::MaintenanceSequenceExhausted)?,
                    )
                    .ok_or(CoordinatorError::MaintenanceSequenceExhausted)?;
                coordinator.invalidate_present_apply(child, cause, sequence)?;
            }
            Ok(result)
        })
    }

    pub(super) fn causal_undo_hashes(&self, roots: &[Byte32]) -> Vec<Byte32> {
        let mut affected: HashSet<_> = roots.iter().cloned().collect();
        for root in roots {
            if let Some(children) = self.by_parent.get(root) {
                affected.extend(children.iter().cloned());
            }
        }
        let affected: Vec<_> = affected.into_iter().collect();
        self.conflict_undo_hashes(&affected)
    }

    /// Remove one already-validated owner and invalidate its direct children
    /// under the same causal undo cohort. Lease-scoped raw, verify and commit
    /// exits plus hash-scoped administration all converge here so no terminal
    /// path can forget dependency invalidation or use a narrower rollback
    /// boundary.
    pub(super) fn terminalize_present_causally(
        &mut self,
        hash: &Byte32,
        disposition: TerminalDisposition,
    ) -> Result<TerminalRecord<R>, CoordinatorError> {
        let undo = self.causal_undo_hashes(std::slice::from_ref(hash));
        let hash = hash.clone();
        self.with_entry_undo(&undo, move |coordinator| {
            coordinator.terminalize_present_apply(hash, None, disposition)
        })
    }

    /// Apply one definitive exit inside the caller's single/batch undo scope.
    pub(super) fn terminalize_present_apply(
        &mut self,
        hash: Byte32,
        descendant_cause: Option<Byte32>,
        disposition: TerminalDisposition,
    ) -> Result<TerminalRecord<R>, CoordinatorError> {
        self.mark_children_invalid(&hash, descendant_cause.as_ref().unwrap_or(&hash))?;
        let entry = self.remove_present_apply(&hash)?;
        self.apply_fault_checkpoint();
        Ok(Self::terminal_record(hash, entry, disposition))
    }

    pub(super) fn conflict_undo_hashes(&self, roots: &[Byte32]) -> Vec<Byte32> {
        let mut affected: HashSet<_> = roots.iter().cloned().collect();
        // Removing a candidate can change commit eligibility only for its
        // direct staged neighbours; no transitive conflict closure is needed.
        for hash in roots {
            if let Some(candidate) = self.entries.get(hash).and_then(CoordinatorEntry::candidate) {
                for input in &candidate.inputs {
                    if let Some(neighbours) = self.conflicts.by_input.get(input) {
                        affected.extend(neighbours.iter().cloned());
                    }
                }
            }
        }
        let mut affected: Vec<_> = affected.into_iter().collect();
        affected.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        affected
    }

    pub(super) fn remove_present_apply(
        &mut self,
        hash: &Byte32,
    ) -> Result<CoordinatorEntry<R, U, V>, CoordinatorError> {
        let active_source = self
            .entries
            .get(hash)
            .and_then(|entry| entry.uses_active_slot().then_some(entry.source));
        if let Some(source) = active_source {
            let peer_slot_missing = source
                .peer()
                .is_some_and(|peer| self.peer_active_work(peer) == 0);
            if self.active_work == 0 || peer_slot_missing {
                return Err(CoordinatorError::ActiveWorkLimitExceeded);
            }
        }
        self.preflight_remove_conflict_indexes(hash)?;
        self.remove_current_scheduling(hash)?;
        self.apply_fault_checkpoint();
        self.remove_present_after_conflicts_apply(hash, active_source)
    }

    /// Remove an owner after its queue membership and staged-conflict delta
    /// have already been applied. Commit handoff uses this to remove winner
    /// plus direct losers with one graph delta instead of K sequential
    /// eligibility oscillations.
    pub(super) fn remove_present_after_conflicts_apply(
        &mut self,
        hash: &Byte32,
        active_source: Option<CoordinatorSource>,
    ) -> Result<CoordinatorEntry<R, U, V>, CoordinatorError> {
        if let Some(source) = active_source {
            self.deactivate_source(source)?;
        }
        let was_waiting = self.entries.get(hash).is_some_and(|entry| {
            matches!(
                &entry.state,
                EntryState::Raw {
                    location: RawLocation::WaitingParents { .. },
                    ..
                }
            )
        });
        let entry = self.remove_present_entry(hash)?;
        if was_waiting {
            self.leave_waiting_parent()?;
        }
        let charge = CoordinatorResidency::new(1, entry.charge_bytes);
        self.global_usage = self
            .global_usage
            .checked_sub(charge)
            .ok_or(CoordinatorError::GlobalBudgetExceeded)?;
        self.by_short_id.remove(&entry.short_id);
        if let Some(peer) = entry.source.peer() {
            let remove_usage = {
                let usage = self
                    .peer_usage
                    .get_mut(&peer)
                    .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
                *usage = usage
                    .checked_sub(charge)
                    .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
                *usage == CoordinatorResidency::default()
            };
            if remove_usage {
                self.peer_usage.remove(&peer);
            }
            if let Some(hashes) = self.by_peer.get_mut(&peer) {
                hashes.remove(hash);
                if hashes.is_empty() {
                    self.by_peer.remove(&peer);
                }
            }
        }
        for parent in &entry.dependencies {
            if let Some(children) = self.by_parent.get_mut(parent) {
                children.remove(hash);
                if children.is_empty() {
                    self.by_parent.remove(parent);
                }
            }
        }
        self.live_deadlines.remove(hash);
        self.compact_deadlines();
        self.dependency_failure_set.remove(hash);
        self.compact_dependency_failures();
        self.apply_fault_checkpoint();
        Ok(entry)
    }

    pub(super) fn compact_dependency_failures(&mut self) {
        if self.dependency_failures.len()
            > lazy_ticket_compaction_limit(self.dependency_failure_set.len())
        {
            self.dependency_failures
                .retain(|hash| self.dependency_failure_set.contains(hash));
        }
    }

    pub(super) fn preview_dependency_failure_roots(&self, max: usize) -> Vec<Byte32> {
        // `mark_children_invalid` appends the next causal frontier while the
        // selected roots are applied. Previewing only the already-live FIFO
        // frontier keeps every maintenance turn O(max); cloning the complete
        // backlog here made a fixed-size drain quadratic in backlog length.
        self.dependency_failures
            .iter()
            .filter(|hash| self.dependency_failure_set.contains(*hash))
            .take(max)
            .cloned()
            .collect()
    }

    pub(super) fn compact_deadlines(&mut self) {
        if self.deadlines.len() > lazy_ticket_compaction_limit(self.live_deadlines.len()) {
            self.deadlines.retain(|Reverse(ticket)| {
                self.live_deadlines
                    .get(&ticket.hash)
                    .is_some_and(|live| live == ticket)
            });
        }
    }

    pub(super) fn terminal_record(
        hash: Byte32,
        entry: CoordinatorEntry<R, U, V>,
        disposition: TerminalDisposition,
    ) -> TerminalRecord<R> {
        let raw = Arc::clone(entry.state.raw());
        #[cfg(not(test))]
        let _ = disposition;
        TerminalRecord {
            hash,
            raw,
            source: entry.source,
            #[cfg(test)]
            disposition,
        }
    }
}
