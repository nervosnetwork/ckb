use super::*;

enum SnapshotMembership<'a> {
    Present,
    Absent(&'a Byte32),
    Mixed,
}

type PreparedEntrySnapshot<R, U, V> = (Vec<EntrySnapshot<R, U, V>>, HashSet<Byte32>);

impl<R, U, V> PipelineCoordinator<R, U, V> {
    pub(super) fn with_entry_undo<T, F>(
        &mut self,
        hashes: &[Byte32],
        apply: F,
    ) -> Result<T, CoordinatorError>
    where
        F: FnOnce(&mut Self) -> Result<T, CoordinatorError>,
    {
        let (snapshot, cohort) =
            self.prepare_entry_snapshot(hashes, SnapshotMembership::Present)?;
        self.with_entry_snapshot(snapshot, cohort, apply)
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
        let (snapshot, cohort) =
            self.prepare_entry_snapshot(hashes, SnapshotMembership::Absent(absent))?;
        self.with_entry_snapshot(snapshot, cohort, apply)
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
        let (snapshot, cohort) = self.prepare_entry_snapshot(hashes, SnapshotMembership::Mixed)?;
        self.with_entry_snapshot(snapshot, cohort, apply)
    }

    fn prepare_entry_snapshot(
        &self,
        hashes: &[Byte32],
        membership: SnapshotMembership<'_>,
    ) -> Result<PreparedEntrySnapshot<R, U, V>, CoordinatorError> {
        let absent = match membership {
            SnapshotMembership::Absent(hash) => {
                if self.entries.contains_key(hash) {
                    return Err(CoordinatorError::DuplicateHash(hash.clone()));
                }
                Some(hash)
            }
            SnapshotMembership::Present | SnapshotMembership::Mixed => None,
        };
        let capacity = hashes.len().saturating_add(usize::from(absent.is_some()));
        let mut cohort = HashSet::new();
        cohort
            .try_reserve(capacity)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        let mut snapshot = Vec::new();
        snapshot
            .try_reserve(capacity)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        if let Some(hash) = absent {
            cohort.insert(hash.clone());
            snapshot.push((hash.clone(), None));
        }
        for hash in hashes {
            if !cohort.insert(hash.clone()) {
                continue;
            }
            let entry = self.entries.get(hash).cloned();
            if matches!(
                membership,
                SnapshotMembership::Present | SnapshotMembership::Absent(_)
            ) && entry.is_none()
            {
                return Err(CoordinatorError::Missing(hash.clone()));
            }
            snapshot.push((hash.clone(), entry));
        }
        Ok((snapshot, cohort))
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
                if let Err(error) = self.sync_victim_indexes(&snapshot) {
                    restore_or_panic(self, snapshot);
                    Err(error)
                } else {
                    Ok(value)
                }
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
        if self.entry_transaction_active {
            return Err(CoordinatorError::NestedUndoTransaction);
        }
        self.entry_transaction_membership
            .try_reserve(cohort.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        for hash in cohort {
            self.entry_transaction_membership.insert(hash.clone());
        }
        self.entry_transaction_active = true;
        Ok(())
    }

    pub(super) fn end_entry_transaction(&mut self, cohort: &HashSet<Byte32>) {
        debug_assert!(self.entry_transaction_active);
        for hash in cohort {
            assert!(
                self.entry_transaction_membership.remove(hash),
                "active undo cohort membership exists"
            );
        }
        assert!(self.entry_transaction_membership.is_empty());
        self.entry_transaction_active = false;
    }

    #[track_caller]
    pub(super) fn ensure_entry_mutation_is_snapshotted(
        &self,
        hash: &Byte32,
    ) -> Result<(), CoordinatorError> {
        if self.entry_transaction_active && !self.entry_transaction_membership.contains(hash) {
            let caller = std::panic::Location::caller();
            return Err(CoordinatorError::UndoCohortViolation {
                hash: hash.clone(),
                mutation_file: caller.file(),
                mutation_line: caller.line(),
                active_members: self.entry_transaction_membership.iter().cloned().collect(),
            });
        }
        Ok(())
    }

    /// Require a caller-owned undo boundary that already covers the complete
    /// apply cohort. Composite transitions use this instead of opening a
    /// nested snapshot.
    pub(super) fn require_entry_transaction(
        &self,
        hashes: &[Byte32],
    ) -> Result<(), CoordinatorError> {
        if !self.entry_transaction_active {
            return Err(CoordinatorError::ConflictInvariant);
        }
        for hash in hashes {
            self.ensure_entry_mutation_is_snapshotted(hash)?;
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
        if !self.entry_transaction_active {
            return Err(CoordinatorError::ConflictInvariant);
        }
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
            let raw_charge = self
                .entries
                .get(child)
                .map(|entry| entry.raw_charge_bytes)
                .ok_or_else(|| CoordinatorError::Missing(child.clone()))?;
            self.ensure_revision_capacity(child)?;
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
        self.next_maintenance_sequence = next_maintenance_sequence;
        for (offset, child) in children.iter().enumerate() {
            let sequence = first_sequence
                .checked_add(
                    u64::try_from(offset)
                        .map_err(|_| CoordinatorError::MaintenanceSequenceExhausted)?,
                )
                .ok_or(CoordinatorError::MaintenanceSequenceExhausted)?;
            self.invalidate_present_apply(child, cause, sequence)?;
        }
        Ok(children)
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
            .ok_or(CoordinatorError::ConflictInvariant)?;
        if self.by_short_id.remove(&entry.short_id).as_ref() != Some(hash) {
            return Err(CoordinatorError::ConflictInvariant);
        }
        if let Some(peer) = entry.source.peer() {
            self.release_peer_attribution(hash, peer, charge, false)?;
        }
        for parent in &entry.dependencies {
            let children = self
                .by_parent
                .get_mut(parent)
                .ok_or(CoordinatorError::ConflictInvariant)?;
            if !children.remove(hash) {
                return Err(CoordinatorError::ConflictInvariant);
            }
            if children.is_empty() {
                self.by_parent.remove(parent);
            }
        }
        if self.live_deadlines.remove(hash).is_some()
            != (entry.expires_at.is_some() && !entry.is_committing())
        {
            return Err(CoordinatorError::ConflictInvariant);
        }
        self.compact_deadlines();
        if self.dependency_failure_set.remove(hash) != entry.invalidated_cause().is_some() {
            return Err(CoordinatorError::ConflictInvariant);
        }
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
