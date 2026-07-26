use super::*;
use crate::resolved_tx::ResolvedTx;
use std::collections::BTreeSet;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Debug)]
pub(crate) struct PrePoolView {
    pub(crate) location: PrePoolLocation,
    pub(crate) source: PrePoolSource,
    pub(crate) dependencies: BTreeSet<Byte32>,
    pub(crate) version: EntryVersion,
}

impl PrePoolKernel {
    pub(crate) fn view(&self, hash: &Byte32) -> Option<PrePoolView> {
        self.entries.get(hash).map(|entry| PrePoolView {
            location: entry.state.location(),
            source: entry.source,
            dependencies: entry
                .dependencies
                .iter()
                .map(DependencyKey::parent_hash)
                .collect(),
            version: entry.version,
        })
    }

    pub(crate) fn peer_active_work(&self, peer: ckb_network::PeerIndex) -> usize {
        self.active_by_owner
            .get(&WorkOwner::Remote(peer))
            .copied()
            .unwrap_or_default()
    }

    fn independently_retained_wait_keys(entry: &Entry) -> BTreeSet<DependencyKey> {
        let mut keys = entry
            .raw
            .tx
            .input_pts_iter()
            .chain(
                entry
                    .raw
                    .tx
                    .cell_deps()
                    .into_iter()
                    .map(|dep| dep.out_point()),
            )
            .map(|out_point| DependencyKey::Cell(crate::util::compact_packed(&out_point)))
            .collect::<BTreeSet<_>>();
        keys.extend(
            entry
                .raw
                .tx
                .header_deps()
                .into_iter()
                .map(|hash| DependencyKey::Header(crate::util::compact_packed(&hash))),
        );
        let related = match &entry.state {
            EntryState::VerifyQueued { payload, .. } | EntryState::VerifyLeased { payload, .. } => {
                Some(payload.rtx.as_ref())
            }
            EntryState::Ready { payload, .. } => Some(payload.candidate.rtx.as_ref()),
            _ => None,
        };
        if let Some(resolved) = related {
            keys.extend(
                resolved
                    .related_dep_out_points()
                    .map(|out_point| DependencyKey::Cell(crate::util::compact_packed(out_point))),
            );
        }
        if let EntryState::Wait(wait) = &entry.state {
            keys.extend(wait.observed.keys().cloned());
        }
        keys
    }

