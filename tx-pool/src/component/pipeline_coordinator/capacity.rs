use super::*;

impl<R, U, V> PipelineCoordinator<R, U, V> {
    pub(super) fn capacity_victim_key(
        hash: &Byte32,
        entry: &CoordinatorEntry<R, U, V>,
    ) -> Option<CapacityVictimKey> {
        (!entry.is_committing()).then(|| CapacityVictimKey {
            valid: entry.invalidated_cause().is_none(),
            source_strength: entry.source.trust(),
            charge_bytes: entry.charge_bytes,
            queue_sequence: entry.queue_sequence,
            hash: hash.clone(),
        })
    }

    pub(super) fn candidate_victim_key(
        hash: &Byte32,
        entry: &CoordinatorEntry<R, U, V>,
    ) -> Option<CandidateRank> {
        let candidate = entry.candidate()?;
        (!entry.is_committing()).then(|| CandidateRank::verified(hash, entry.source, candidate))
    }

    pub(super) fn sync_victim_indexes(&mut self, snapshot: &[EntrySnapshot<R, U, V>]) {
        for (hash, old_entry) in snapshot {
            if let Some(key) = old_entry
                .as_ref()
                .and_then(|entry| Self::capacity_victim_key(hash, entry))
            {
                self.capacity_victim_index.remove(&key);
            }
            if let Some(key) = old_entry
                .as_ref()
                .and_then(|entry| Self::candidate_victim_key(hash, entry))
            {
                self.candidate_victim_index.remove(&key);
            }
        }
        for (hash, _) in snapshot {
            if let Some(key) = self
                .entries
                .get(hash)
                .and_then(|entry| Self::capacity_victim_key(hash, entry))
            {
                // Replace keeps publication idempotent when a snapshot lists
                // the same hash through more than one causal relation.
                self.capacity_victim_index.replace(key);
            }
            if let Some(key) = self
                .entries
                .get(hash)
                .and_then(|entry| Self::candidate_victim_key(hash, entry))
            {
                self.candidate_victim_index.replace(key);
            }
        }
    }

    pub(super) fn current_victim_keys(
        &self,
        hash: &Byte32,
    ) -> (Option<CapacityVictimKey>, Option<CandidateRank>) {
        let entry = self.entries.get(hash);
        (
            entry.and_then(|entry| Self::capacity_victim_key(hash, entry)),
            entry.and_then(|entry| Self::candidate_victim_key(hash, entry)),
        )
    }

    pub(super) fn refresh_victim_indexes(
        &mut self,
        hash: &Byte32,
        old: (Option<CapacityVictimKey>, Option<CandidateRank>),
    ) {
        if self.entry_transaction_depth != 0 {
            // The outer undo snapshot owns derived-index publication for the
            // complete nested mutation cohort.
            return;
        }
        if let Some(key) = old.0 {
            self.capacity_victim_index.remove(&key);
        }
        if let Some(key) = old.1 {
            self.candidate_victim_index.remove(&key);
        }
        if let Some(key) = self
            .entries
            .get(hash)
            .and_then(|entry| Self::capacity_victim_key(hash, entry))
        {
            self.capacity_victim_index.replace(key);
        }
        if let Some(key) = self
            .entries
            .get(hash)
            .and_then(|entry| Self::candidate_victim_key(hash, entry))
        {
            self.candidate_victim_index.replace(key);
        }
    }

    pub(super) fn dependency_capacity_victims(
        &self,
        source: CoordinatorSource,
        dependencies: &HashSet<Byte32>,
        protected: &HashSet<Byte32>,
    ) -> Result<Vec<Byte32>, CoordinatorError> {
        let mut parents: Vec<_> = dependencies.iter().cloned().collect();
        parents.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        let mut selected = HashSet::new();
        let mut victims = Vec::new();
        for parent in parents {
            let Some(children) = self.by_parent.get(&parent) else {
                continue;
            };
            let occupied = children
                .iter()
                .filter(|child| !selected.contains(*child))
                .count();
            if occupied < self.limits.max_dependents_per_parent {
                continue;
            }
            let incoming_strength = source.trust();
            let victim = children
                .iter()
                .filter(|child| !selected.contains(*child))
                .filter(|child| !protected.contains(*child))
                .filter_map(|child| self.entries.get(child).map(|entry| (child, entry)))
                .filter(|(_, entry)| {
                    !entry.is_committing()
                        && (entry.invalidated_cause().is_some()
                            || entry.source.trust() < incoming_strength)
                })
                .min_by(|(left_hash, left), (right_hash, right)| {
                    left.invalidated_cause()
                        .is_none()
                        .cmp(&right.invalidated_cause().is_none())
                        .then_with(|| left.source.trust().cmp(&right.source.trust()))
                        .then_with(|| right.queue_sequence.cmp(&left.queue_sequence))
                        .then_with(|| left_hash.as_slice().cmp(right_hash.as_slice()))
                })
                .map(|(hash, _)| hash.clone())
                .ok_or_else(|| CoordinatorError::ParentFanoutLimitExceeded(parent.clone()))?;
            selected.insert(victim.clone());
            victims.push(victim);
        }
        victims.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        Ok(victims)
    }

