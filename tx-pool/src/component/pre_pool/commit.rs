use super::lifecycle::{MutationSet, PreparedKernelMutation};
use super::*;

type CommitHandoffDesired = (MutationSet, EntryRevision);

/// Optional accepted-victim history carried into one admission handoff.
///
/// This is not a third ownership protocol: the matching accepted entry is
/// removed by the same `AdmissionPlan` which applies the kernel cohort.  If
/// the bounded history partition cannot hold the complete optional set, the
/// handoff deterministically drops that set and still commits the winner.
pub(crate) struct ConflictRetention {
    raw: PipelineRawTx,
    source: PrePoolSource,
    keys: BTreeSet<DependencyKey>,
    expires_at: Option<u64>,
}

impl ConflictRetention {
    pub(crate) fn new(
        raw: PipelineRawTx,
        source: PrePoolSource,
        keys: BTreeSet<DependencyKey>,
        expires_at: Option<u64>,
    ) -> Self {
        Self {
            raw,
            source,
            keys,
            expires_at,
        }
    }
}

/// Read-only pre-pool half of a Ready-to-accepted admission.
pub(crate) struct ReadyCommitPlan<'authority> {
    prepared: PreparedKernelMutation<'authority>,
    settlement: CommitSettlement,
}

impl ReadyCommitPlan<'_> {
    pub(crate) fn settlement(&self) -> &CommitSettlement {
        &self.settlement
    }

    pub(crate) fn apply(self) {
        self.prepared.apply();
    }
}

/// Read-only pre-pool half of a direct/external accepted admission.
pub(crate) struct ExternalCommitPlan<'authority> {
    prepared: PreparedKernelMutation<'authority>,
    records: Vec<TerminalRecord>,
}

impl ExternalCommitPlan<'_> {
    pub(crate) fn records(&self) -> &[TerminalRecord] {
        &self.records
    }

    pub(crate) fn apply(self) {
        self.prepared.apply();
    }
}

/// Read-only terminal settlement for a Ready candidate rejected by the final
/// accepted-pool plan.  Applying this value cannot discover a new capacity,
/// identity or location error.
pub(crate) struct FailedCommitPlan<'authority> {
    prepared: PreparedKernelMutation<'authority>,
    record: TerminalRecord,
}

impl FailedCommitPlan<'_> {
    pub(crate) fn record(&self) -> &TerminalRecord {
        &self.record
    }

    pub(crate) fn apply(self) -> TerminalRecord {
        self.prepared.apply();
        self.record
    }
}

impl ReadyCommitSession<'_> {
    pub(crate) fn payload(&self) -> &Arc<PipelineVerifiedTx> {
        &self.candidate.payload
    }

    pub(crate) fn ingress_peer(&self) -> Option<PeerIndex> {
        self.candidate.ingress_peer
    }

    pub(crate) fn plan_failed(
        &mut self,
        disposition: ConflictDisposition,
    ) -> Result<FailedCommitPlan<'_>, PrePoolError> {
        self.authority
            .plan_failed_commit(&self.candidate, disposition)
    }

    pub(crate) fn plan_ready(
        &mut self,
        unavailable_parents: &HashSet<Byte32>,
        available_dependencies: impl IntoIterator<Item = DependencyKey>,
        history: Vec<ConflictRetention>,
    ) -> Result<ReadyCommitPlan<'_>, PrePoolError> {
        self.authority.plan_ready_commit(
            &self.candidate,
            unavailable_parents,
            available_dependencies,
            history,
        )
    }
}

