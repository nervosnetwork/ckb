use super::lifecycle::{MutationSet, PreparedKernelMutation};
use super::*;

type CommitHandoffDesired = (MutationSet, EntryVersion);

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

impl PrePoolKernel {
    fn conflict_retention_entry(
        &self,
        retention: ConflictRetention,
        version_cursor: &mut EntryVersion,
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
        let version = EntryVersion::take(version_cursor)?;
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
            version,
            arrival,
            expires_at,
            payload_charge_bytes,
            dependencies,
        };
        let entry = StoredEntry::prepare(entry, self.limits)?;
        self.validate_entry_shape(&hash, &entry)?;
        Ok(entry)
    }

    fn extend_optional_history(
        &self,
        desired: &mut MutationSet,
        history: Vec<ConflictRetention>,
        version_cursor: &mut EntryVersion,
        arrival_cursor: &mut Arrival,
    ) -> Result<(), PrePoolError> {
        for retention in history {
            let entry = self.conflict_retention_entry(retention, version_cursor, arrival_cursor)?;
            desired.try_add_entry(entry)?;
        }
        Ok(())
    }

    fn commit_handoff_desired(
        &self,
        unavailable_parents: &HashSet<Byte32>,
        losers: &BTreeSet<Byte32>,
        winner: &Byte32,
        retain_conflicts: bool,
    ) -> Result<CommitHandoffDesired, PrePoolError> {
        // Conflict-history retention keeps a loser executable, so its
        // children remain valid waiters. The capacity fallback terminalizes
        // losers instead; only then are their produced outputs definitive
        // dependency losses. Build that expanded set exclusively on the rare
        // fallback path so the normal Ready hot path adds no clone or scan.
        let terminal_unavailable = (!retain_conflicts).then(|| {
            unavailable_parents
                .iter()
                .cloned()
                .chain(losers.iter().cloned())
                .collect::<HashSet<_>>()
        });
        let unavailable = terminal_unavailable.as_ref().unwrap_or(unavailable_parents);
        let mut version_cursor = self.next_version;
        let mut desired = MutationSet::default();
        for (_, entry) in self.unavailable_replacements(unavailable, &mut version_cursor)? {
            desired.set_entry(entry);
        }
        for hash in losers {
            if !retain_conflicts {
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
            let version = EntryVersion::take(&mut version_cursor)?;
            let mut next = entry.into_draft();
            next.version = version;
            next.state = EntryState::Wait(WaitState {
                reason: WaitReason::Conflict,
                observed: self.observed_dependencies(keys)?,
            });
            let next = StoredEntry::prepare(next, self.limits)?;
            desired.set_entry(next);
        }
        desired.set_remove(winner.clone());
        Ok((desired, version_cursor))
    }

    pub(crate) fn begin_next_commit(&self) -> Result<Option<CommitTicket>, PrePoolError> {
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
        Ok(Some(CommitTicket {
            hash: rank.hash.clone(),
            version: rank.version,
            rank,
            payload: Arc::clone(payload),
        }))
    }

    fn validate_commit(&self, ticket: &CommitTicket) -> Result<&StoredEntry, PrePoolError> {
        let entry = self.validate_location(&ticket.hash, ticket.version, PrePoolLocation::Ready)?;
        if !matches!(entry.state, EntryState::Ready { .. }) {
            return Err(PrePoolError::ProjectionInconsistent(
                "Ready location contains a non-Ready state",
            ));
        }
        // The commit driver is serialized, but verification can publish a
        // higher-ranked Ready owner while the selected ticket waits for the
        // TxPool write boundary. That does not invalidate this exact owner:
        // the later candidate remains Ready for the next driver iteration.
        if entry.ready_key(&ticket.hash).as_ref() != Some(&ticket.rank) {
            return Err(PrePoolError::stale(
                ticket.hash.clone(),
                ticket.version,
                entry.version,
            ));
        }
        Ok(entry)
    }

    fn plan_terminal_commit(
        &mut self,
        ticket: &CommitTicket,
    ) -> Result<FailedCommitPlan<'_>, PrePoolError> {
        self.validate_commit(ticket)?;
        let record =
            self.terminal_record(&ticket.hash)
                .ok_or(PrePoolError::ProjectionInconsistent(
                    "validated Ready commit lost its primary",
                ))?;
        let parents = HashSet::from([ticket.hash.clone()]);
        let dependency_changes = self.dependency_keys_for_parents(&parents);
        let mut version_cursor = self.next_version;
        let mut desired = MutationSet::default();
        for (_, entry) in self.unavailable_replacements(&parents, &mut version_cursor)? {
            desired.set_entry(entry);
        }
        desired.set_remove(ticket.hash.clone());
        let cohort = self.compile_cohort(desired, version_cursor, self.next_arrival)?;
        let prepared = self.seal_cohort(cohort, dependency_changes)?;
        Ok(FailedCommitPlan { prepared, record })
    }

    pub(crate) fn plan_failed_commit(
        &mut self,
        ticket: &CommitTicket,
        retain_conflict: bool,
    ) -> Result<FailedCommitPlan<'_>, PrePoolError> {
        let entry = self.validate_commit(ticket)?;
        let record = TerminalRecord {
            hash: ticket.hash.clone(),
            raw: Arc::clone(&entry.raw),
            source: entry.source,
        };
        if retain_conflict {
            let keys = Self::causal_keys(entry);
            let mut next = entry.clone().into_draft();
            let mut next_version = self.next_version;
            next.version = EntryVersion::take(&mut next_version)?;
            next.state = EntryState::Wait(WaitState {
                reason: WaitReason::Conflict,
                observed: self.observed_dependencies(keys)?,
            });
            let next = StoredEntry::prepare(next, self.limits)?;
            let mut desired = MutationSet::default();
            desired.set_entry(next);
            match self.compile_cohort(desired, next_version, self.next_arrival) {
                Ok(cohort) => {
                    let prepared = self.seal_cohort(cohort, std::iter::empty())?;
                    return Ok(FailedCommitPlan { prepared, record });
                }
                Err(error) if error.is_capacity_rejection() => {}
                Err(error) => return Err(error),
            }
        }
        self.plan_terminal_commit(ticket)
    }

    pub(crate) fn plan_ready_commit(
        &mut self,
        ticket: &CommitTicket,
        unavailable_parents: &HashSet<Byte32>,
        available_dependencies: impl IntoIterator<Item = DependencyKey>,
        history: Vec<ConflictRetention>,
    ) -> Result<ReadyCommitPlan<'_>, PrePoolError> {
        let mut dependency_changes = self.dependency_keys_for_parents(unavailable_parents);
        dependency_changes.extend(available_dependencies);
        let winner = self.validate_commit(ticket)?;
        let (winner_inputs, winner_raw, winner_source) = match &winner.state {
            EntryState::Ready { inputs, .. } => {
                (inputs.clone(), Arc::clone(&winner.raw), winner.source)
            }
            _ => {
                return Err(PrePoolError::ProjectionInconsistent(
                    "validated Ready ticket contains a non-Ready state",
                ));
            }
        };
        let max_losers = self
            .limits
            .max_inputs_per_ready
            .checked_mul(self.limits.max_candidates_per_input)
            .ok_or(PrePoolError::ResidencyChargeOverflow)?;
        let mut losers = BTreeSet::new();
        for input in &winner_inputs {
            if let Some(candidates) = self.ready_by_input.get(input) {
                for rank in candidates.iter().filter(|rank| rank.hash != ticket.hash) {
                    losers.insert(rank.hash.clone());
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

        let (desired, version_cursor) =
            self.commit_handoff_desired(unavailable_parents, &losers, &ticket.hash, true)?;
        let mut optional = desired;
        let mut optional_version = version_cursor;
        let mut optional_arrival = self.next_arrival;
        let (cohort, terminalized_losers) = match self
            .extend_optional_history(
                &mut optional,
                history,
                &mut optional_version,
                &mut optional_arrival,
            )
            .and_then(|()| self.compile_cohort(optional, optional_version, optional_arrival))
        {
            Ok(cohort) => (cohort, false),
            Err(error) if error.is_optional_retention_rejection() => {
                let (fallback, fallback_version) =
                    self.commit_handoff_desired(unavailable_parents, &losers, &ticket.hash, false)?;
                (
                    self.compile_cohort(fallback, fallback_version, self.next_arrival)?,
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
            hash: ticket.hash.clone(),
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
        let mut version_cursor = self.next_version;
        let mut desired = MutationSet::default();
        for (_, entry) in self.unavailable_replacements(unavailable_parents, &mut version_cursor)? {
            desired.set_entry(entry);
        }
        for hash in hashes {
            desired.set_remove(hash);
        }
        let fallback = desired.clone();
        let mut optional = desired;
        let mut optional_version = version_cursor;
        let mut optional_arrival = self.next_arrival;
        let cohort = match self
            .extend_optional_history(
                &mut optional,
                history,
                &mut optional_version,
                &mut optional_arrival,
            )
            .and_then(|()| self.compile_cohort(optional, optional_version, optional_arrival))
        {
            Ok(cohort) => cohort,
            Err(error) if error.is_optional_retention_rejection() => {
                self.compile_cohort(fallback, version_cursor, self.next_arrival)?
            }
            Err(error) => return Err(error),
        };
        let prepared = self.seal_cohort(cohort, dependency_changes)?;
        Ok(ExternalCommitPlan { prepared, records })
    }
}