    pub(super) fn dependency_ancestor_closure(
        &self,
        owner: &Byte32,
        dependencies: &HashSet<Byte32>,
    ) -> Result<HashSet<Byte32>, CoordinatorError> {
        let mut ancestors = HashSet::new();
        let mut pending: Vec<_> = dependencies.iter().cloned().collect();
        while let Some(hash) = pending.pop() {
            if &hash == owner {
                return Err(CoordinatorError::DependencyCycle(owner.clone()));
            }
            if !ancestors.insert(hash.clone()) {
                continue;
            }
            if ancestors.len() > self.limits.max_dependency_ancestors {
                return Err(CoordinatorError::DependencyAncestorLimitExceeded);
            }
            if let Some(entry) = self.entries.get(&hash) {
                pending.extend(entry.dependencies.iter().cloned());
            }
        }
        Ok(ancestors)
    }

    pub(super) fn check_peer_budget_after_victims(
        &self,
        incoming_hash: Option<&Byte32>,
        incoming_source: CoordinatorSource,
        incoming_charge_bytes: usize,
        victims: &HashSet<Byte32>,
    ) -> Result<(), CoordinatorError> {
        let (Some(peer), Some(limit)) = (incoming_source.peer(), self.limits.per_peer) else {
            return Ok(());
        };
        let mut projected = self.peer_usage(peer);
        if let Some(hash) = incoming_hash {
            let old = self
                .entries
                .get(hash)
                .filter(|entry| entry.source.peer() == Some(peer))
                .map(|entry| CoordinatorResidency::new(1, entry.charge_bytes))
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            projected = projected
                .checked_sub(old)
                .ok_or(CoordinatorError::ConflictInvariant)?;
        }
        for hash in victims {
            let Some(entry) = self.entries.get(hash) else {
                return Err(CoordinatorError::Missing(hash.clone()));
            };
            if entry.source.peer() == Some(peer) {
                projected = projected
                    .checked_sub(CoordinatorResidency::new(1, entry.charge_bytes))
                    .ok_or(CoordinatorError::ConflictInvariant)?;
            }
        }
        projected = projected
            .checked_add(CoordinatorResidency::new(1, incoming_charge_bytes))
            .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
        if !projected.fits(limit) {
            return Err(CoordinatorError::PeerBudgetExceeded(peer));
        }
        Ok(())
    }

    pub(super) fn global_capacity_victims(
        &self,
        incoming_hash: Option<&Byte32>,
        incoming_source: CoordinatorSource,
        incoming_charge_bytes: usize,
        preselected: &HashSet<Byte32>,
        protected: &HashSet<Byte32>,
    ) -> Result<Vec<Byte32>, CoordinatorError> {
        if preselected.len() > self.limits.max_capacity_evictions_per_transition {
            return Err(CoordinatorError::CapacityEvictionLimitExceeded);
        }
        let mut projected = self.global_usage;
        if let Some(hash) = incoming_hash {
            let old = self
                .entries
                .get(hash)
                .map(|entry| CoordinatorResidency::new(1, entry.charge_bytes))
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            projected = projected
                .checked_sub(old)
                .ok_or(CoordinatorError::ConflictInvariant)?;
        }
        for hash in preselected {
            let charge = self
                .entries
                .get(hash)
                .map(|entry| CoordinatorResidency::new(1, entry.charge_bytes))
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            projected = projected
                .checked_sub(charge)
                .ok_or(CoordinatorError::ConflictInvariant)?;
        }
        projected = projected
            .checked_add(CoordinatorResidency::new(1, incoming_charge_bytes))
            .ok_or(CoordinatorError::GlobalBudgetExceeded)?;

        let mut selected = preselected.clone();
        let mut victims = Vec::new();
        let incoming_strength = incoming_source.trust();
        for key in &self.capacity_victim_index {
            #[cfg(test)]
            self.capacity_victim_probes
                .set(self.capacity_victim_probes.get().saturating_add(1));
            if projected.fits(self.limits.global) {
                break;
            }
            // Invalidated work sorts first and is always reclaimable. Once
            // the valid suffix reaches the incoming source strength, no later
            // key can be an eligible victim.
            if key.valid && key.source_strength >= incoming_strength {
                break;
            }
            if incoming_hash == Some(&key.hash)
                || selected.contains(&key.hash)
                || protected.contains(&key.hash)
            {
                continue;
            }
            if selected.len() >= self.limits.max_capacity_evictions_per_transition {
                return Err(CoordinatorError::CapacityEvictionLimitExceeded);
            }
            let charge_bytes = self
                .entries
                .get(&key.hash)
                .map(|entry| entry.charge_bytes)
                .ok_or_else(|| CoordinatorError::Missing(key.hash.clone()))?;
            selected.insert(key.hash.clone());
            projected = projected
                .checked_sub(CoordinatorResidency::new(1, charge_bytes))
                .ok_or(CoordinatorError::ConflictInvariant)?;
            victims.push(key.hash.clone());
        }
        if !projected.fits(self.limits.global) {
            return Err(CoordinatorError::GlobalBudgetExceeded);
        }
        victims.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        Ok(victims)
    }

