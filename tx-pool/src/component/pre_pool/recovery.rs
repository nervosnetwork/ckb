use super::lifecycle::MutationSet;
use super::*;
use ckb_types::core::TransactionView;

impl PrePoolKernel {
    /// Retain the largest valid parent-first prefix that fits an empty
    /// generation. A prefix is closure-safe because every in-cohort parent
    /// precedes its descendants; once one entry cannot be represented, no
    /// later entry is allowed to cross that missing ownership boundary.
    ///
    /// The scratch kernel has the same exact charge/index rules but owns no
    /// additional transaction backing allocations (`TransactionView` clones
    /// share packed data). Its population and destruction are bounded by the
    /// configured recovery envelope.
    pub(crate) fn retain_recovery_prefix_after_clear(
        &mut self,
        txs: Vec<TransactionView>,
        admitted_epoch: u64,
    ) -> Result<usize, PrePoolError> {
        if !self.entries.is_empty() {
            return Err(PrePoolError::ProjectionInconsistent(
                "recovery-prefix planning requires a fresh generation",
            ));
        }
        let mut probe = Self::new(self.limits);
        probe.next_version = self.next_version;
        probe.next_arrival = self.next_arrival;
        let mut selected = Vec::new();
        for tx in txs {
            match probe.retain_recovery_batch(vec![tx.clone()], admitted_epoch) {
                Ok(1) => selected.push(tx),
                Ok(_) => {}
                // Transaction-shaped identity/shape/capacity failures bound
                // the closure. Structural clock/projection failures still
                // remain internal invariant failures.
                Err(PrePoolError::Public(_)) => break,
                Err(error) => return Err(error),
            }
        }
        self.retain_recovery_batch(selected, admitted_epoch)
    }

    /// Atomically install one parent-first detached-chain recovery cohort.
    ///
    /// Planning touches only the incoming cohort and its exact existing
    /// owners. Every fallible identity, graph, clock and budget predicate is
    /// checked before the first primary/index mutation. Recovery then uses the
    /// same ordered-resolve worker protocol as every other source.
    pub(crate) fn retain_recovery_batch(
        &mut self,
        txs: Vec<TransactionView>,
        admitted_epoch: u64,
    ) -> Result<usize, PrePoolError> {
        if txs.is_empty() {
            return Ok(0);
        }

        let retained = txs.len();
        let mut hashes = HashSet::with_capacity(retained);
        let mut version_cursor = self.next_version;
        let mut arrival_cursor = self.next_arrival;
        let mut planned = MutationSet::default();
        let mut planned_count = 0usize;

        for tx in txs {
            let hash = crate::util::compact_packed(&tx.hash());
            if !hashes.insert(hash.clone()) {
                continue;
            }
            let old_arrival = self.entries.get(&hash).map(|old| old.arrival);
            let version = EntryVersion::take(&mut version_cursor)?;
            let arrival = if let Some(arrival) = old_arrival {
                arrival
            } else {
                Arrival::take(&mut arrival_cursor)?
            };
            let raw = PipelineRawTx::recovery(tx, admitted_epoch);
            let dependencies = conflict_dependency_keys(&raw.tx, std::iter::empty())
                .into_iter()
                .map(DependencyKey::into_compact)
                .collect();
            let payload_charge_bytes = raw.charge_bytes();
            let next = Entry {
                raw: Arc::new(raw),
                source: PrePoolSource::Recovery,
                state: EntryState::ResolveQueued {
                    lane: ResolveLane::Ordered,
                },
                version,
                arrival,
                expires_at: None,
                payload_charge_bytes,
                dependencies,
            };
            let next = StoredEntry::prepare(next, self.limits)?;
            planned.set_entry(next);
            planned_count = planned_count
                .checked_add(1)
                .ok_or(PrePoolError::ResidencyChargeOverflow)?;
        }
        let prepared =
            self.prepare_cohort(planned, version_cursor, arrival_cursor, std::iter::empty())?;
        prepared.apply();
        Ok(planned_count)
    }

    pub(crate) fn recovery_snapshot(&self) -> Vec<TransactionView> {
        let mut items = self
            .entries
            .values()
            .filter(|entry| entry.source == PrePoolSource::Recovery)
            .map(|entry| (entry.arrival, entry.raw.tx.clone()))
            .collect::<Vec<_>>();
        items.sort_unstable_by_key(|(arrival, _)| *arrival);
        items.into_iter().map(|(_, tx)| tx).collect()
    }
}
