use super::*;

#[cfg(test)]
#[path = "../tests/pipeline_coordinator_lifecycle_seam.rs"]
mod test_seam;

pub(super) struct ParentsUnavailablePlan {
    pub(super) undo: Vec<Byte32>,
    affected: Vec<Byte32>,
    missing_by_child: HashMap<Byte32, HashSet<Byte32>>,
    trusted_sequences: HashMap<Byte32, u64>,
    next_maintenance_sequence: u64,
}

impl<R, U, V> PipelineCoordinator<R, U, V> {
    /// Extend the canonical dependency graph with parents discovered only
    /// after dep-group expansion, and preflight the exact raw-phase charge.
    ///
    /// Raw transactions can name the dep-group cell at admission but cannot
    /// name its members until resolution reads the group data. Treating such
    /// a resolver miss as an invariant violation lets an ordinary remote
    /// transaction reach fail-stop. The dependency set is already the sole
    /// graph authority, so extend it transactionally instead of introducing a
    /// second orphan graph or weakening the waiting-state invariant.
    fn plan_discovered_dependencies(
        &self,
        hash: &Byte32,
        discovered: &HashSet<Byte32>,
    ) -> Result<(HashSet<Byte32>, Vec<Byte32>, usize, usize), CoordinatorError> {
        let entry = self
            .entries
            .get(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        let mut dependencies = entry.dependencies.clone();
        let mut added = Vec::new();
        for parent in discovered {
            let parent = crate::util::compact_packed(parent);
            if parent == *hash {
                return Err(CoordinatorError::SelfDependency(hash.clone()));
            }
            if dependencies.insert(parent.clone()) {
                added.push(parent);
            }
        }
        if dependencies.len() > self.limits.max_dependencies_per_entry {
            return Err(CoordinatorError::DependencyLimitExceeded);
        }
        self.dependency_ancestor_closure(hash, &dependencies)?;
        for parent in &added {
            if self.by_parent.get(parent).map_or(0, HashSet::len)
                >= self.limits.max_dependents_per_parent
            {
                return Err(CoordinatorError::ParentFanoutLimitExceeded(parent.clone()));
            }
        }
        let base_metadata_bytes =
            self.metadata_charge_bytes(dependencies.len(), entry.expires_at.is_some(), 0)?;
        let raw_charge_bytes = entry
            .raw_resident_payload_bytes
            .checked_add(base_metadata_bytes)
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        Ok((dependencies, added, base_metadata_bytes, raw_charge_bytes))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn admit_raw_sourced(
        &mut self,
        hash: Byte32,
        short_id: ProposalShortId,
        raw: R,
        initial_stage: RawStage,
        source: CoordinatorSource,
        expires_at: Option<u64>,
        charge_bytes: usize,
        dependencies: HashSet<Byte32>,
    ) -> Result<(CoordinatorVersion, Vec<TerminalRecord<R>>), CoordinatorError> {
        // The coordinator is the persistent ownership boundary. Enforce
        // compact identity and parent keys here rather than relying on every
        // adapter to remember that molecule accessors may share a whole raw
        // transaction, block, or relay envelope.
        let hash = crate::util::compact_packed(&hash);
        let short_id = crate::util::compact_packed(&short_id);
        let dependencies = dependencies
            .into_iter()
            .map(|parent| crate::util::compact_packed(&parent))
            .collect::<HashSet<_>>();
        if self.entries.contains_key(&hash) {
            return Err(CoordinatorError::DuplicateHash(hash));
        }
        if let Some(existing_hash) = self.by_short_id.get(&short_id) {
            return Err(CoordinatorError::ShortIdCollision {
                short_id,
                existing_hash: existing_hash.clone(),
            });
        }
        if dependencies.contains(&hash) {
            return Err(CoordinatorError::SelfDependency(hash));
        }
        if dependencies.len() > self.limits.max_dependencies_per_entry {
            return Err(CoordinatorError::DependencyLimitExceeded);
        }
        let protected = self.dependency_ancestor_closure(&hash, &dependencies)?;
        let mut victims = self.dependency_capacity_victims(source, &dependencies, &protected)?;
        let base_metadata_bytes =
            self.metadata_charge_bytes(dependencies.len(), expires_at.is_some(), 0)?;
        let incoming_charge_bytes = charge_bytes
            .checked_add(base_metadata_bytes)
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        let selected: HashSet<_> = victims.iter().cloned().collect();
        self.check_peer_budget_after_victims(None, source, incoming_charge_bytes, &selected)?;
        victims.extend(self.global_capacity_victims(
            None,
            source,
            incoming_charge_bytes,
            &selected,
            &protected,
        )?);
        victims.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        victims.dedup();
        let subject = CapacitySubject::Absent(hash.clone());
        self.with_capacity_victims(subject, victims, Vec::new(), move |coordinator| {
            coordinator.admit_raw_sourced_inner(
                hash,
                short_id,
                raw,
                initial_stage,
                source,
                expires_at,
                charge_bytes,
                dependencies,
            )
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn admit_raw_sourced_inner(
        &mut self,
        hash: Byte32,
        short_id: ProposalShortId,
        raw: R,
        initial_stage: RawStage,
        source: CoordinatorSource,
        expires_at: Option<u64>,
        charge_bytes: usize,
        dependencies: HashSet<Byte32>,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        if self.entries.contains_key(&hash) {
            return Err(CoordinatorError::DuplicateHash(hash));
        }
        if let Some(existing_hash) = self.by_short_id.get(&short_id) {
            return Err(CoordinatorError::ShortIdCollision {
                short_id,
                existing_hash: existing_hash.clone(),
            });
        }
        if dependencies.contains(&hash) {
            return Err(CoordinatorError::SelfDependency(hash));
        }
        if dependencies.len() > self.limits.max_dependencies_per_entry {
            return Err(CoordinatorError::DependencyLimitExceeded);
        }
        for parent in &dependencies {
            if self.by_parent.get(parent).map_or(0, HashSet::len)
                >= self.limits.max_dependents_per_parent
            {
                return Err(CoordinatorError::ParentFanoutLimitExceeded(parent.clone()));
            }
        }
        let base_metadata_bytes =
            self.metadata_charge_bytes(dependencies.len(), expires_at.is_some(), 0)?;
        let total_charge_bytes = charge_bytes
            .checked_add(base_metadata_bytes)
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        let charge = CoordinatorResidency::new(1, total_charge_bytes);
        let peer = source.peer();
        self.check_add_budget(peer, charge)?;
        let incarnation = self.next_incarnation;
        let next_incarnation = incarnation
            .checked_add(1)
            .ok_or(CoordinatorError::IncarnationExhausted)?;
        let (queue_sequence, next_queue_sequence) = self.queue_sequence_range(1)?;
        let queue_kind = match initial_stage {
            RawStage::PreCheck => QueueKind::PreCheck,
            RawStage::Resolve => QueueKind::Resolve,
        };
        self.queue_mut(queue_kind)
            .reserve_live(source.queue_owner(), false)?;
        if expires_at.is_some() {
            self.deadlines
                .try_reserve(1)
                .map_err(|_| CoordinatorError::QueueReservationFailed)?;
            self.live_deadlines
                .try_reserve(1)
                .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        }

        let entry = CoordinatorEntry {
            short_id: short_id.clone(),
            state: EntryState::Raw {
                raw: Arc::new(raw),
                location: RawLocation::Queued(initial_stage),
            },
            source,
            expires_at,
            raw_charge_bytes: total_charge_bytes,
            raw_resident_payload_bytes: charge_bytes,
            resident_payload_bytes: charge_bytes,
            base_metadata_bytes,
            metadata_bytes: base_metadata_bytes,
            charge_bytes: total_charge_bytes,
            dependencies: dependencies.clone(),
            incarnation,
            revision: 0,
            deadline_generation: 0,
            queue_sequence,
        };
        let ticket = entry.ticket(&hash);
        self.next_incarnation = next_incarnation;
        self.next_queue_sequence = next_queue_sequence;
        self.global_usage = self
            .global_usage
            .checked_add(charge)
            .ok_or(CoordinatorError::ConflictInvariant)?;
        if let Some(peer) = peer {
            let usage = self.peer_usage.entry(peer).or_default();
            *usage = usage
                .checked_add(charge)
                .ok_or(CoordinatorError::ConflictInvariant)?;
            if !self.by_peer.entry(peer).or_default().insert(hash.clone()) {
                return Err(CoordinatorError::ConflictInvariant);
            }
        }
        for parent in &dependencies {
            self.insert_parent_membership(parent, &hash)?;
        }
        if self.by_short_id.insert(short_id, hash.clone()).is_some() {
            return Err(CoordinatorError::ConflictInvariant);
        }
        if let Some(expires_at) = expires_at {
            let deadline = DeadlineTicket {
                expires_at,
                hash: hash.clone(),
                incarnation,
                generation: 0,
            };
            self.deadlines.push(Reverse(deadline.clone()));
            if self.live_deadlines.insert(hash.clone(), deadline).is_some() {
                return Err(CoordinatorError::ConflictInvariant);
            }
        }
        self.insert_absent_entry(hash, entry)?;
        self.queue_mut(queue_kind)
            .push_reserved(queue_kind, ticket, source.is_proposal())?;
        Ok(CoordinatorVersion {
            incarnation,
            revision: 0,
        })
    }

    pub(super) fn release_peer_attribution(
        &mut self,
        hash: &Byte32,
        peer: PeerIndex,
        charge: CoordinatorResidency,
        active: bool,
    ) -> Result<(), CoordinatorError> {
        let remove_usage = {
            let usage = self
                .peer_usage
                .get_mut(&peer)
                .ok_or(CoordinatorError::ConflictInvariant)?;
            *usage = usage
                .checked_sub(charge)
                .ok_or(CoordinatorError::ConflictInvariant)?;
            *usage == CoordinatorResidency::default()
        };
        if remove_usage {
            self.peer_usage.remove(&peer);
        }
        let hashes = self
            .by_peer
            .get_mut(&peer)
            .ok_or(CoordinatorError::ConflictInvariant)?;
        if !hashes.remove(hash) {
            return Err(CoordinatorError::ConflictInvariant);
        }
        if hashes.is_empty() {
            self.by_peer.remove(&peer);
        }
        if active {
            let remove_active = {
                let active = self
                    .active_work_by_peer
                    .get_mut(&peer)
                    .ok_or(CoordinatorError::ConflictInvariant)?;
                *active = active
                    .checked_sub(1)
                    .ok_or(CoordinatorError::ConflictInvariant)?;
                *active == 0
            };
            if remove_active {
                self.active_work_by_peer.remove(&peer);
            }
        }
        Ok(())
    }

    fn insert_parent_membership(
        &mut self,
        parent: &Byte32,
        child: &Byte32,
    ) -> Result<(), CoordinatorError> {
        if !self
            .by_parent
            .entry(parent.clone())
            .or_default()
            .insert(child.clone())
        {
            return Err(CoordinatorError::ConflictInvariant);
        }
        Ok(())
    }

    pub(crate) fn promote_source(
        &mut self,
        hash: &Byte32,
        promotion: TrustedSource,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        let (
            current,
            old_charge,
            new_charge,
            new_raw_charge_bytes,
            new_base_metadata_bytes,
            new_metadata_bytes,
            old_ticket,
            queue_kind,
            version,
            active,
            had_live_deadline,
            waiting_parent,
            candidate_state,
        ) = {
            let entry = self
                .entries
                .get(hash)
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            let old_charge = CoordinatorResidency::new(1, entry.charge_bytes);
            let (new_base_metadata_bytes, new_metadata_bytes) = if entry.expires_at.is_some() {
                (
                    self.metadata_charge_bytes(entry.dependencies.len(), false, 0)?,
                    self.metadata_charge_bytes(
                        entry.dependencies.len(),
                        false,
                        entry
                            .candidate()
                            .map_or(0, |candidate| candidate.inputs.len()),
                    )?,
                )
            } else {
                (entry.base_metadata_bytes, entry.metadata_bytes)
            };
            let new_charge_bytes = entry
                .resident_payload_bytes
                .checked_add(new_metadata_bytes)
                .ok_or(CoordinatorError::ConflictInvariant)?;
            let new_raw_charge_bytes = entry
                .raw_resident_payload_bytes
                .checked_add(new_base_metadata_bytes)
                .ok_or(CoordinatorError::ConflictInvariant)?;
            (
                entry.source,
                old_charge,
                CoordinatorResidency::new(1, new_charge_bytes),
                new_raw_charge_bytes,
                new_base_metadata_bytes,
                new_metadata_bytes,
                entry.ticket(hash),
                entry.queue_kind(),
                entry.version(),
                entry.uses_active_slot(),
                entry.expires_at.is_some() && !entry.is_committing(),
                matches!(entry.location(), CoordinatorLocation::WaitingParents { .. }),
                match &entry.state {
                    EntryState::CandidateVerified {
                        candidate,
                        location,
                        ..
                    } => Some((candidate.clone(), location.clone())),
                    _ => None,
                },
            )
        };
        let target = match promotion {
            TrustedSource::Local => CoordinatorSource::Local,
            TrustedSource::Proposal => CoordinatorSource::Proposal,
        };
        if current == CoordinatorSource::Proposal && target == CoordinatorSource::Local {
            return Err(CoordinatorError::SourceDowngrade);
        }
        let repeated_proposal = current == CoordinatorSource::Proposal
            && target == CoordinatorSource::Proposal
            && (queue_kind.is_some() || waiting_parent);
        if current == target && !repeated_proposal && !waiting_parent {
            return Ok(version);
        }
        // Trusted owners have no expiry. A promoted remote orphan therefore
        // cannot remain in WaitingParents: requeue it through Resolve so the
        // unified trusted-parent policy either observes an in-flight parent
        // or terminalizes an unavailable external dependency.
        let target_queue_kind = queue_kind.or(waiting_parent.then_some(QueueKind::Resolve));
        let reticket = target_queue_kind.is_some();
        let queue_sequence = if reticket {
            Some(self.queue_sequence_range(1)?)
        } else {
            None
        };
        if reticket {
            self.ensure_revision_capacity(hash)?;
        }

        // Source trust participates in CandidateRank. Recompute only the
        // bounded direct cohort and reconcile its derived tickets in the same
        // ownership transaction as the attribution change.
        let (conflict_delta, conflict_force) = if let Some((candidate, location)) = &candidate_state
        {
            let next_rank = CandidateRank::from_entry(hash, target, candidate, location);
            let delta = self.preview_conflict_rerank(hash, &next_rank, location.clone())?;
            let force = if *location == CandidateLocation::Verified {
                HashSet::from([hash.clone()])
            } else {
                HashSet::new()
            };
            (Some(delta), force)
        } else {
            (None, HashSet::new())
        };
        let source_overrides = HashMap::from([(hash.clone(), target)]);
        let mut undo = vec![hash.clone()];
        if let Some(delta) = &conflict_delta {
            undo.extend(delta.affected().iter().cloned());
        }
        undo.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        undo.dedup();
        self.with_entry_undo(&undo, |coordinator| {
            if reticket {
                coordinator
                    .queue_mut(target_queue_kind.ok_or(CoordinatorError::SourceDowngrade)?)
                    .reserve_live(
                        target.queue_owner(),
                        old_ticket.verify_schedule.is_large_cycle,
                    )?;
            }
            let mut conflict_ticket_plan = match &conflict_delta {
                Some(delta) => Some(coordinator.prepare_conflict_ticket_plan(
                    delta,
                    &conflict_force,
                    &source_overrides,
                )?),
                None => None,
            };
            if let Some(plan) = &conflict_ticket_plan {
                coordinator.remove_conflict_tickets(plan)?;
            }
            if let Some(peer) = current.peer() {
                coordinator.release_peer_attribution(hash, peer, old_charge, active)?;
                coordinator.apply_fault_checkpoint();
            }

            if old_charge != new_charge {
                coordinator.global_usage = coordinator
                    .global_usage
                    .checked_sub(old_charge)
                    .and_then(|usage| usage.checked_add(new_charge))
                    .ok_or(CoordinatorError::ConflictInvariant)?;
                coordinator.apply_fault_checkpoint();
            }
            if coordinator.live_deadlines.remove(hash).is_some() != had_live_deadline {
                return Err(CoordinatorError::ConflictInvariant);
            }
            if had_live_deadline {
                coordinator.apply_fault_checkpoint();
            }

            let new_ticket = {
                let entry = coordinator.entry_mut(hash)?;
                entry.source = target;
                entry.expires_at = None;
                entry.base_metadata_bytes = new_base_metadata_bytes;
                entry.metadata_bytes = new_metadata_bytes;
                entry.raw_charge_bytes = new_raw_charge_bytes;
                entry.charge_bytes = new_charge.bytes;
                if target_queue_kind.filter(|_| reticket).is_some() {
                    let (sequence, next_sequence) =
                        queue_sequence.ok_or(CoordinatorError::QueueSequenceExhausted)?;
                    if waiting_parent {
                        let EntryState::Raw { location, .. } = &mut entry.state else {
                            return Err(CoordinatorError::ConflictInvariant);
                        };
                        *location = RawLocation::Queued(RawStage::Resolve);
                    }
                    entry.queue_sequence = sequence;
                    entry.revision += 1;
                    Some((entry.ticket(hash), next_sequence))
                } else {
                    None
                }
            };
            if waiting_parent {
                coordinator.leave_waiting_parent()?;
            }
            if let Some((new_ticket, next_sequence)) = new_ticket {
                coordinator.next_queue_sequence = next_sequence;
                if let Some(old_kind) = queue_kind {
                    coordinator
                        .queue_mut(old_kind)
                        .remove_live(old_kind, &old_ticket)?;
                }
                let kind = target_queue_kind.ok_or(CoordinatorError::SourceDowngrade)?;
                coordinator.queue_mut(kind).push_reserved(
                    kind,
                    new_ticket,
                    target.is_proposal(),
                )?;
            }
            if let Some(delta) = &conflict_delta {
                coordinator.apply_conflict_delta(delta)?;
            }
            if let Some(plan) = conflict_ticket_plan.take() {
                coordinator.apply_conflict_ticket_plan(plan)?;
            }
            coordinator.apply_fault_checkpoint();
            coordinator.compact_deadlines();
            coordinator
                .entries
                .get(hash)
                .map(CoordinatorEntry::version)
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))
        })
    }

    /// Install a trusted witness-bearing raw payload for an existing raw-hash
    /// owner and restart its lifecycle from the requested raw stage. A raw
    /// hash deliberately excludes witnesses, so source promotion alone is not
    /// sufficient when a Proposal/Local payload differs from the one first
    /// received from the network.
    ///
    /// The replacement keeps the existing incarnation and dependency graph,
    /// but advances the revision, removes every later-phase/conflict claim and
    /// invalidates any outstanding worker lease. Capacity reconciliation uses
    /// the trusted target strength, allowing a larger authoritative witness to
    /// displace weaker work instead of being pinned by a small remote variant.
    pub(crate) fn replace_raw_payload(
        &mut self,
        hash: &Byte32,
        raw: R,
        raw_payload_bytes: usize,
        promotion: TrustedSource,
        stage: RawStage,
    ) -> Result<(CoordinatorVersion, Vec<TerminalRecord<R>>), CoordinatorError> {
        let target = match promotion {
            TrustedSource::Local => CoordinatorSource::Local,
            TrustedSource::Proposal => CoordinatorSource::Proposal,
        };
        let entry = self
            .entries
            .get(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        if entry.is_committing() {
            return Err(CoordinatorError::CommitInProgress(hash.clone()));
        }
        if entry.source == CoordinatorSource::Proposal && target == CoordinatorSource::Local {
            return Err(CoordinatorError::SourceDowngrade);
        }
        let base_metadata_bytes = self.metadata_charge_bytes(entry.dependencies.len(), false, 0)?;
        let total_charge_bytes = raw_payload_bytes
            .checked_add(base_metadata_bytes)
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        let protected = self.dependency_ancestor_closure(hash, &entry.dependencies)?;
        let victims = self.global_capacity_victims(
            Some(hash),
            target,
            total_charge_bytes,
            &HashSet::new(),
            &protected,
        )?;
        let subject = CapacitySubject::Present(hash.clone());
        let subject_undo = self.causal_undo_hashes(std::slice::from_ref(hash));
        self.with_capacity_victims(subject, victims, subject_undo, move |coordinator| {
            coordinator.replace_raw_payload_inner(hash, raw, raw_payload_bytes, promotion, stage)
        })
    }

    fn replace_raw_payload_inner(
        &mut self,
        hash: &Byte32,
        raw: R,
        raw_payload_bytes: usize,
        promotion: TrustedSource,
        stage: RawStage,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        let target = match promotion {
            TrustedSource::Local => CoordinatorSource::Local,
            TrustedSource::Proposal => CoordinatorSource::Proposal,
        };
        let undo = self.causal_undo_hashes(std::slice::from_ref(hash));
        self.require_entry_transaction(&undo)?;
        (move |coordinator: &mut Self| {
            let current = coordinator
                .entries
                .get(hash)
                .map(|entry| entry.source)
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            if current == CoordinatorSource::Proposal && target == CoordinatorSource::Local {
                return Err(CoordinatorError::SourceDowngrade);
            }
            if coordinator
                .entries
                .get(hash)
                .is_some_and(CoordinatorEntry::is_committing)
            {
                return Err(CoordinatorError::CommitInProgress(hash.clone()));
            }
            coordinator.ensure_revision_capacity(hash)?;
            let (dependencies, active, had_live_deadline, invalidated) = coordinator
                .entries
                .get(hash)
                .map(|entry| {
                    (
                        entry.dependencies.len(),
                        entry.uses_active_slot(),
                        entry.expires_at.is_some(),
                        entry.invalidated_cause().is_some(),
                    )
                })
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            let base_metadata_bytes = coordinator.metadata_charge_bytes(dependencies, false, 0)?;
            let total_charge_bytes = raw_payload_bytes
                .checked_add(base_metadata_bytes)
                .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
            coordinator.preflight_remove_conflict_indexes(hash)?;

            let queue_kind = match stage {
                RawStage::PreCheck => QueueKind::PreCheck,
                RawStage::Resolve => QueueKind::Resolve,
            };
            coordinator
                .queue_mut(queue_kind)
                .reserve_live(target.queue_owner(), false)?;
            let (queue_sequence, next_queue_sequence) = coordinator.queue_sequence_range(1)?;

            coordinator.remove_current_scheduling(hash)?;
            if active {
                coordinator.deactivate_source(current)?;
            }
            if coordinator.live_deadlines.remove(hash).is_some() != had_live_deadline
                || coordinator.dependency_failure_set.remove(hash) != invalidated
            {
                return Err(CoordinatorError::ConflictInvariant);
            }
            coordinator.compact_deadlines();
            coordinator.compact_dependency_failures();
            coordinator.apply_recharge(hash, total_charge_bytes)?;
            if let Some(peer) = current.peer() {
                coordinator.release_peer_attribution(
                    hash,
                    peer,
                    CoordinatorResidency::new(1, total_charge_bytes),
                    false,
                )?;
            }

            let was_waiting = coordinator.entries.get(hash).is_some_and(|entry| {
                matches!(
                    &entry.state,
                    EntryState::Raw {
                        location: RawLocation::WaitingParents { .. },
                        ..
                    }
                )
            });
            if was_waiting {
                coordinator.leave_waiting_parent()?;
            }

            let entry = coordinator.entry_mut(hash)?;
            entry.source = target;
            entry.expires_at = None;
            entry.raw_charge_bytes = total_charge_bytes;
            entry.raw_resident_payload_bytes = raw_payload_bytes;
            entry.resident_payload_bytes = raw_payload_bytes;
            entry.base_metadata_bytes = base_metadata_bytes;
            entry.metadata_bytes = base_metadata_bytes;
            entry.queue_sequence = queue_sequence;
            entry.state = EntryState::Raw {
                raw: Arc::new(raw),
                location: RawLocation::Queued(stage),
            };
            entry.revision += 1;
            let version = entry.version();
            let ticket = entry.ticket(hash);
            coordinator.queue_mut(queue_kind).push_reserved(
                queue_kind,
                ticket,
                target.is_proposal(),
            )?;
            coordinator.next_queue_sequence = next_queue_sequence;
            coordinator.apply_fault_checkpoint();
            Ok(version)
        })(self)
    }

    pub(crate) fn checkout_raw(
        &mut self,
        stage: RawStage,
    ) -> Result<Option<RawWorkLease<R>>, CoordinatorError> {
        let kind = match stage {
            RawStage::PreCheck => QueueKind::PreCheck,
            RawStage::Resolve => QueueKind::Resolve,
        };
        let Some(ticket) = self.peek_live_ticket(kind, WorkerCapability::Any)? else {
            return Ok(None);
        };
        let expected = CoordinatorLocation::RawQueued(stage);
        self.validate_version_location(&ticket.hash, ticket.version, &expected)?;
        self.ensure_revision_capacity(&ticket.hash)?;
        let source = self
            .entries
            .get(&ticket.hash)
            .map(|entry| entry.source)
            .ok_or_else(|| CoordinatorError::Missing(ticket.hash.clone()))?;
        self.check_activate_source(source)?;
        self.consume_front_ticket(kind, &ticket)?;
        self.activate_source(source)?;
        let entry = self.entry_mut(&ticket.hash)?;
        let EntryState::Raw { raw, location } = &mut entry.state else {
            return Err(CoordinatorError::ConflictInvariant);
        };
        *location = RawLocation::Active(stage);
        let payload = Arc::clone(raw);
        entry.revision += 1;
        Ok(Some(RawWorkLease {
            hash: ticket.hash,
            stage,
            version: entry.version(),
            payload,
        }))
    }

    /// Terminalize exactly the active raw owner represented by `lease`.
    /// Administrative hash-only removal remains separate because it has
    /// different stale-worker semantics.
    pub(crate) fn terminalize_raw(
        &mut self,
        lease: &RawWorkLease<R>,
        disposition: TerminalDisposition,
    ) -> Result<TerminalRecord<R>, CoordinatorError> {
        let expected = CoordinatorLocation::RawActive(lease.stage);
        self.validate_version_location(&lease.hash, lease.version, &expected)?;
        self.terminalize_present_causally(&lease.hash, disposition)
    }

    /// Terminalize exactly one active verification lease. Source promotion
    /// may update attribution without invalidating the work lease; the final
    /// terminal record therefore takes its source from the authoritative
    /// entry, not from a worker snapshot.
    pub(crate) fn terminalize_verification(
        &mut self,
        lease: &VerifyWorkLease<U>,
        disposition: TerminalDisposition,
    ) -> Result<TerminalRecord<R>, CoordinatorError> {
        self.validate_version_location(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::VerifyActive,
        )?;
        self.terminalize_present_causally(&lease.hash, disposition)
    }

    /// A chain update can make an input disappear after resolution but before
    /// script verification completes. Preserve the transaction under the
    /// coordinator instead of terminalizing it into a second orphan owner:
    /// discard the stale resolved payload, recharge the raw phase, and either
    /// wait for the exact still-missing parents or requeue resolution when a
    /// TxPool/coordinator handoff made every reported parent available.
    pub(crate) fn verification_retry_resolution(
        &mut self,
        lease: &VerifyWorkLease<U>,
        missing: HashSet<Byte32>,
    ) -> Result<(CoordinatorVersion, CoordinatorSource), CoordinatorError> {
        let missing = missing
            .into_iter()
            .map(|parent| crate::util::compact_packed(&parent))
            .collect::<HashSet<_>>();
        self.validate_version_location(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::VerifyActive,
        )?;
        let (dependencies, added, base_metadata_bytes, raw_charge_bytes) =
            self.plan_discovered_dependencies(&lease.hash, &missing)?;
        self.check_recharge(&lease.hash, raw_charge_bytes)?;
        self.ensure_revision_capacity(&lease.hash)?;
        let requeue = missing.is_empty();
        let (queue_sequence, next_queue_sequence) = if requeue {
            let (first, next) = self.queue_sequence_range(1)?;
            (Some(first), Some(next))
        } else {
            (None, None)
        };
        self.with_entry_undo(std::slice::from_ref(&lease.hash), |coordinator| {
            let source = coordinator
                .entries
                .get(&lease.hash)
                .map(|entry| entry.source)
                .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
            if requeue {
                coordinator
                    .queue_mut(QueueKind::Resolve)
                    .reserve_live(source.queue_owner(), false)?;
            }
            coordinator.deactivate_source(source)?;
            coordinator.apply_recharge(&lease.hash, raw_charge_bytes)?;
            if !requeue {
                coordinator.enter_waiting_parent()?;
            }
            for parent in &added {
                coordinator.insert_parent_membership(parent, &lease.hash)?;
            }
            let version = {
                let entry = coordinator.entry_mut(&lease.hash)?;
                let raw = Arc::clone(entry.state.raw());
                let location = if requeue {
                    RawLocation::Queued(RawStage::Resolve)
                } else {
                    RawLocation::WaitingParents { missing }
                };
                entry.state = EntryState::Raw { raw, location };
                entry.dependencies = dependencies;
                entry.base_metadata_bytes = base_metadata_bytes;
                entry.raw_charge_bytes = raw_charge_bytes;
                entry.resident_payload_bytes = entry.raw_resident_payload_bytes;
                entry.metadata_bytes = base_metadata_bytes;
                if let Some(queue_sequence) = queue_sequence {
                    entry.queue_sequence = queue_sequence;
                }
                entry.revision += 1;
                entry.version()
            };
            if let Some(next_queue_sequence) = next_queue_sequence {
                let entry = coordinator
                    .entries
                    .get(&lease.hash)
                    .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
                let ticket = entry.ticket(&lease.hash);
                let priority = entry.source.is_proposal();
                coordinator.queue_mut(QueueKind::Resolve).push_reserved(
                    QueueKind::Resolve,
                    ticket,
                    priority,
                )?;
                coordinator.next_queue_sequence = next_queue_sequence;
            }
            coordinator.apply_fault_checkpoint();
            Ok((version, source))
        })
    }

    /// Replace raw work with an unverified phase bundle and atomically extend
    /// its causal graph with dependencies learned from successful dep-group
    /// expansion. Live expanded members matter just as much as missing ones:
    /// a later parent removal must invalidate this resolved payload before it
    /// reaches commit.
    pub(crate) fn complete_raw_with_dependencies(
        &mut self,
        lease: &RawWorkLease<R>,
        unverified: U,
        charge_bytes: usize,
        verify_schedule: VerifySchedule,
        discovered_dependencies: HashSet<Byte32>,
    ) -> Result<(CoordinatorVersion, Vec<TerminalRecord<R>>), CoordinatorError> {
        let expected = CoordinatorLocation::RawActive(lease.stage);
        self.validate_version_location(&lease.hash, lease.version, &expected)?;
        let (dependencies, added, base_metadata_bytes, raw_charge_bytes) =
            self.plan_discovered_dependencies(&lease.hash, &discovered_dependencies)?;
        let total_charge_bytes = charge_bytes
            .checked_add(base_metadata_bytes)
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        let source = self
            .entries
            .get(&lease.hash)
            .map(|entry| entry.source)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        let protected = self.dependency_ancestor_closure(&lease.hash, &dependencies)?;
        self.check_peer_budget_after_victims(
            Some(&lease.hash),
            source,
            total_charge_bytes,
            &HashSet::new(),
        )?;
        let victims = self.global_capacity_victims(
            Some(&lease.hash),
            source,
            total_charge_bytes,
            &HashSet::new(),
            &protected,
        )?;
        let subject = CapacitySubject::Present(lease.hash.clone());
        self.with_capacity_victims(subject, victims, Vec::new(), move |coordinator| {
            coordinator.complete_raw_inner(
                lease,
                unverified,
                charge_bytes,
                verify_schedule,
                dependencies,
                added,
                base_metadata_bytes,
                raw_charge_bytes,
            )
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn complete_raw_inner(
        &mut self,
        lease: &RawWorkLease<R>,
        unverified: U,
        charge_bytes: usize,
        verify_schedule: VerifySchedule,
        dependencies: HashSet<Byte32>,
        added: Vec<Byte32>,
        base_metadata_bytes: usize,
        raw_charge_bytes: usize,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        let expected = CoordinatorLocation::RawActive(lease.stage);
        self.validate_version_location(&lease.hash, lease.version, &expected)?;
        self.ensure_revision_capacity(&lease.hash)?;
        let total_charge_bytes = charge_bytes
            .checked_add(base_metadata_bytes)
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        self.check_recharge(&lease.hash, total_charge_bytes)?;
        let source = self
            .entries
            .get(&lease.hash)
            .map(|entry| entry.source)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        self.queue_mut(QueueKind::Verify)
            .reserve_live(source.queue_owner(), verify_schedule.is_large_cycle)?;
        let (queue_sequence, next_queue_sequence) = self.queue_sequence_range(1)?;
        self.deactivate_source(source)?;
        self.apply_recharge(&lease.hash, total_charge_bytes)?;
        for parent in &added {
            self.insert_parent_membership(parent, &lease.hash)?;
        }
        let entry = self.entry_mut(&lease.hash)?;
        let raw = Arc::clone(entry.state.raw());
        entry.state = EntryState::Unverified {
            raw,
            payload: Arc::new(unverified),
            location: UnverifiedLocation::Queued,
            verify_schedule,
        };
        entry.dependencies = dependencies;
        entry.base_metadata_bytes = base_metadata_bytes;
        entry.raw_charge_bytes = raw_charge_bytes;
        entry.resident_payload_bytes = charge_bytes;
        entry.metadata_bytes = base_metadata_bytes;
        entry.queue_sequence = queue_sequence;
        entry.revision += 1;
        let version = entry.version();
        let ticket = entry.ticket(&lease.hash);
        let front = entry.source.is_proposal();
        self.queue_mut(QueueKind::Verify)
            .push_reserved(QueueKind::Verify, ticket, front)?;
        self.next_queue_sequence = next_queue_sequence;
        Ok(version)
    }

    pub(crate) fn wait_for_parents(
        &mut self,
        lease: &RawWorkLease<R>,
        missing: HashSet<Byte32>,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        let missing = missing
            .into_iter()
            .map(|parent| crate::util::compact_packed(&parent))
            .collect::<HashSet<_>>();
        let expected = CoordinatorLocation::RawActive(lease.stage);
        self.validate_version_location(&lease.hash, lease.version, &expected)?;
        if missing.is_empty() {
            return self.requeue_raw(lease);
        }
        let (dependencies, added, base_metadata_bytes, raw_charge_bytes) =
            self.plan_discovered_dependencies(&lease.hash, &missing)?;
        self.check_recharge(&lease.hash, raw_charge_bytes)?;
        self.ensure_revision_capacity(&lease.hash)?;
        let next_waiting_parent_count = self
            .waiting_parent_count
            .checked_add(1)
            .ok_or(CoordinatorError::ConflictInvariant)?;
        let source = self
            .entries
            .get(&lease.hash)
            .map(|entry| entry.source)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        self.with_entry_undo(std::slice::from_ref(&lease.hash), |coordinator| {
            coordinator.deactivate_source(source)?;
            coordinator.apply_recharge(&lease.hash, raw_charge_bytes)?;
            for parent in &added {
                coordinator.insert_parent_membership(parent, &lease.hash)?;
            }
            let entry = coordinator.entry_mut(&lease.hash)?;
            let EntryState::Raw { location, .. } = &mut entry.state else {
                return Err(CoordinatorError::ConflictInvariant);
            };
            *location = RawLocation::WaitingParents { missing };
            entry.dependencies = dependencies;
            entry.base_metadata_bytes = base_metadata_bytes;
            entry.metadata_bytes = base_metadata_bytes;
            entry.raw_charge_bytes = raw_charge_bytes;
            entry.revision += 1;
            let version = entry.version();
            coordinator.waiting_parent_count = next_waiting_parent_count;
            coordinator.apply_fault_checkpoint();
            Ok(version)
        })
    }

    pub(crate) fn requeue_raw(
        &mut self,
        lease: &RawWorkLease<R>,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        let expected = CoordinatorLocation::RawActive(lease.stage);
        self.validate_version_location(&lease.hash, lease.version, &expected)?;
        self.ensure_revision_capacity(&lease.hash)?;
        let kind = match lease.stage {
            RawStage::PreCheck => QueueKind::PreCheck,
            RawStage::Resolve => QueueKind::Resolve,
        };
        let source = self
            .entries
            .get(&lease.hash)
            .map(|entry| entry.source)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        let (queue_sequence, next_queue_sequence) = self.queue_sequence_range(1)?;
        self.with_entry_undo(std::slice::from_ref(&lease.hash), |coordinator| {
            coordinator
                .queue_mut(kind)
                .reserve_live(source.queue_owner(), false)?;
            coordinator.deactivate_source(source)?;
            let entry = coordinator.entry_mut(&lease.hash)?;
            let EntryState::Raw { location, .. } = &mut entry.state else {
                return Err(CoordinatorError::ConflictInvariant);
            };
            *location = RawLocation::Queued(lease.stage);
            entry.queue_sequence = queue_sequence;
            entry.revision += 1;
            let version = entry.version();
            let ticket = entry.ticket(&lease.hash);
            let front = entry.source.is_proposal();
            coordinator
                .queue_mut(kind)
                .push_reserved(kind, ticket, front)?;
            coordinator.next_queue_sequence = next_queue_sequence;
            Ok(version)
        })
    }

    pub(super) fn parent_available_apply(
        &mut self,
        parent: &Byte32,
    ) -> Result<Vec<CoordinatorTicket>, CoordinatorError> {
        let mut children: Vec<_> = self
            .by_parent
            .get(parent)
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        children.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        let mut affected = Vec::new();
        let mut ready_count = 0usize;
        let mut ready_owners = Vec::new();
        for child in children {
            let Some(entry) = self.entries.get(&child) else {
                continue;
            };
            let EntryState::Raw {
                location: RawLocation::WaitingParents { missing },
                ..
            } = &entry.state
            else {
                continue;
            };
            if !missing.contains(parent) {
                continue;
            }
            self.ensure_revision_capacity(&child)?;
            if missing.len() == 1 {
                ready_count = ready_count
                    .checked_add(1)
                    .ok_or(CoordinatorError::QueueReservationFailed)?;
                ready_owners.push(entry.source.queue_owner());
            }
            affected.push(child);
        }
        let mut ready = Vec::new();
        ready
            .try_reserve(ready_count)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        let (first_queue_sequence, next_queue_sequence) = self.queue_sequence_range(ready_count)?;
        self.require_entry_transaction(&affected)?;
        self.queue_mut(QueueKind::Resolve)
            .reserve_many(ready_owners, false)?;
        self.next_queue_sequence = next_queue_sequence;
        let mut queue_sequence = first_queue_sequence;
        for child in affected {
            let entry = self.entry_mut(&child)?;
            let missing = match &mut entry.state {
                EntryState::Raw {
                    location: RawLocation::WaitingParents { missing },
                    ..
                } => missing,
                state => {
                    return Err(CoordinatorError::LocationMismatch {
                        expected: CoordinatorLocation::WaitingParents {
                            missing: HashSet::from([parent.clone()]),
                        },
                        actual: state.location(),
                    });
                }
            };
            if !missing.remove(parent) {
                return Err(CoordinatorError::ConflictInvariant);
            }
            let ready_now = missing.is_empty();
            entry.revision += 1;
            if ready_now {
                let EntryState::Raw { location, .. } = &mut entry.state else {
                    return Err(CoordinatorError::ConflictInvariant);
                };
                *location = RawLocation::Queued(RawStage::Resolve);
                entry.queue_sequence = queue_sequence;
                queue_sequence = queue_sequence
                    .checked_add(1)
                    .ok_or(CoordinatorError::QueueSequenceExhausted)?;
                let ticket = entry.ticket(&child);
                let front = entry.source.is_proposal();
                self.leave_waiting_parent()?;
                self.queue_mut(QueueKind::Resolve).push_reserved(
                    QueueKind::Resolve,
                    ticket.clone(),
                    front,
                )?;
                ready.push(ticket);
            }
            self.apply_fault_checkpoint();
        }
        Ok(ready)
    }

    /// Atomically reclassify every coordinator transaction whose dependency is
    /// in `parents`. Expiring Remote owners may wait for retransmission;
    /// non-expiring Local/Proposal owners become causal terminal work instead
    /// of parking forever. Administrative pool removal uses this before
    /// deleting a root and its accepted descendants, so no already-resolved
    /// consumer can outlive any member of the removed closure. Every child is
    /// transitioned once even when several parents disappear together.
    pub(crate) fn parents_unavailable(
        &mut self,
        parents: &HashSet<Byte32>,
    ) -> Result<Vec<Byte32>, CoordinatorError> {
        let plan = self.plan_parents_unavailable(parents)?;
        let undo = plan.undo.clone();
        self.with_entry_undo(&undo, |coordinator| {
            coordinator.apply_parents_unavailable(plan)
        })
    }

    pub(super) fn plan_parents_unavailable(
        &mut self,
        parents: &HashSet<Byte32>,
    ) -> Result<ParentsUnavailablePlan, CoordinatorError> {
        let mut ordered_parents: Vec<_> = parents.iter().cloned().collect();
        ordered_parents.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        let mut missing_by_child: HashMap<Byte32, HashSet<Byte32>> = HashMap::new();
        for parent in ordered_parents {
            let mut children: Vec<_> = self
                .by_parent
                .get(&parent)
                .into_iter()
                .flatten()
                .cloned()
                .collect();
            children.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
            for child in children {
                let Some(entry) = self.entries.get(&child) else {
                    continue;
                };
                // Definitive dependency failure has precedence over a later
                // availability transition for another parent.
                if entry.invalidated_cause().is_some() {
                    continue;
                }
                let already_missing = matches!(
                    &entry.state,
                    EntryState::Raw {
                        location: RawLocation::WaitingParents { missing },
                        ..
                    } if missing.contains(&parent)
                );
                if !already_missing {
                    missing_by_child
                        .entry(child)
                        .or_default()
                        .insert(parent.clone());
                }
            }
        }

        let mut affected: Vec<_> = missing_by_child.keys().cloned().collect();
        affected.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        let trusted: Vec<_> = affected
            .iter()
            .filter(|child| {
                self.entries
                    .get(*child)
                    .is_some_and(|entry| !matches!(entry.source, CoordinatorSource::Remote(_)))
            })
            .cloned()
            .collect();
        for child in &affected {
            self.ensure_revision_capacity(child)?;
            self.preflight_remove_conflict_indexes(child)?;
        }
        // A Remote owner may wait for retransmission under its original
        // expiry. Local/Proposal owners deliberately have no expiry, so
        // parking them here would create permanent high-priority residency.
        // Pre-reserve their bounded terminal-maintenance tickets instead.
        self.dependency_failures
            .try_reserve(trusted.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.dependency_failure_set
            .try_reserve(trusted.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        let (first_sequence, next_maintenance_sequence) =
            self.maintenance_sequence_range(trusted.len())?;
        let trusted_sequences: HashMap<_, _> = trusted
            .iter()
            .enumerate()
            .map(|(offset, child)| {
                let offset = u64::try_from(offset)
                    .map_err(|_| CoordinatorError::MaintenanceSequenceExhausted)?;
                let sequence = first_sequence
                    .checked_add(offset)
                    .ok_or(CoordinatorError::MaintenanceSequenceExhausted)?;
                Ok((child.clone(), sequence))
            })
            .collect::<Result<_, CoordinatorError>>()?;

        let mut undo = self.conflict_undo_hashes(&affected);
        undo.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        undo.dedup();
        Ok(ParentsUnavailablePlan {
            undo,
            affected,
            missing_by_child,
            trusted_sequences,
            next_maintenance_sequence,
        })
    }

    pub(super) fn apply_parents_unavailable(
        &mut self,
        plan: ParentsUnavailablePlan,
    ) -> Result<Vec<Byte32>, CoordinatorError> {
        let ParentsUnavailablePlan {
            undo,
            affected,
            missing_by_child,
            trusted_sequences,
            next_maintenance_sequence,
        } = plan;
        self.require_entry_transaction(&undo)?;
        self.next_maintenance_sequence = next_maintenance_sequence;
        for child in &affected {
            if let Some(sequence) = trusted_sequences.get(child) {
                let cause = missing_by_child
                    .get(child)
                    .and_then(|missing| {
                        missing
                            .iter()
                            .min_by(|left, right| left.as_slice().cmp(right.as_slice()))
                    })
                    .cloned()
                    .ok_or(CoordinatorError::ConflictInvariant)?;
                self.invalidate_present_apply(child, &cause, *sequence)?;
                continue;
            }

            let active_source = self
                .entries
                .get(child)
                .and_then(|entry| entry.uses_active_slot().then_some(entry.source));
            if let Some(source) = active_source {
                self.deactivate_source(source)?;
            }
            self.remove_current_scheduling(child)?;
            self.apply_fault_checkpoint();
            let was_waiting = self.entries.get(child).is_some_and(|entry| {
                matches!(
                    &entry.state,
                    EntryState::Raw {
                        location: RawLocation::WaitingParents { .. },
                        ..
                    }
                )
            });
            let raw_charge = self
                .entries
                .get(child)
                .ok_or_else(|| CoordinatorError::Missing(child.clone()))?
                .raw_charge_bytes;
            self.apply_recharge(child, raw_charge)?;
            let entry = self.entry_mut(child)?;
            let mut missing = match &entry.state {
                EntryState::Raw {
                    location: RawLocation::WaitingParents { missing },
                    ..
                } => missing.clone(),
                _ => HashSet::new(),
            };
            missing.extend(
                missing_by_child
                    .get(child)
                    .ok_or(CoordinatorError::ConflictInvariant)?
                    .iter()
                    .cloned(),
            );
            entry.resident_payload_bytes = entry.raw_resident_payload_bytes;
            entry.metadata_bytes = entry.base_metadata_bytes;
            let raw = Arc::clone(entry.state.raw());
            entry.state = EntryState::Raw {
                raw,
                location: RawLocation::WaitingParents { missing },
            };
            entry.revision += 1;
            if !was_waiting {
                self.enter_waiting_parent()?;
            }
            self.apply_fault_checkpoint();
        }
        Ok(affected)
    }

    pub(crate) fn drain_dependency_failures(
        &mut self,
        max: usize,
    ) -> Result<Vec<TerminalRecord<R>>, CoordinatorError> {
        let roots = self.preview_dependency_failure_roots(max);
        let mut affected: HashSet<_> = roots.iter().cloned().collect();
        for root in &roots {
            if let Some(children) = self.by_parent.get(root) {
                affected.extend(children.iter().cloned());
            }
        }
        let roots_and_children: Vec<_> = affected.into_iter().collect();
        let mut affected = self.causal_undo_hashes(&roots_and_children);
        affected.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        let mut terminal = Vec::new();
        terminal
            .try_reserve(roots.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.with_entry_undo(&affected, |coordinator| {
            for hash in roots {
                let cause = coordinator
                    .entries
                    .get(&hash)
                    .and_then(CoordinatorEntry::invalidated_cause)
                    .cloned()
                    .ok_or(CoordinatorError::ConflictInvariant)?;
                terminal.push(coordinator.terminalize_present_apply(
                    hash,
                    Some(cause),
                    TerminalDisposition::DependencyFailed,
                )?);
            }
            Ok(terminal)
        })
    }

    pub(crate) fn dependency_failure_len(&self) -> usize {
        self.dependency_failure_set.len()
    }

    pub(crate) fn checkout_verify(
        &mut self,
        capability: WorkerCapability,
    ) -> Result<Option<VerifyWorkLease<U>>, CoordinatorError> {
        let Some(ticket) = self.peek_live_ticket(QueueKind::Verify, capability)? else {
            return Ok(None);
        };
        self.validate_version_location(
            &ticket.hash,
            ticket.version,
            &CoordinatorLocation::VerifyQueued,
        )?;
        self.ensure_revision_capacity(&ticket.hash)?;
        let source = self
            .entries
            .get(&ticket.hash)
            .map(|entry| entry.source)
            .ok_or_else(|| CoordinatorError::Missing(ticket.hash.clone()))?;
        self.check_activate_source(source)?;
        self.consume_front_ticket(QueueKind::Verify, &ticket)?;
        self.activate_source(source)?;
        let entry = self.entry_mut(&ticket.hash)?;
        let payload = match &mut entry.state {
            EntryState::Unverified {
                payload, location, ..
            } => {
                *location = UnverifiedLocation::Active;
                Arc::clone(payload)
            }
            _ => return Err(CoordinatorError::ConflictInvariant),
        };
        entry.revision += 1;
        Ok(Some(VerifyWorkLease {
            hash: ticket.hash,
            version: entry.version(),
            payload,
        }))
    }

    /// Install a verified conflict candidate. `charge_bytes` covers the
    /// complete resident phase bundle; conflict index metadata is added by
    /// the coordinator separately.
    pub(crate) fn complete_verification_candidate(
        &mut self,
        lease: &VerifyWorkLease<U>,
        verified: V,
        charge_bytes: usize,
        candidate: VerifiedCandidate,
    ) -> Result<(CoordinatorVersion, Vec<TerminalRecord<R>>), CoordinatorError> {
        // Candidate inputs become keys in shared conflict indexes and can
        // outlive whichever transaction first introduced an outpoint.
        let candidate = VerifiedCandidate {
            inputs: candidate
                .inputs
                .into_iter()
                .map(|input| crate::util::compact_packed(&input))
                .collect(),
            fee: candidate.fee,
            tx_size: candidate.tx_size,
        };
        self.validate_version_location(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::VerifyActive,
        )?;
        if candidate.inputs.len() > self.limits.max_conflict_inputs_per_entry {
            return Err(CoordinatorError::ConflictInputLimitExceeded);
        }
        let entry = self
            .entries
            .get(&lease.hash)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        let source = entry.source;
        let protected = self.dependency_ancestor_closure(&lease.hash, &entry.dependencies)?;
        let incoming = CandidateMeta {
            inputs: candidate.inputs.clone(),
            fee: candidate.fee,
            tx_size: candidate.tx_size,
            arrival: self.next_arrival,
        };
        let subject_undo: Vec<_> = self
            .conflicting_candidates_for_undo(&lease.hash, &incoming.inputs)?
            .into_iter()
            .collect();
        let mut victims =
            self.conflict_capacity_victims(&lease.hash, source, &incoming, &protected)?;
        let metadata_bytes = self.metadata_charge_bytes(
            entry.dependencies.len(),
            entry.expires_at.is_some(),
            candidate.inputs.len(),
        )?;
        let total_charge_bytes = charge_bytes
            .checked_add(metadata_bytes)
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        let selected: HashSet<_> = victims.iter().cloned().collect();
        self.check_peer_budget_after_victims(
            Some(&lease.hash),
            source,
            total_charge_bytes,
            &selected,
        )?;
        victims.extend(self.global_capacity_victims(
            Some(&lease.hash),
            source,
            total_charge_bytes,
            &selected,
            &protected,
        )?);
        victims.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        victims.dedup();
        let subject = CapacitySubject::Present(lease.hash.clone());
        self.with_capacity_victims(subject, victims, subject_undo, move |coordinator| {
            coordinator.complete_verification_candidate_inner(
                lease,
                verified,
                charge_bytes,
                candidate,
            )
        })
    }

    fn complete_verification_candidate_inner(
        &mut self,
        lease: &VerifyWorkLease<U>,
        verified: V,
        charge_bytes: usize,
        candidate: VerifiedCandidate,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        self.validate_version_location(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::VerifyActive,
        )?;
        self.ensure_revision_capacity(&lease.hash)?;
        if candidate.inputs.len() > self.limits.max_conflict_inputs_per_entry {
            return Err(CoordinatorError::ConflictInputLimitExceeded);
        }
        let (dependencies, has_deadline, source) = self
            .entries
            .get(&lease.hash)
            .map(|entry| {
                (
                    entry.dependencies.len(),
                    entry.expires_at.is_some(),
                    entry.source,
                )
            })
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        let metadata_bytes =
            self.metadata_charge_bytes(dependencies, has_deadline, candidate.inputs.len())?;
        let total_charge_bytes = charge_bytes
            .checked_add(metadata_bytes)
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
        self.check_recharge(&lease.hash, total_charge_bytes)?;
        let arrival = self.next_arrival;
        let next_arrival = arrival
            .checked_add(1)
            .ok_or(CoordinatorError::ArrivalSequenceExhausted)?;
        let meta = CandidateMeta {
            inputs: candidate.inputs,
            fee: candidate.fee,
            tx_size: candidate.tx_size,
            arrival,
        };
        let delta = self.preview_conflict_insert(&lease.hash, source, &meta)?;
        let undo = delta.affected().to_vec();
        self.require_entry_transaction(&undo)?;
        (|coordinator: &mut Self| {
            let ticket_plan = coordinator.prepare_conflict_ticket_plan(
                &delta,
                &HashSet::new(),
                &HashMap::new(),
            )?;
            coordinator.remove_conflict_tickets(&ticket_plan)?;
            let source = coordinator
                .entries
                .get(&lease.hash)
                .map(|entry| entry.source)
                .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
            coordinator.deactivate_source(source)?;
            coordinator.apply_recharge(&lease.hash, total_charge_bytes)?;
            coordinator.next_arrival = next_arrival;
            coordinator.apply_fault_checkpoint();
            let entry = coordinator.entry_mut(&lease.hash)?;
            let raw = Arc::clone(entry.state.raw());
            entry.state = EntryState::CandidateVerified {
                raw,
                payload: Arc::new(verified),
                candidate: meta,
                location: CandidateLocation::Verified,
            };
            entry.resident_payload_bytes = charge_bytes;
            entry.metadata_bytes = metadata_bytes;
            coordinator.apply_conflict_delta(&delta)?;
            if !ticket_plan.revises(&lease.hash) {
                coordinator.entry_mut(&lease.hash)?.revision += 1;
            }
            coordinator.apply_conflict_ticket_plan(ticket_plan)?;
            coordinator.apply_fault_checkpoint();
            coordinator
                .entries
                .get(&lease.hash)
                .map(CoordinatorEntry::version)
                .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))
        })(self)
    }
}