    pub(super) fn compare_candidate_capacity(
        left_hash: &Byte32,
        left_source: CoordinatorSource,
        left: &CandidateMeta,
        right_hash: &Byte32,
        right_source: CoordinatorSource,
        right: &CandidateMeta,
    ) -> Ordering {
        CandidateRank::verified(left_hash, left_source, left).cmp(&CandidateRank::verified(
            right_hash,
            right_source,
            right,
        ))
    }

    pub(super) fn conflict_capacity_victims(
        &self,
        incoming_hash: &Byte32,
        incoming_source: CoordinatorSource,
        incoming: &CandidateMeta,
        protected: &HashSet<Byte32>,
    ) -> Result<Vec<Byte32>, CoordinatorError> {
        let mut inputs: Vec<_> = incoming.inputs.iter().cloned().collect();
        inputs.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        let mut selected = HashSet::new();
        let mut victims = Vec::new();
        for input in inputs {
            let Some(candidates) = self.conflicts.by_input.get(&input) else {
                continue;
            };
            let occupied = candidates
                .iter()
                .filter(|hash| !selected.contains(*hash))
                .count();
            if occupied < self.limits.max_candidates_per_input {
                continue;
            }
            let victim = candidates
                .iter()
                .filter(|hash| !selected.contains(*hash))
                .filter(|hash| !protected.contains(*hash))
                .filter_map(|hash| {
                    self.entries.get(hash).and_then(|entry| {
                        entry.candidate().map(|candidate| (hash, entry, candidate))
                    })
                })
                .filter(|(hash, entry, candidate)| {
                    !entry.is_committing()
                        && Self::compare_candidate_capacity(
                            incoming_hash,
                            incoming_source,
                            incoming,
                            hash,
                            entry.source,
                            candidate,
                        ) == Ordering::Greater
                })
                .min_by(
                    |(left_hash, left_entry, left), (right_hash, right_entry, right)| {
                        Self::compare_candidate_capacity(
                            left_hash,
                            left_entry.source,
                            left,
                            right_hash,
                            right_entry.source,
                            right,
                        )
                    },
                )
                .map(|(hash, _, _)| hash.clone())
                .ok_or_else(|| CoordinatorError::ConflictCandidateLimitExceeded(input.clone()))?;
            selected.insert(victim.clone());
            victims.push(victim);
        }
        let mut projected_edges = self.conflicts.input_memberships;
        for hash in &selected {
            let edges = self
                .entries
                .get(hash)
                .and_then(CoordinatorEntry::candidate)
                .map(|candidate| candidate.inputs.len())
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            projected_edges = projected_edges
                .checked_sub(edges)
                .ok_or(CoordinatorError::ConflictInvariant)?;
        }
        projected_edges = projected_edges
            .checked_add(incoming.inputs.len())
            .ok_or(CoordinatorError::ConflictEdgeLimitExceeded)?;
        let incoming_key = CandidateRank::verified(incoming_hash, incoming_source, incoming);
        for key in &self.candidate_victim_index {
            #[cfg(test)]
            self.candidate_victim_probes
                .set(self.candidate_victim_probes.get().saturating_add(1));
            if projected_edges <= self.limits.max_conflict_edges {
                break;
            }
            if key >= &incoming_key {
                break;
            }
            if &key.hash == incoming_hash
                || selected.contains(&key.hash)
                || protected.contains(&key.hash)
            {
                continue;
            }
            if selected.len() >= self.limits.max_capacity_evictions_per_transition {
                return Err(CoordinatorError::CapacityEvictionLimitExceeded);
            }
            let edges = self
                .entries
                .get(&key.hash)
                .and_then(CoordinatorEntry::candidate)
                .map(|candidate| candidate.inputs.len())
                .ok_or_else(|| CoordinatorError::Missing(key.hash.clone()))?;
            selected.insert(key.hash.clone());
            victims.push(key.hash.clone());
            projected_edges = projected_edges
                .checked_sub(edges)
                .ok_or(CoordinatorError::ConflictInvariant)?;
        }
        if projected_edges > self.limits.max_conflict_edges {
            return Err(CoordinatorError::ConflictEdgeLimitExceeded);
        }
        victims.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        Ok(victims)
    }

