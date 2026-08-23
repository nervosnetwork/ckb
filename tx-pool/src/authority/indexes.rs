//! Payload-free projections derived from the primary owner map.
//!
//! Every index transition is compiled from the same owner before/after set.
//! Callers cannot update proposal and ingress-peer views independently.

use super::state::{AcceptedAtMillis, OwnedTx, ProposalId, RawTxHash, RemoteDeadline};
use ckb_network::PeerIndex;
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    ops::Bound::{Excluded, Unbounded},
};

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
struct DeadlineKey {
    // Remote residency expiry is immutable for one owner and is validated
    // against the current primary entry when consumed. Compute phase/version
    // churn therefore must not detach and reinsert the same deadline.
    expires_at: RemoteDeadline,
    hash: RawTxHash,
}

/// Accepted expiry is tied to the immutable admission timestamp, not the OCC
/// version. Status-only version changes therefore do not churn this index.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
struct AcceptedDeadlineKey {
    accepted_at: AcceptedAtMillis,
    hash: RawTxHash,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct DueRemote {
    pub(super) expires_at: RemoteDeadline,
    pub(super) hash: RawTxHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DueAccepted {
    pub(super) accepted_at: AcceptedAtMillis,
    pub(super) hash: RawTxHash,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum IndexError {
    ProposalCollision,
    Projection,
    Arithmetic,
    Allocation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StableIndexError {
    Projection,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct IndexFact {
    proposal: ProposalId,
    preaccepted_peer: Option<PeerIndex>,
    context_sensitive_accepted: bool,
    active_deadline: Option<RemoteDeadline>,
    accepted_at: Option<AcceptedAtMillis>,
}

impl IndexFact {
    fn from_owner(key: &RawTxHash, owner: &OwnedTx) -> Result<Self, IndexError> {
        if &owner.record().identity.raw != key {
            return Err(IndexError::Projection);
        }
        let preaccepted_peer = match owner {
            OwnedTx::PreAccepted(entry) => entry.source.ingress_peer(),
            OwnedTx::Accepted(_) | OwnedTx::ReplacementHistory(_) => None,
        };
        let context_sensitive_accepted = match owner {
            OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_) => false,
            OwnedTx::Accepted(entry) => entry.proof.sensitivity().requires_reorg_revalidation(),
        };
        let active_deadline = match owner {
            OwnedTx::PreAccepted(entry) => entry.source.active_remote_deadline(),
            OwnedTx::Accepted(_) | OwnedTx::ReplacementHistory(_) => None,
        };
        let accepted_at = match owner {
            OwnedTx::Accepted(entry) => Some(entry.accepted_at),
            OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_) => None,
        };
        Ok(Self {
            proposal: owner.record().identity.proposal.clone(),
            preaccepted_peer,
            context_sensitive_accepted,
            active_deadline,
            accepted_at,
        })
    }

    fn deadline_key(&self, hash: &RawTxHash) -> Option<DeadlineKey> {
        self.active_deadline.map(|expires_at| DeadlineKey {
            expires_at,
            hash: hash.clone(),
        })
    }

    fn accepted_deadline_key(&self, hash: &RawTxHash) -> Option<AcceptedDeadlineKey> {
        self.accepted_at.map(|accepted_at| AcceptedDeadlineKey {
            accepted_at,
            hash: hash.clone(),
        })
    }
}

struct IndexChange {
    key: RawTxHash,
    before: Option<IndexFact>,
    after: Option<IndexFact>,
}

#[derive(Default)]
pub(super) struct IndexDelta {
    proposal_removals: Vec<(ProposalId, RawTxHash)>,
    proposal_insertions: Vec<(ProposalId, RawTxHash)>,
    peer_removals: Vec<(PeerIndex, RawTxHash)>,
    peer_insertions: Vec<(PeerIndex, RawTxHash)>,
    new_peer_rows: Vec<(PeerIndex, HashSet<RawTxHash>)>,
    touched_peers: Vec<PeerIndex>,
    context_removals: Vec<RawTxHash>,
    context_insertions: Vec<RawTxHash>,
    deadline_removals: Vec<DeadlineKey>,
    deadline_insertions: Vec<DeadlineKey>,
    accepted_deadline_removals: Vec<AcceptedDeadlineKey>,
    accepted_deadline_insertions: Vec<AcceptedDeadlineKey>,
}

#[derive(Debug, Default)]
pub(super) struct AuthorityIndexes {
    by_proposal: HashMap<ProposalId, RawTxHash>,
    preaccepted_by_peer: HashMap<PeerIndex, HashSet<RawTxHash>>,
    context_sensitive_accepted: HashSet<RawTxHash>,
    deadlines: BTreeSet<DeadlineKey>,
    accepted_deadlines: BTreeSet<AcceptedDeadlineKey>,
}

impl AuthorityIndexes {
    fn validate_present_fact(&self, key: &RawTxHash, fact: &IndexFact) -> Result<(), IndexError> {
        if self.by_proposal.get(&fact.proposal) != Some(key) {
            return Err(IndexError::Projection);
        }
        if let Some(peer) = fact.preaccepted_peer
            && !self
                .preaccepted_by_peer
                .get(&peer)
                .is_some_and(|owners| owners.contains(key))
        {
            return Err(IndexError::Projection);
        }
        if self.context_sensitive_accepted.contains(key) != fact.context_sensitive_accepted {
            return Err(IndexError::Projection);
        }
        if fact
            .deadline_key(key)
            .is_some_and(|deadline| !self.deadlines.contains(&deadline))
        {
            return Err(IndexError::Projection);
        }
        if fact
            .accepted_deadline_key(key)
            .is_some_and(|deadline| !self.accepted_deadlines.contains(&deadline))
        {
            return Err(IndexError::Projection);
        }
        Ok(())
    }

    pub(super) fn proposal_owner(&self, proposal: &ProposalId) -> Option<&RawTxHash> {
        self.by_proposal.get(proposal)
    }

    pub(super) fn preaccepted_for_peer(&self, peer: PeerIndex) -> Option<&HashSet<RawTxHash>> {
        self.preaccepted_by_peer.get(&peer)
    }

    pub(super) fn context_sensitive_accepted(&self) -> &HashSet<RawTxHash> {
        &self.context_sensitive_accepted
    }

    pub(super) fn due_remote(
        &self,
        now: RemoteDeadline,
        limit: usize,
    ) -> Result<Vec<DueRemote>, IndexError> {
        let mut due = Vec::new();
        due.try_reserve(limit.min(self.deadlines.len()))
            .map_err(|_| IndexError::Allocation)?;
        for deadline in self
            .deadlines
            .iter()
            .take_while(|deadline| deadline.expires_at <= now)
            .take(limit)
        {
            due.push(DueRemote {
                expires_at: deadline.expires_at,
                hash: deadline.hash.clone(),
            });
        }
        Ok(due)
    }

    /// Scan one bounded page of all retained Remote owners in immutable
    /// deadline/hash order. A caller pairs the opaque cursor with an
    /// authority read cut; if that cut changes, it restarts from the first
    /// page rather than treating this derived index as an authority.
    pub(super) fn remote_page_into(
        &self,
        after: Option<&DueRemote>,
        limit: usize,
        page: &mut Vec<DueRemote>,
    ) -> Result<bool, IndexError> {
        page.clear();
        if page.capacity() < limit.min(self.deadlines.len()) {
            return Err(IndexError::Allocation);
        }
        let after = after.map(|cursor| DeadlineKey {
            expires_at: cursor.expires_at,
            hash: cursor.hash.clone(),
        });
        let start = after.as_ref().map_or(Unbounded, Excluded);
        let mut entries = self.deadlines.range((start, Unbounded));
        for deadline in entries.by_ref().take(limit) {
            page.push(DueRemote {
                expires_at: deadline.expires_at,
                hash: deadline.hash.clone(),
            });
        }
        Ok(entries.next().is_some())
    }

    pub(super) fn due_accepted(
        &self,
        cutoff: AcceptedAtMillis,
        limit: usize,
    ) -> Result<Vec<DueAccepted>, IndexError> {
        let mut due = Vec::new();
        due.try_reserve(limit.min(self.accepted_deadlines.len()))
            .map_err(|_| IndexError::Allocation)?;
        for deadline in self
            .accepted_deadlines
            .iter()
            .take_while(|deadline| deadline.accepted_at <= cutoff)
            .take(limit)
        {
            due.push(DueAccepted {
                accepted_at: deadline.accepted_at,
                hash: deadline.hash.clone(),
            });
        }
        Ok(due)
    }

    /// Compile the common one-owner transition without allocating when its
    /// index facts are unchanged. Compute phase and accepted-status changes
    /// therefore pay only projection validation, not batch-planner storage.
    pub(super) fn plan_replace(
        &mut self,
        key: &RawTxHash,
        before: Option<&OwnedTx>,
        after: Option<&OwnedTx>,
    ) -> Result<IndexDelta, IndexError> {
        let before = before
            .map(|owner| IndexFact::from_owner(key, owner))
            .transpose()?;
        let after = after
            .map(|owner| IndexFact::from_owner(key, owner))
            .transpose()?;
        if let Some(before) = &before {
            self.validate_present_fact(key, before)?;
        }

        let mut delta = IndexDelta::default();
        if before.as_ref().map(|fact| &fact.proposal) != after.as_ref().map(|fact| &fact.proposal) {
            if let Some(before) = &before {
                delta
                    .proposal_removals
                    .try_reserve(1)
                    .map_err(|_| IndexError::Allocation)?;
                delta
                    .proposal_removals
                    .push((before.proposal.clone(), key.clone()));
            }
            if let Some(after) = &after {
                if self.by_proposal.contains_key(&after.proposal) {
                    return Err(IndexError::ProposalCollision);
                }
                self.by_proposal
                    .try_reserve(1)
                    .map_err(|_| IndexError::Allocation)?;
                delta
                    .proposal_insertions
                    .try_reserve(1)
                    .map_err(|_| IndexError::Allocation)?;
                delta
                    .proposal_insertions
                    .push((after.proposal.clone(), key.clone()));
            }
        }

        let before_peer = before.as_ref().and_then(|fact| fact.preaccepted_peer);
        let after_peer = after.as_ref().and_then(|fact| fact.preaccepted_peer);
        if before_peer != after_peer {
            if let Some(peer) = before_peer {
                delta
                    .peer_removals
                    .try_reserve(1)
                    .map_err(|_| IndexError::Allocation)?;
                delta
                    .touched_peers
                    .try_reserve(1)
                    .map_err(|_| IndexError::Allocation)?;
                delta.peer_removals.push((peer, key.clone()));
                delta.touched_peers.push(peer);
            }
            if let Some(peer) = after_peer {
                if self
                    .preaccepted_by_peer
                    .get(&peer)
                    .is_some_and(|owners| owners.contains(key))
                {
                    return Err(IndexError::Projection);
                }
                if let Some(row) = self.preaccepted_by_peer.get_mut(&peer) {
                    row.try_reserve(1).map_err(|_| IndexError::Allocation)?;
                    delta
                        .peer_insertions
                        .try_reserve(1)
                        .map_err(|_| IndexError::Allocation)?;
                    delta
                        .touched_peers
                        .try_reserve(1)
                        .map_err(|_| IndexError::Allocation)?;
                    delta.peer_insertions.push((peer, key.clone()));
                    delta.touched_peers.push(peer);
                } else {
                    self.preaccepted_by_peer
                        .try_reserve(1)
                        .map_err(|_| IndexError::Allocation)?;
                    let mut row = HashSet::new();
                    row.try_reserve(1).map_err(|_| IndexError::Allocation)?;
                    row.insert(key.clone());
                    delta
                        .new_peer_rows
                        .try_reserve(1)
                        .map_err(|_| IndexError::Allocation)?;
                    delta.new_peer_rows.push((peer, row));
                }
            }
            delta.touched_peers.sort_unstable();
            delta.touched_peers.dedup();
        }
        let before_context = before
            .as_ref()
            .is_some_and(|fact| fact.context_sensitive_accepted);
        let after_context = after
            .as_ref()
            .is_some_and(|fact| fact.context_sensitive_accepted);
        if before_context != after_context {
            if before_context {
                delta
                    .context_removals
                    .try_reserve(1)
                    .map_err(|_| IndexError::Allocation)?;
                delta.context_removals.push(key.clone());
            }
            if after_context {
                if self.context_sensitive_accepted.contains(key) {
                    return Err(IndexError::Projection);
                }
                self.context_sensitive_accepted
                    .try_reserve(1)
                    .map_err(|_| IndexError::Allocation)?;
                delta
                    .context_insertions
                    .try_reserve(1)
                    .map_err(|_| IndexError::Allocation)?;
                delta.context_insertions.push(key.clone());
            }
        }
        let before_deadline = before.as_ref().and_then(|fact| fact.deadline_key(key));
        let after_deadline = after.as_ref().and_then(|fact| fact.deadline_key(key));
        if before_deadline != after_deadline {
            if let Some(deadline) = before_deadline {
                delta.deadline_removals.push(deadline);
            }
            if let Some(deadline) = after_deadline {
                if self.deadlines.contains(&deadline) {
                    return Err(IndexError::Projection);
                }
                delta.deadline_insertions.push(deadline);
            }
        }
        let before_accepted_deadline = before
            .as_ref()
            .and_then(|fact| fact.accepted_deadline_key(key));
        let after_accepted_deadline = after
            .as_ref()
            .and_then(|fact| fact.accepted_deadline_key(key));
        if before_accepted_deadline != after_accepted_deadline {
            if let Some(deadline) = before_accepted_deadline {
                delta.accepted_deadline_removals.push(deadline);
            }
            if let Some(deadline) = after_accepted_deadline {
                if self.accepted_deadlines.contains(&deadline) {
                    return Err(IndexError::Projection);
                }
                delta.accepted_deadline_insertions.push(deadline);
            }
        }
        Ok(delta)
    }

    /// Validate a phase-only owner replacement whose index facts are stable.
    /// The equality and current-projection proofs are checked directly. The
    /// general compiler is deliberately not called, so this emergency path
    /// has no insertion, removal, reservation, or allocation error surface.
    pub(super) fn plan_stable_replace(
        &mut self,
        key: &RawTxHash,
        before: &OwnedTx,
        after: &OwnedTx,
    ) -> Result<IndexDelta, StableIndexError> {
        let before_fact =
            IndexFact::from_owner(key, before).map_err(|_| StableIndexError::Projection)?;
        let after_fact =
            IndexFact::from_owner(key, after).map_err(|_| StableIndexError::Projection)?;
        if before_fact != after_fact {
            return Err(StableIndexError::Projection);
        }
        self.validate_present_fact(key, &before_fact)
            .map_err(|_| StableIndexError::Projection)?;
        Ok(IndexDelta::default())
    }

    pub(super) fn plan_replacements<'entry>(
        &mut self,
        changes: impl IntoIterator<
            Item = (
                &'entry RawTxHash,
                Option<&'entry OwnedTx>,
                Option<&'entry OwnedTx>,
            ),
        >,
    ) -> Result<IndexDelta, IndexError> {
        let mut input = changes.into_iter();
        let mut changes = Vec::new();
        if let Some(capacity) = input.size_hint().1 {
            changes
                .try_reserve_exact(capacity)
                .map_err(|_| IndexError::Allocation)?;
        }
        for (key, before, after) in input.by_ref() {
            if changes.len() == changes.capacity() {
                changes.try_reserve(1).map_err(|_| IndexError::Allocation)?;
            }
            changes.push(IndexChange {
                key: key.clone(),
                before: before
                    .map(|owner| IndexFact::from_owner(key, owner))
                    .transpose()?,
                after: after
                    .map(|owner| IndexFact::from_owner(key, owner))
                    .transpose()?,
            });
        }
        changes.sort_unstable_by(|left, right| left.key.cmp(&right.key));
        if changes
            .array_windows::<2>()
            .any(|[left, right]| left.key == right.key)
        {
            return Err(IndexError::Projection);
        }

        let mut proposal_removals = Vec::new();
        let mut proposal_insertions = Vec::new();
        let mut peer_removals = Vec::new();
        let mut peer_insertions = Vec::new();
        let mut context_removals = Vec::new();
        let mut context_insertions = Vec::new();
        let mut deadline_removals = Vec::new();
        let mut deadline_insertions = Vec::new();
        let mut accepted_deadline_removals = Vec::new();
        let mut accepted_deadline_insertions = Vec::new();
        proposal_removals
            .try_reserve(changes.len())
            .map_err(|_| IndexError::Allocation)?;
        proposal_insertions
            .try_reserve(changes.len())
            .map_err(|_| IndexError::Allocation)?;
        peer_removals
            .try_reserve(changes.len())
            .map_err(|_| IndexError::Allocation)?;
        peer_insertions
            .try_reserve(changes.len())
            .map_err(|_| IndexError::Allocation)?;
        context_removals
            .try_reserve(changes.len())
            .map_err(|_| IndexError::Allocation)?;
        context_insertions
            .try_reserve(changes.len())
            .map_err(|_| IndexError::Allocation)?;
        deadline_removals
            .try_reserve(changes.len())
            .map_err(|_| IndexError::Allocation)?;
        deadline_insertions
            .try_reserve(changes.len())
            .map_err(|_| IndexError::Allocation)?;
        accepted_deadline_removals
            .try_reserve(changes.len())
            .map_err(|_| IndexError::Allocation)?;
        accepted_deadline_insertions
            .try_reserve(changes.len())
            .map_err(|_| IndexError::Allocation)?;
        for change in &changes {
            if let Some(before) = &change.before {
                if self.by_proposal.get(&before.proposal) != Some(&change.key) {
                    return Err(IndexError::Projection);
                }
                if let Some(peer) = before.preaccepted_peer
                    && !self
                        .preaccepted_by_peer
                        .get(&peer)
                        .is_some_and(|owners| owners.contains(&change.key))
                {
                    return Err(IndexError::Projection);
                }
                if self.context_sensitive_accepted.contains(&change.key)
                    != before.context_sensitive_accepted
                {
                    return Err(IndexError::Projection);
                }
                if before
                    .deadline_key(&change.key)
                    .is_some_and(|deadline| !self.deadlines.contains(&deadline))
                {
                    return Err(IndexError::Projection);
                }
                if before
                    .accepted_deadline_key(&change.key)
                    .is_some_and(|deadline| !self.accepted_deadlines.contains(&deadline))
                {
                    return Err(IndexError::Projection);
                }
            } else if self.context_sensitive_accepted.contains(&change.key) {
                return Err(IndexError::Projection);
            }
            let before_proposal = change.before.as_ref().map(|fact| &fact.proposal);
            let after_proposal = change.after.as_ref().map(|fact| &fact.proposal);
            if before_proposal != after_proposal {
                if let Some(before) = &change.before {
                    proposal_removals.push((before.proposal.clone(), change.key.clone()));
                }
                if let Some(after) = &change.after {
                    proposal_insertions.push((after.proposal.clone(), change.key.clone()));
                }
            }
            let before_peer = change
                .before
                .as_ref()
                .and_then(|fact| fact.preaccepted_peer);
            let after_peer = change.after.as_ref().and_then(|fact| fact.preaccepted_peer);
            if before_peer != after_peer {
                if let Some(peer) = before_peer {
                    peer_removals.push((peer, change.key.clone()));
                }
                if let Some(peer) = after_peer {
                    peer_insertions.push((peer, change.key.clone()));
                }
            }
            let before_context = change
                .before
                .as_ref()
                .is_some_and(|fact| fact.context_sensitive_accepted);
            let after_context = change
                .after
                .as_ref()
                .is_some_and(|fact| fact.context_sensitive_accepted);
            if before_context != after_context {
                if before_context {
                    context_removals.push(change.key.clone());
                }
                if after_context {
                    context_insertions.push(change.key.clone());
                }
            }
            let before_deadline = change
                .before
                .as_ref()
                .and_then(|fact| fact.deadline_key(&change.key));
            let after_deadline = change
                .after
                .as_ref()
                .and_then(|fact| fact.deadline_key(&change.key));
            if before_deadline != after_deadline {
                if let Some(deadline) = before_deadline {
                    deadline_removals.push(deadline);
                }
                if let Some(deadline) = after_deadline {
                    deadline_insertions.push(deadline);
                }
            }
            let before_accepted_deadline = change
                .before
                .as_ref()
                .and_then(|fact| fact.accepted_deadline_key(&change.key));
            let after_accepted_deadline = change
                .after
                .as_ref()
                .and_then(|fact| fact.accepted_deadline_key(&change.key));
            if before_accepted_deadline != after_accepted_deadline {
                if let Some(deadline) = before_accepted_deadline {
                    accepted_deadline_removals.push(deadline);
                }
                if let Some(deadline) = after_accepted_deadline {
                    accepted_deadline_insertions.push(deadline);
                }
            }
        }

        deadline_removals.sort_unstable();
        deadline_insertions.sort_unstable();
        if deadline_removals
            .array_windows::<2>()
            .any(|[left, right]| left == right)
            || deadline_insertions
                .array_windows::<2>()
                .any(|[left, right]| left == right)
            || deadline_insertions.iter().any(|deadline| {
                self.deadlines.contains(deadline)
                    && deadline_removals.binary_search(deadline).is_err()
            })
        {
            return Err(IndexError::Projection);
        }

        accepted_deadline_removals.sort_unstable();
        accepted_deadline_insertions.sort_unstable();
        if accepted_deadline_removals
            .array_windows::<2>()
            .any(|[left, right]| left == right)
            || accepted_deadline_insertions
                .array_windows::<2>()
                .any(|[left, right]| left == right)
            || accepted_deadline_insertions.iter().any(|deadline| {
                self.accepted_deadlines.contains(deadline)
                    && accepted_deadline_removals.binary_search(deadline).is_err()
            })
        {
            return Err(IndexError::Projection);
        }

        proposal_removals.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        proposal_insertions.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        if proposal_removals
            .array_windows::<2>()
            .any(|[left, right]| left.0 == right.0)
        {
            return Err(IndexError::Projection);
        }
        if proposal_insertions
            .array_windows::<2>()
            .any(|[left, right]| left.0 == right.0)
        {
            return Err(IndexError::ProposalCollision);
        }
        for (proposal, _) in &proposal_insertions {
            if let Some(current) = self.by_proposal.get(proposal)
                && proposal_removals
                    .binary_search_by(|(removed, _)| removed.cmp(proposal))
                    .ok()
                    .and_then(|index| proposal_removals.get(index))
                    .map(|(_, hash)| hash)
                    != Some(current)
            {
                return Err(IndexError::ProposalCollision);
            }
        }
        let new_proposals = proposal_insertions
            .iter()
            .filter(|(proposal, _)| !self.by_proposal.contains_key(proposal))
            .count();
        self.by_proposal
            .try_reserve(new_proposals)
            .map_err(|_| IndexError::Allocation)?;

        for key in &context_insertions {
            if self.context_sensitive_accepted.contains(key) {
                return Err(IndexError::Projection);
            }
        }
        self.context_sensitive_accepted
            .try_reserve(context_insertions.len())
            .map_err(|_| IndexError::Allocation)?;

        let mut additions_by_peer = HashMap::<PeerIndex, usize>::new();
        additions_by_peer
            .try_reserve(peer_insertions.len())
            .map_err(|_| IndexError::Allocation)?;
        for (peer, key) in &peer_insertions {
            if self
                .preaccepted_by_peer
                .get(peer)
                .is_some_and(|owners| owners.contains(key))
            {
                return Err(IndexError::Projection);
            }
            let count = additions_by_peer.entry(*peer).or_default();
            *count = count.checked_add(1).ok_or(IndexError::Arithmetic)?;
        }
        let new_peer_count = additions_by_peer
            .keys()
            .filter(|peer| !self.preaccepted_by_peer.contains_key(peer))
            .count();
        self.preaccepted_by_peer
            .try_reserve(new_peer_count)
            .map_err(|_| IndexError::Allocation)?;

        let mut new_rows = HashMap::<PeerIndex, HashSet<RawTxHash>>::new();
        new_rows
            .try_reserve(new_peer_count)
            .map_err(|_| IndexError::Allocation)?;
        for (peer, additions) in additions_by_peer {
            if let Some(row) = self.preaccepted_by_peer.get_mut(&peer) {
                row.try_reserve(additions)
                    .map_err(|_| IndexError::Allocation)?;
            } else {
                let mut row = HashSet::new();
                row.try_reserve(additions)
                    .map_err(|_| IndexError::Allocation)?;
                new_rows.insert(peer, row);
            }
        }

        let mut retained_peer_insertions = Vec::new();
        retained_peer_insertions
            .try_reserve(peer_insertions.len())
            .map_err(|_| IndexError::Allocation)?;
        for (peer, key) in peer_insertions {
            if let Some(row) = new_rows.get_mut(&peer) {
                if !row.insert(key) {
                    return Err(IndexError::Projection);
                }
            } else {
                retained_peer_insertions.push((peer, key));
            }
        }

        let mut touched_peers = Vec::new();
        touched_peers
            .try_reserve(
                peer_removals
                    .len()
                    .checked_add(retained_peer_insertions.len())
                    .ok_or(IndexError::Arithmetic)?,
            )
            .map_err(|_| IndexError::Allocation)?;
        touched_peers.extend(peer_removals.iter().map(|(peer, _)| *peer));
        touched_peers.extend(retained_peer_insertions.iter().map(|(peer, _)| *peer));
        touched_peers.sort_unstable();
        touched_peers.dedup();

        let mut new_peer_rows = Vec::new();
        new_peer_rows
            .try_reserve_exact(new_rows.len())
            .map_err(|_| IndexError::Allocation)?;
        new_peer_rows.extend(new_rows);
        new_peer_rows.sort_unstable_by_key(|(peer, _)| *peer);
        peer_removals.sort_unstable();
        retained_peer_insertions.sort_unstable();
        Ok(IndexDelta {
            proposal_removals,
            proposal_insertions,
            peer_removals,
            peer_insertions: retained_peer_insertions,
            new_peer_rows,
            touched_peers,
            context_removals,
            context_insertions,
            deadline_removals,
            deadline_insertions,
            accepted_deadline_removals,
            accepted_deadline_insertions,
        })
    }

    pub(super) fn apply(&mut self, delta: IndexDelta) {
        for (proposal, _) in delta.proposal_removals {
            self.by_proposal.remove(&proposal);
        }
        for (proposal, key) in delta.proposal_insertions {
            self.by_proposal.insert(proposal, key);
        }
        for (peer, key) in delta.peer_removals {
            if let Some(row) = self.preaccepted_by_peer.get_mut(&peer) {
                row.remove(&key);
            }
        }
        for (peer, row) in delta.new_peer_rows {
            self.preaccepted_by_peer.insert(peer, row);
        }
        for (peer, key) in delta.peer_insertions {
            if let Some(row) = self.preaccepted_by_peer.get_mut(&peer) {
                row.insert(key);
            }
        }
        for peer in delta.touched_peers {
            if self
                .preaccepted_by_peer
                .get(&peer)
                .is_some_and(HashSet::is_empty)
            {
                self.preaccepted_by_peer.remove(&peer);
            }
        }
        for key in delta.context_removals {
            self.context_sensitive_accepted.remove(&key);
        }
        for key in delta.context_insertions {
            self.context_sensitive_accepted.insert(key);
        }
        for deadline in delta.deadline_removals {
            self.deadlines.remove(&deadline);
        }
        for deadline in delta.deadline_insertions {
            self.deadlines.insert(deadline);
        }
        for deadline in delta.accepted_deadline_removals {
            self.accepted_deadlines.remove(&deadline);
        }
        for deadline in delta.accepted_deadline_insertions {
            self.accepted_deadlines.insert(deadline);
        }
    }
}

#[cfg(test)]
impl IndexDelta {
    pub(in crate::authority) fn extend_shard_support(
        &self,
        support: &mut super::shard_support::AuthorityShardSupport,
    ) {
        for (proposal, _) in self
            .proposal_removals
            .iter()
            .chain(&self.proposal_insertions)
        {
            support.insert(b"index/proposal", proposal);
        }
        for (peer, _) in self.peer_removals.iter().chain(&self.peer_insertions) {
            support.insert(b"index/peer", peer);
        }
        for (peer, _) in &self.new_peer_rows {
            support.insert(b"index/peer", peer);
        }
        for peer in &self.touched_peers {
            support.insert(b"index/peer", peer);
        }
        for key in self.context_removals.iter().chain(&self.context_insertions) {
            support.insert(b"index/context", key);
        }
        for key in self
            .deadline_removals
            .iter()
            .chain(&self.deadline_insertions)
        {
            support.insert(b"index/deadline", key);
        }
        for key in self
            .accepted_deadline_removals
            .iter()
            .chain(&self.accepted_deadline_insertions)
        {
            support.insert(b"index/accepted-deadline", key);
        }
    }
}

#[cfg(test)]
#[path = "tests/support/indexes.rs"]
pub(in crate::authority) mod test_support;
