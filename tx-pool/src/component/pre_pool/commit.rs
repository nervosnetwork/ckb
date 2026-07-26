use super::*;

impl PrePoolKernel {
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
        let mut losers = BTreeSet::new();
        for input in &winner_inputs {
            if let Some(candidates) = self.ready_by_input.get(input) {
                for rank in candidates.iter().filter(|rank| rank.hash != ticket.hash) {
                    losers.insert(rank.hash.clone());
                    if losers.len() > self.limits.max_candidates_per_input {
                        return Err(PrePoolError::Repair(
                            "ready conflict union exceeds configured bound",
                        ));
                    }
                }
            }
        }

        self.parents_unavailable(unavailable_parents)?;
        let mut retained_conflicts = Vec::with_capacity(losers.len());
        for hash in losers {
            let Some(entry) = self.entries.get(&hash).cloned() else {
                continue;
            };
            let keys = Self::causal_keys(&entry);
            retained_conflicts.push(TerminalRecord {
                hash: hash.clone(),
                raw: Arc::clone(&entry.raw),
                source: entry.source,
            });
            match self.move_to_wait(&hash, WaitReason::Conflict, keys, None) {
                Ok(_) => {}
                Err(error) if error.is_capacity_rejection() => {
                    self.remove_entry(&hash)?;
                }
                Err(error) => return Err(error),
            }
        }

        let winner_record = self.remove_entry(&ticket.hash)?;
        Ok(CommitSettlement {
            winner: winner_record,
            retained_conflicts,
        })
    }

    pub(crate) fn external_commit_with_unavailable_parents(
        &mut self,
        hash: &Byte32,
        unavailable_parents: &HashSet<Byte32>,
    ) -> Result<Option<TerminalRecord>, PrePoolError> {
        self.parents_unavailable(unavailable_parents)?;
        if !self.entries.contains_key(hash) {
            return Ok(None);
        }
        self.remove_entry(hash).map(Some)
    }

    pub(crate) fn external_commits_with_unavailable_parents(
        &mut self,
        committed: &HashSet<Byte32>,
        unavailable_parents: &HashSet<Byte32>,
    ) -> Result<Vec<TerminalRecord>, PrePoolError> {
        self.parents_unavailable(unavailable_parents)?;
        let mut hashes = committed.iter().cloned().collect::<Vec<_>>();
        hashes.sort_unstable();
        let mut records = Vec::new();
        for hash in hashes {
            if self.entries.contains_key(&hash) {
                records.push(self.remove_entry(&hash)?);
            }
        }
        Ok(records)
    }
}