    pub(super) fn check_activate_source(
        &self,
        source: CoordinatorSource,
    ) -> Result<(), CoordinatorError> {
        if self.active_work >= self.limits.max_active_work {
            return Err(CoordinatorError::ActiveWorkLimitExceeded);
        }
        if let Some(peer) = source.peer()
            && self.peer_active_work(peer) >= self.limits.max_active_work_per_peer
        {
            return Err(CoordinatorError::PeerActiveWorkLimitExceeded(peer));
        }
        Ok(())
    }

    pub(super) fn activate_source(
        &mut self,
        source: CoordinatorSource,
    ) -> Result<(), CoordinatorError> {
        self.active_work = self
            .active_work
            .checked_add(1)
            .ok_or(CoordinatorError::ConflictInvariant)?;
        if let Some(peer) = source.peer() {
            let active = self.active_work_by_peer.entry(peer).or_default();
            *active = active
                .checked_add(1)
                .ok_or(CoordinatorError::ConflictInvariant)?;
        }
        Ok(())
    }

    pub(super) fn deactivate_source(
        &mut self,
        source: CoordinatorSource,
    ) -> Result<(), CoordinatorError> {
        self.active_work = self
            .active_work
            .checked_sub(1)
            .ok_or(CoordinatorError::ConflictInvariant)?;
        if let Some(peer) = source.peer() {
            let remove = {
                let active = self
                    .active_work_by_peer
                    .get_mut(&peer)
                    .ok_or(CoordinatorError::ConflictInvariant)?;
                *active = active
                    .checked_sub(1)
                    .ok_or(CoordinatorError::ConflictInvariant)?;
                *active == 0
            };
            if remove {
                self.active_work_by_peer.remove(&peer);
            }
        }
        Ok(())
    }

    pub(super) fn metadata_charge_bytes(
        &self,
        dependencies: usize,
        has_deadline: bool,
        conflict_inputs: usize,
    ) -> Result<usize, CoordinatorError> {
        let cost = self.limits.metadata_cost;
        let mut bytes = cost
            .entry_bytes
            .checked_add(cost.lifecycle_ticket_bytes)
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        bytes = bytes
            .checked_add(
                dependencies
                    .checked_mul(cost.dependency_edge_bytes)
                    .ok_or(CoordinatorError::ResidencyChargeOverflow)?,
            )
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        if has_deadline {
            bytes = bytes
                .checked_add(cost.deadline_ticket_bytes)
                .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        }
        bytes = bytes
            .checked_add(
                conflict_inputs
                    .checked_mul(cost.conflict_edge_bytes)
                    .ok_or(CoordinatorError::ResidencyChargeOverflow)?,
            )
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        Ok(bytes)
    }

    /// Canonical entry charge equation used by both production undo rebuild
    /// and the independent test auditor. Index reconstruction intentionally
    /// remains independent so it can detect implementation drift, while the
    /// accounting contract itself has exactly one definition.
    pub(super) fn entry_metadata_charge_is_valid(&self, entry: &CoordinatorEntry<R, U, V>) -> bool {
        let conflict_inputs = entry.candidate().map_or(0, |meta| meta.inputs.len());
        let Ok(base_metadata) =
            self.metadata_charge_bytes(entry.dependencies.len(), entry.expires_at.is_some(), 0)
        else {
            return false;
        };
        let Ok(metadata) = self.metadata_charge_bytes(
            entry.dependencies.len(),
            entry.expires_at.is_some(),
            conflict_inputs,
        ) else {
            return false;
        };
        let Some(raw_charge) = entry.raw_resident_payload_bytes.checked_add(base_metadata) else {
            return false;
        };
        let Some(charge) = entry.resident_payload_bytes.checked_add(metadata) else {
            return false;
        };
        entry.base_metadata_bytes == base_metadata
            && entry.metadata_bytes == metadata
            && entry.raw_charge_bytes == raw_charge
            && entry.charge_bytes == charge
    }