impl PrePoolKernel {
    fn conflict_retention_entry(
        &self,
        retention: ConflictRetention,
        revision_cursor: &mut EntryRevision,
        arrival_cursor: &mut Arrival,
    ) -> Result<StoredEntry, PrePoolError> {
        let ConflictRetention {
            raw,
            source,
            keys,
            expires_at,
        } = retention;
        let hash = crate::util::compact_packed(&raw.tx.hash());
        if self.entries.contains_key(&hash) {
            return Err(PrePoolError::DuplicateHash(hash));
        }
        let short_id = crate::util::compact_packed(&raw.tx.proposal_short_id());
        if let Some(existing_hash) = self.by_short_id.get(&short_id) {
            return Err(PrePoolError::ShortIdCollision(
                short_id,
                existing_hash.clone(),
            ));
        }
        let keys = keys
            .into_iter()
            .map(DependencyKey::into_compact)
            .collect::<BTreeSet<_>>();
        let revision = EntryRevision::take(revision_cursor)?;
        let arrival = Arrival::take(arrival_cursor)?;
        let mut dependencies = conflict_dependency_keys(&raw.tx, std::iter::empty());
        dependencies.extend(keys.iter().cloned());
        let payload_charge_bytes = raw.charge_bytes();
        let entry = Entry {
            raw: Arc::new(raw),
            source,
            state: EntryState::Wait(WaitState {
                reason: WaitReason::Conflict,
                observed: self.observed_dependencies(keys)?,
            }),
            revision,
            arrival,
            expires_at,
            payload_charge_bytes,
            dependencies: Arc::new(dependencies),
        };
        let entry = StoredEntry::prepare(entry, self.limits)?;
        self.validate_entry_shape(&hash, &entry)?;
        Ok(entry)
    }

    fn extend_optional_history(
        &self,
        desired: &mut MutationSet,
        history: Vec<ConflictRetention>,
        revision_cursor: &mut EntryRevision,
        arrival_cursor: &mut Arrival,
    ) -> Result<(), PrePoolError> {
        for retention in history {
            let entry =
                self.conflict_retention_entry(retention, revision_cursor, arrival_cursor)?;
            desired.try_add_entry(entry)?;
        }
        Ok(())
    }

    fn commit_handoff_desired(
        &self,
        unavailable_parents: &HashSet<Byte32>,
        losers: &BTreeSet<Byte32>,
        winner: &Byte32,
        disposition: ConflictDisposition,
    ) -> Result<CommitHandoffDesired, PrePoolError> {
        // Conflict-history retention keeps a loser executable, so its
        // children remain valid waiters. The capacity fallback terminalizes
        // losers instead; only then are their produced outputs definitive
        // dependency losses. Build that expanded set exclusively on the rare
        // fallback path so the normal Ready hot path adds no clone or scan.
        let terminal_unavailable = (!disposition.retains()).then(|| {
            unavailable_parents
                .iter()
                .cloned()
                .chain(losers.iter().cloned())
                .collect::<HashSet<_>>()
        });
        let unavailable = terminal_unavailable.as_ref().unwrap_or(unavailable_parents);
        let mut revision_cursor = self.next_revision;
        let mut desired = MutationSet::default();
        for (_, entry) in self.unavailable_replacements(unavailable, &mut revision_cursor)? {
            desired.set_entry(entry);
        }
        for hash in losers {
            if !disposition.retains() {
                desired.set_remove(hash.clone());
                continue;
            }
            let Some(entry) = desired
                .take_entry(hash)
                .or_else(|| self.entries.get(hash).cloned())
            else {
                continue;
            };
            let keys = Self::causal_keys(&entry);
            let revision = EntryRevision::take(&mut revision_cursor)?;
            let mut next = entry.into_draft();
            next.revision = revision;
            next.state = EntryState::Wait(WaitState {
                reason: WaitReason::Conflict,
                observed: self.observed_dependencies(keys)?,
            });
            let next = StoredEntry::prepare(next, self.limits)?;
            desired.set_entry(next);
        }
        desired.set_remove(winner.clone());
        Ok((desired, revision_cursor))
    }