    fn independently_expected_charge(&self, entry: &Entry) -> Result<usize, String> {
        let mut memberships = 2usize;
        if entry.source.peer().is_some() {
            memberships = memberships
                .checked_add(2)
                .ok_or_else(|| "peer projection charge overflow".to_string())?;
        }
        let parent_count = entry
            .dependencies
            .iter()
            .map(DependencyKey::parent_hash)
            .collect::<BTreeSet<_>>()
            .len();
        memberships = memberships
            .checked_add(entry.dependencies.len())
            .and_then(|value| value.checked_add(parent_count.checked_mul(2)?))
            .and_then(|value| value.checked_add(usize::from(entry.expires_at.is_some())))
            .ok_or_else(|| "common projection charge overflow".to_string())?;
        let current_state_memberships = match &entry.state {
            EntryState::ResolveLeased | EntryState::VerifyLeased { .. } => 0,
            EntryState::ResolveQueued { .. } => 3,
            EntryState::VerifyQueued { .. } => 4,
            EntryState::Wait(wait) => wait
                .observed
                .len()
                .checked_mul(4)
                .map(|value| value.max(3))
                .ok_or_else(|| "wait projection charge overflow".to_string())?,
            EntryState::Ready { inputs, .. } => inputs
                .len()
                .checked_mul(2)
                .and_then(|value| value.checked_add(1))
                .ok_or_else(|| "ready projection charge overflow".to_string())?,
        };
        let wait_reservation = entry
            .dependencies
            .len()
            .checked_mul(4)
            .map(|value| value.max(3))
            .ok_or_else(|| "dependency wait reservation overflow".to_string())?;
        let state_memberships = current_state_memberships.max(wait_reservation);
        memberships
            .checked_add(state_memberships)
            .and_then(|value| value.checked_mul(self.limits.dependency_overhead))
            .and_then(|value| value.checked_add(self.limits.entry_overhead))
            .and_then(|value| value.checked_add(entry.payload_charge_bytes))
            .ok_or_else(|| "entry charge overflow".to_string())
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn hashes(&self) -> Vec<ckb_types::packed::Byte32> {
        self.entries.keys().cloned().collect()
    }

    pub(crate) fn debug_wait_state(&self) -> String {
        format!(
            "waiters={:?}, availability={:?}, dirty={:?}, dirty_order={:?}",
            self.waiters, self.availability_epoch, self.dirty, self.dirty_order
        )
    }

    pub(crate) fn deadline_len(&self) -> usize {
        self.deadlines.len()
    }

    pub(crate) fn complete_raw(
        &mut self,
        lease: &ResolveLease,
        resolved: ResolvedTx,
        charge_bytes: usize,
        schedule: VerifySchedule,
    ) -> Result<EntryVersion, PrePoolError> {
        self.complete_resolve(lease, resolved, charge_bytes, schedule, BTreeSet::new())
    }

    pub(crate) fn audit(&self) -> Result<(), String> {
        let mut by_short_id = HashMap::new();
        let mut by_peer = HashMap::<_, BTreeSet<_>>::new();
        let mut by_parent = HashMap::<_, BTreeSet<_>>::new();
        let mut waiters = HashMap::<_, BTreeSet<_>>::new();
        let mut queues: [BTreeSet<WorkKey>; 4] = std::array::from_fn(|_| BTreeSet::new());
        let mut ready = BTreeSet::new();
        let mut ready_by_input = HashMap::<_, BTreeSet<_>>::new();
        let mut deadlines = BTreeSet::new();
        let mut total_usage = Residency::default();
        let mut remote_usage = Residency::default();
        let mut conflict_usage = Residency::default();
        let mut peer_usage = HashMap::<_, Residency>::new();
        let mut active_work = 0usize;
        let mut active_by_owner = HashMap::<WorkOwner, usize>::new();
        let mut versions = HashSet::new();
        let mut max_version = 0u128;
        let mut max_arrival = None::<u128>;

        for (hash, entry) in &self.entries {
            self.validate_entry_shape(hash, entry)
                .map_err(|error| format!("entry shape: {error:?}"))?;
            if entry.raw.tx.hash() != *hash {
                return Err("primary hash does not match retained transaction".to_string());
            }
            if entry.short_id != entry.raw.tx.proposal_short_id() {
                return Err("short id does not match retained transaction".to_string());
            }
            if !Self::independently_retained_wait_keys(entry).is_subset(&entry.dependencies) {
                return Err("canonical causal keys omit retained payload dependencies".to_string());
            }
            if entry.charge_bytes != self.independently_expected_charge(entry)? {
                return Err("entry charge is not a closed projection of primary state".to_string());
            }
            if !versions.insert(entry.version) {
                return Err("live entries share a global version".to_string());
            }
            max_version = max_version.max(entry.version);
            max_arrival = Some(max_arrival.map_or(entry.arrival, |value| value.max(entry.arrival)));
            if by_short_id
                .insert(entry.short_id.clone(), hash.clone())
                .is_some()
            {
                return Err("two full hashes occupy one short-id slot".to_string());
            }
            if let Some(peer) = entry.source.peer() {
                by_peer.entry(peer).or_default().insert(hash.clone());
            }
            for parent in entry
                .dependencies
                .iter()
                .map(DependencyKey::parent_hash)
                .collect::<BTreeSet<_>>()
            {
                by_parent.entry(parent).or_default().insert(hash.clone());
            }
            if let Some(key) = entry.work_key(hash, self.limits.verify_fee_rate_ordering) {
                let lane = match entry.state {
                    EntryState::ResolveQueued { lane } => Self::lane_for_resolve(lane),
                    EntryState::VerifyQueued { .. } => WorkLane::Verify,
                    _ => return Err("non-queued entry produced a work key".to_string()),
                };
                queues[lane.index()].insert(key);
            }
            if let EntryState::Wait(wait) = &entry.state {
                let edge = WaitEdge {
                    hash: hash.clone(),
                    version: entry.version,
                };
                for key in wait.observed.keys() {
                    waiters.entry(key.clone()).or_default().insert(edge.clone());
                }
            }
            if let EntryState::Ready { inputs, rank, .. } = &entry.state {
                if rank.hash != *hash || rank.version != entry.version {
                    return Err("ready rank does not identify its primary owner".to_string());
                }
                ready.insert(rank.clone());
                for input in inputs {
                    ready_by_input
                        .entry(input.clone())
                        .or_default()
                        .insert(rank.clone());
                }
            }
            if let Some(deadline) = Self::deadline_key(hash, entry) {
                deadlines.insert(deadline);
            }
            let charge = Residency::new(1, entry.charge_bytes);
            total_usage = total_usage
                .checked_add(charge)
                .ok_or_else(|| "total audit charge overflow".to_string())?;
            if let Some(peer) = entry.source.peer() {
                remote_usage = remote_usage
                    .checked_add(charge)
                    .ok_or_else(|| "remote audit charge overflow".to_string())?;
                let usage = peer_usage.get(&peer).copied().unwrap_or_default();
                peer_usage.insert(
                    peer,
                    usage
                        .checked_add(charge)
                        .ok_or_else(|| "peer audit charge overflow".to_string())?,
                );
            }
            if Self::is_conflict(entry) {
                conflict_usage = conflict_usage
                    .checked_add(charge)
                    .ok_or_else(|| "conflict audit charge overflow".to_string())?;
            }
            if matches!(
                entry.state,
                EntryState::ResolveLeased | EntryState::VerifyLeased { .. }
            ) {
                active_work = active_work
                    .checked_add(1)
                    .ok_or_else(|| "active audit count overflow".to_string())?;
                *active_by_owner.entry(entry.source.into()).or_default() += 1;
            }
        }

        if by_short_id != self.by_short_id
            || by_peer != self.by_peer
            || by_parent != self.by_parent
            || waiters != self.waiters
            || ready != self.ready
            || ready_by_input != self.ready_by_input
            || deadlines != self.deadlines
            || total_usage != self.total_usage
            || remote_usage != self.remote_usage
            || conflict_usage != self.conflict_usage
            || peer_usage != self.peer_usage
            || active_work != self.active_work
            || active_by_owner != self.active_by_owner
        {
            return Err("pre-pool derived projection or accounting drift".to_string());
        }
        for (index, queue) in self.queues.iter().enumerate() {
            queue.audit()?;
            if queue.work_keys() != queues[index] {
                return Err("pre-pool work queue projection drift".to_string());
            }
        }
        let dirty_order = self.dirty_order.iter().cloned().collect::<VecDeque<_>>();
        let unique_dirty = dirty_order.iter().cloned().collect::<HashSet<_>>();
        if unique_dirty.len() != dirty_order.len()
            || unique_dirty != self.dirty.keys().cloned().collect::<HashSet<_>>()
        {
            return Err("dirty dependency order is not a one-to-one projection".to_string());
        }
        for (key, dirty) in &self.dirty {
            let current = self
                .availability_epoch
                .get(key)
                .copied()
                .unwrap_or_default();
            if dirty.target_epoch > current
                || dirty.pending_epoch.is_some_and(|pending| pending > current)
            {
                return Err("dirty dependency targets a future epoch".to_string());
            }
        }
        if self.availability_epoch.keys().any(|key| {
            !self.dirty.contains_key(key)
                && self.waiters.get(key).is_none_or(|edges| edges.is_empty())
        }) {
            return Err("availability epoch outlives every waiter and dirty cursor".to_string());
        }
        if !self.entries.is_empty() && self.next_version <= max_version {
            return Err("next version can alias a live entry".to_string());
        }
        if max_arrival.is_some_and(|arrival| self.next_arrival <= arrival) {
            return Err("next arrival can alias a live entry".to_string());
        }
        Ok(())
    }

    pub(crate) fn total_usage(&self) -> Residency {
        self.total_usage
    }

    pub(crate) fn conflict_usage(&self) -> Residency {
        self.conflict_usage
    }

    pub(crate) fn remote_usage(&self) -> Residency {
        self.remote_usage
    }

    pub(crate) fn peer_usage(&self, peer: ckb_network::PeerIndex) -> Residency {
        self.peer_usage.get(&peer).copied().unwrap_or_default()
    }

    pub(crate) fn dependency_epoch_len(&self) -> usize {
        self.availability_epoch.len()
    }

    pub(crate) fn active_work(&self) -> usize {
        self.active_work
    }
}