    pub(super) fn check_add_budget(
        &self,
        peer: Option<PeerIndex>,
        charge: CoordinatorResidency,
    ) -> Result<(), CoordinatorError> {
        let next_global = self
            .global_usage
            .checked_add(charge)
            .ok_or(CoordinatorError::GlobalBudgetExceeded)?;
        if !next_global.fits(self.limits.global) {
            return Err(CoordinatorError::GlobalBudgetExceeded);
        }
        if let (Some(peer), Some(limit)) = (peer, self.limits.per_peer) {
            let next_peer = self
                .peer_usage(peer)
                .checked_add(charge)
                .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
            if !next_peer.fits(limit) {
                return Err(CoordinatorError::PeerBudgetExceeded(peer));
            }
        }
        Ok(())
    }

    pub(super) fn check_recharge(
        &self,
        hash: &Byte32,
        new_bytes: usize,
    ) -> Result<(), CoordinatorError> {
        let entry = self
            .entries
            .get(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        let old = CoordinatorResidency::new(1, entry.charge_bytes);
        let new = CoordinatorResidency::new(1, new_bytes);
        let remaining_global = self
            .global_usage
            .checked_sub(old)
            .ok_or(CoordinatorError::ConflictInvariant)?;
        let next_global = remaining_global
            .checked_add(new)
            .ok_or(CoordinatorError::GlobalBudgetExceeded)?;
        if !next_global.fits(self.limits.global) {
            return Err(CoordinatorError::GlobalBudgetExceeded);
        }
        if let (Some(peer), Some(limit)) = (entry.source.peer(), self.limits.per_peer) {
            let remaining_peer = self
                .peer_usage(peer)
                .checked_sub(old)
                .ok_or(CoordinatorError::ConflictInvariant)?;
            let next_peer = remaining_peer
                .checked_add(new)
                .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
            if !next_peer.fits(limit) {
                return Err(CoordinatorError::PeerBudgetExceeded(peer));
            }
        }
        Ok(())
    }

    pub(super) fn apply_recharge(
        &mut self,
        hash: &Byte32,
        new_bytes: usize,
    ) -> Result<(), CoordinatorError> {
        let (peer, old_bytes) = self
            .entries
            .get(hash)
            .map(|entry| (entry.source.peer(), entry.charge_bytes))
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        let old = CoordinatorResidency::new(1, old_bytes);
        let new = CoordinatorResidency::new(1, new_bytes);
        self.global_usage = self
            .global_usage
            .checked_sub(old)
            .and_then(|usage| usage.checked_add(new))
            .ok_or(CoordinatorError::ConflictInvariant)?;
        if let Some(peer) = peer {
            let usage = self
                .peer_usage
                .get_mut(&peer)
                .ok_or(CoordinatorError::ConflictInvariant)?;
            *usage = usage
                .checked_sub(old)
                .and_then(|usage| usage.checked_add(new))
                .ok_or(CoordinatorError::ConflictInvariant)?;
        }
        self.entry_mut(hash)?.charge_bytes = new_bytes;
        Ok(())
    }

    pub(super) fn with_capacity_victims<T, F>(
        &mut self,
        subject: CapacitySubject,
        victims: Vec<Byte32>,
        subject_undo: Vec<Byte32>,
        apply_subject: F,
    ) -> Result<(T, Vec<TerminalRecord<R>>), CoordinatorError>
    where
        F: FnOnce(&mut Self) -> Result<T, CoordinatorError>,
    {
        if victims.len() > self.limits.max_capacity_evictions_per_transition {
            return Err(CoordinatorError::CapacityEvictionLimitExceeded);
        }
        let mut terminal = Vec::new();
        terminal
            .try_reserve(victims.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        let mut affected = self.causal_undo_hashes(&victims);
        affected.extend(subject_undo);
        for victim in &victims {
            self.preflight_remove_conflict_indexes(victim)?;
        }
        let transaction = move |coordinator: &mut Self| {
            for victim in victims {
                terminal.push(coordinator.terminalize_present_apply(
                    victim,
                    None,
                    TerminalDisposition::CapacityEvicted,
                )?);
            }
            let result = apply_subject(coordinator)?;
            coordinator.apply_fault_checkpoint();
            Ok((result, terminal))
        };
        match subject {
            CapacitySubject::Absent(hash) => {
                self.with_absent_entry_undo(&hash, &affected, transaction)
            }
            CapacitySubject::Present(hash) => {
                affected.push(hash);
                self.with_entry_undo(&affected, transaction)
            }
        }
    }
}
