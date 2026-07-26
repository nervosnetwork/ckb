use super::*;

type CommitHandoffDesired = (Vec<(Byte32, Option<Entry>)>, EntryVersion);

impl PrePoolKernel {
    fn commit_handoff_desired(
        &self,
        unavailable_parents: &HashSet<Byte32>,
        losers: &BTreeSet<Byte32>,
        winner: &Byte32,
        retain_conflicts: bool,
    ) -> Result<CommitHandoffDesired, PrePoolError> {
        let mut version_cursor = self.next_version;
        let mut desired = self
            .unavailable_replacements(unavailable_parents, &mut version_cursor)?
            .into_iter()
            .map(|(hash, entry)| (hash, Some(entry)))
            .collect::<BTreeMap<_, _>>();
        for hash in losers {
            if !retain_conflicts {
                desired.insert(hash.clone(), None);
                continue;
            }
            let Some(entry) = desired
                .remove(hash)
                .flatten()
                .or_else(|| self.entries.get(hash).cloned())
            else {
                continue;
            };
            let keys = Self::causal_keys(&entry);
            let version = version_cursor;
            version_cursor = version_cursor
                .checked_add(1)
                .ok_or(PrePoolError::VersionExhausted)?;
            let mut next = entry;
            next.version = version;
            next.state = EntryState::Wait(WaitState {
                reason: WaitReason::Conflict,
                observed: self.observed_dependencies(keys),
            });
            next.charge_bytes = self.entry_charge(&next)?;
            desired.insert(hash.clone(), Some(next));
        }
        desired.insert(winner.clone(), None);
        Ok((desired.into_iter().collect(), version_cursor))
    }

    pub(crate) fn begin_next_commit(&self) -> Result<Option<CommitTicket>, PrePoolError> {
        let Some(rank) = self.ready.last().cloned() else {
            return Ok(None);
        };
        let entry = self
            .entries
            .get(&rank.hash)
            .ok_or_else(|| PrePoolError::Missing(rank.hash.clone()))?;
        if entry.version != rank.version {
            return Err(PrePoolError::Repair("ready rank version drift"));
        }
        let EntryState::Ready {
            payload,
            rank: current,
            ..
        } = &entry.state
        else {
            return Err(PrePoolError::Repair("ready rank points to non-ready entry"));
        };
        if current != &rank {
            return Err(PrePoolError::Repair("ready primary rank drift"));
        }
        Ok(Some(CommitTicket {
            hash: rank.hash.clone(),
            version: rank.version,
            rank,
            payload: Arc::clone(payload),
        }))
    }

    fn validate_commit(&self, ticket: &CommitTicket) -> Result<&Entry, PrePoolError> {
        let entry = self.validate_location(&ticket.hash, ticket.version, PrePoolLocation::Ready)?;
        let EntryState::Ready { rank, .. } = &entry.state else {
            unreachable!();
        };
        if rank != &ticket.rank || self.ready.last() != Some(rank) {
            return Err(PrePoolError::Stale {
                hash: ticket.hash.clone(),
                expected: ticket.version,
                actual: entry.version,
            });
        }
        Ok(entry)
    }

    pub(crate) fn fail_commit(
        &mut self,
        ticket: &CommitTicket,
    ) -> Result<TerminalRecord, PrePoolError> {
        self.validate_commit(ticket)?;
        self.remove_entry(&ticket.hash)
    }

    pub(crate) fn park_failed_commit(
        &mut self,
        ticket: &CommitTicket,
    ) -> Result<TerminalRecord, PrePoolError> {
        self.validate_commit(ticket)?;
        self.park_conflict_or_terminalize(&ticket.hash, ticket.version, PrePoolLocation::Ready)
    }

    pub(crate) fn commit_any_handoff_with_unavailable_parents(
        &mut self,
        ticket: &CommitTicket,
        unavailable_parents: &HashSet<Byte32>,
    ) -> Result<CommitSettlement, PrePoolError> {
        let winner = self.validate_commit(ticket)?.clone();
        let winner_inputs = match &winner.state {
            EntryState::Ready { inputs, .. } => inputs.clone(),
            _ => unreachable!(),
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
                        return Err(PrePoolError::Repair(
                            "ready conflict union exceeds configured bound",
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
        let plan = match self.plan_cohort(desired, version_cursor, self.next_arrival) {
            Ok(plan) => plan,
            Err(PrePoolError::ConflictHistoryBudgetExceeded) => {
                let (fallback, fallback_version) =
                    self.commit_handoff_desired(unavailable_parents, &losers, &ticket.hash, false)?;
                self.plan_cohort(fallback, fallback_version, self.next_arrival)?
            }
            Err(error) => return Err(error),
        };
        self.apply_cohort(plan);
        let winner_record = TerminalRecord {
            hash: ticket.hash.clone(),
            raw: Arc::clone(&winner.raw),
            source: winner.source,
        };
        Ok(CommitSettlement {
            winner: winner_record,
            superseded,
        })
    }

    pub(crate) fn external_commit_with_unavailable_parents(
        &mut self,
        hash: &Byte32,
        unavailable_parents: &HashSet<Byte32>,
    ) -> Result<Option<TerminalRecord>, PrePoolError> {
        let records = self.external_commits_with_unavailable_parents(
            &HashSet::from([hash.clone()]),
            unavailable_parents,
        )?;
        Ok(records.into_iter().next())
    }

    pub(crate) fn external_commits_with_unavailable_parents(
        &mut self,
        committed: &HashSet<Byte32>,
        unavailable_parents: &HashSet<Byte32>,
    ) -> Result<Vec<TerminalRecord>, PrePoolError> {
        let mut hashes = committed.iter().cloned().collect::<Vec<_>>();
        hashes.sort_unstable();
        let records = hashes
            .iter()
            .filter_map(|hash| self.terminal_record(hash))
            .collect();
        let mut version_cursor = self.next_version;
        let mut desired = self
            .unavailable_replacements(unavailable_parents, &mut version_cursor)?
            .into_iter()
            .map(|(hash, entry)| (hash, Some(entry)))
            .collect::<BTreeMap<_, _>>();
        for hash in hashes {
            desired.insert(hash, None);
        }
        let plan = self.plan_cohort(
            desired.into_iter().collect(),
            version_cursor,
            self.next_arrival,
        )?;
        self.apply_cohort(plan);
        Ok(records)
    }
}