    pub(crate) fn begin_next_commit(
        &mut self,
    ) -> Result<Option<ReadyCommitSession<'_>>, PrePoolError> {
        let Some(rank) = self.ready.last().cloned() else {
            return Ok(None);
        };
        let entry = self
            .entries
            .get(&rank.hash)
            .ok_or_else(|| PrePoolError::Missing(rank.hash.clone()))?;
        let EntryState::Ready { payload, .. } = &entry.state else {
            return Err(PrePoolError::ProjectionInconsistent(
                "ready rank points to a non-Ready primary",
            ));
        };
        let current = entry.ready_key(&rank.hash);
        if current.as_ref() != Some(&rank) {
            return Err(PrePoolError::ProjectionInconsistent(
                "ready rank does not match its primary",
            ));
        }
        let candidate = ReadyCommitCandidate {
            rank,
            payload: Arc::clone(payload),
            ingress_peer: entry.raw.ingress_peer(),
        };
        Ok(Some(ReadyCommitSession {
            authority: self,
            candidate,
        }))
    }

    fn validate_commit(
        &self,
        candidate: &ReadyCommitCandidate,
    ) -> Result<&StoredEntry, PrePoolError> {
        let entry = self.validate_location(
            &candidate.rank.hash,
            candidate.rank.revision,
            PrePoolLocation::Ready,
        )?;
        if !matches!(entry.state, EntryState::Ready { .. }) {
            return Err(PrePoolError::ProjectionInconsistent(
                "Ready location contains a non-Ready state",
            ));
        }
        if entry.ready_key(&candidate.rank.hash).as_ref() != Some(&candidate.rank) {
            return Err(PrePoolError::revision_mismatch(
                candidate.rank.hash.clone(),
                candidate.rank.revision,
                entry.revision,
            ));
        }
        Ok(entry)
    }

    fn plan_terminal_commit(
        &mut self,
        candidate: &ReadyCommitCandidate,
    ) -> Result<FailedCommitPlan<'_>, PrePoolError> {
        self.validate_commit(candidate)?;
        let record = self.terminal_record(&candidate.rank.hash).ok_or(
            PrePoolError::ProjectionInconsistent("validated Ready commit lost its primary"),
        )?;
        let parents = HashSet::from([candidate.rank.hash.clone()]);
        let dependency_changes = self.dependency_keys_for_parents(&parents);
        let mut revision_cursor = self.next_revision;
        let mut desired = MutationSet::default();
        for (_, entry) in self.unavailable_replacements(&parents, &mut revision_cursor)? {
            desired.set_entry(entry);
        }
        desired.set_remove(candidate.rank.hash.clone());
        let cohort = self.compile_cohort(desired, revision_cursor, self.next_arrival)?;
        let prepared = self.seal_cohort(cohort, dependency_changes)?;
        Ok(FailedCommitPlan { prepared, record })
    }

    fn plan_failed_commit(
        &mut self,
        candidate: &ReadyCommitCandidate,
        disposition: ConflictDisposition,
    ) -> Result<FailedCommitPlan<'_>, PrePoolError> {
        let entry = self.validate_commit(candidate)?;
        let record = TerminalRecord {
            hash: candidate.rank.hash.clone(),
            raw: Arc::clone(&entry.raw),
            source: entry.source,
        };
        if disposition.retains() {
            let keys = Self::causal_keys(entry);
            let mut next = entry.clone().into_draft();
            let mut next_revision = self.next_revision;
            next.revision = EntryRevision::take(&mut next_revision)?;
            next.state = EntryState::Wait(WaitState {
                reason: WaitReason::Conflict,
                observed: self.observed_dependencies(keys)?,
            });
            let next = StoredEntry::prepare(next, self.limits)?;
            let mut desired = MutationSet::default();
            desired.set_entry(next);
            match self.compile_cohort(desired, next_revision, self.next_arrival) {
                Ok(cohort) => {
                    let prepared = self.seal_cohort(cohort, std::iter::empty())?;
                    return Ok(FailedCommitPlan { prepared, record });
                }
                Err(error) if error.is_capacity_rejection() => {}
                Err(error) => return Err(error),
            }
        }
        self.plan_terminal_commit(candidate)
    }

    fn plan_ready_commit(
        &mut self,
        candidate: &ReadyCommitCandidate,
        unavailable_parents: &HashSet<Byte32>,
        available_dependencies: impl IntoIterator<Item = DependencyKey>,
        history: Vec<ConflictRetention>,
    ) -> Result<ReadyCommitPlan<'_>, PrePoolError> {
        let mut dependency_changes = self.dependency_keys_for_parents(unavailable_parents);
        dependency_changes.extend(available_dependencies);
        let winner = self.validate_commit(candidate)?;
        let (winner_inputs, winner_raw, winner_source) = match &winner.state {
            EntryState::Ready { inputs, .. } => {
                (inputs.clone(), Arc::clone(&winner.raw), winner.source)
            }
            _ => {
                return Err(PrePoolError::ProjectionInconsistent(
                    "validated Ready candidate contains a non-Ready state",
                ));
            }
        };
        let max_losers = self
            .limits
            .max_inputs_per_ready
            .checked_mul(self.limits.max_candidates_per_input)
            .ok_or(PrePoolError::InvalidConfiguration(
                "ready conflict product bound overflows usize",
            ))?;
        let mut losers = BTreeSet::new();
        for input in &winner_inputs {
            if let Some(candidates) = self.ready_by_input.get(input) {
                for rank in candidates
                    .iter()
                    .filter(|rank| rank.hash != candidate.rank.hash)
                {
                    losers.insert(rank.hash.clone());
                    if losers.len() > crate::constants::MAX_POOL_MUTATION_CANDIDATES {
                        return Err(PrePoolError::CommitConflictCohortLimitExceeded);
                    }
                    if losers.len() > max_losers {
                        return Err(PrePoolError::ProjectionInconsistent(
                            "ready conflict union exceeds its indexed product bound",
                        ));
                    }
                }
            }
        }

        let mut superseded = Vec::with_capacity(losers.len());
        for hash in &losers {
            let Some(entry) = self.entries.get(hash) else {
                continue;
            };
            superseded.push(TerminalRecord {
                hash: hash.clone(),
                raw: Arc::clone(&entry.raw),
                source: entry.source,
            });
        }

        let (desired, revision_cursor) = self.commit_handoff_desired(
            unavailable_parents,
            &losers,
            &candidate.rank.hash,
            ConflictDisposition::Retain,
        )?;
        let mut optional = desired;
        let mut optional_revision = revision_cursor;
        let mut optional_arrival = self.next_arrival;
        let (cohort, terminalized_losers) = match self
            .extend_optional_history(
                &mut optional,
                history,
                &mut optional_revision,
                &mut optional_arrival,
            )
            .and_then(|()| self.compile_cohort(optional, optional_revision, optional_arrival))
        {
            Ok(cohort) => (cohort, false),
            Err(error) if error.is_optional_retention_rejection() => {
                let (fallback, fallback_revision) = self.commit_handoff_desired(
                    unavailable_parents,
                    &losers,
                    &candidate.rank.hash,
                    ConflictDisposition::Terminalize,
                )?;
                (
                    self.compile_cohort(fallback, fallback_revision, self.next_arrival)?,
                    true,
                )
            }
            Err(error) => return Err(error),
        };
        if terminalized_losers {
            let parents = losers.iter().cloned().collect::<HashSet<_>>();
            dependency_changes.extend(self.dependency_keys_for_parents(&parents));
        }
        let prepared = self.seal_cohort(cohort, dependency_changes)?;
        let winner_record = TerminalRecord {
            hash: candidate.rank.hash.clone(),
            raw: winner_raw,
            source: winner_source,
        };
        Ok(ReadyCommitPlan {
            prepared,
            settlement: CommitSettlement {
                winner: winner_record,
                superseded,
            },
        })
    }

    pub(crate) fn plan_external_commit(
        &mut self,
        committed: &HashSet<Byte32>,
        unavailable_parents: &HashSet<Byte32>,
        available_dependencies: impl IntoIterator<Item = DependencyKey>,
        history: Vec<ConflictRetention>,
    ) -> Result<ExternalCommitPlan<'_>, PrePoolError> {
        let mut dependency_changes = self.dependency_keys_for_parents(unavailable_parents);
        dependency_changes.extend(available_dependencies);
        let mut hashes = committed.iter().cloned().collect::<Vec<_>>();
        hashes.sort_unstable();
        let records = hashes
            .iter()
            .filter_map(|hash| self.terminal_record(hash))
            .collect();
        let mut revision_cursor = self.next_revision;
        let mut desired = MutationSet::default();
        for (_, entry) in
            self.unavailable_replacements(unavailable_parents, &mut revision_cursor)?
        {
            desired.set_entry(entry);
        }
        for hash in hashes {
            desired.set_remove(hash);
        }
        let fallback = desired.clone();
        let mut optional = desired;
        let mut optional_revision = revision_cursor;
        let mut optional_arrival = self.next_arrival;
        let cohort = match self
            .extend_optional_history(
                &mut optional,
                history,
                &mut optional_revision,
                &mut optional_arrival,
            )
            .and_then(|()| self.compile_cohort(optional, optional_revision, optional_arrival))
        {
            Ok(cohort) => cohort,
            Err(error) if error.is_optional_retention_rejection() => {
                self.compile_cohort(fallback, revision_cursor, self.next_arrival)?
            }
            Err(error) => return Err(error),
        };
        let prepared = self.seal_cohort(cohort, dependency_changes)?;
        Ok(ExternalCommitPlan { prepared, records })
    }
}
