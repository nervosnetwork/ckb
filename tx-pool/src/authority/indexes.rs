//! Payload-free projections derived from the primary owner map.
//!
//! Every index transition is compiled from the same owner before/after set.
//! Callers cannot update proposal and ingress-peer views independently.

use super::{
    shard::{AUTHORITY_SHARD_COUNT, ShardedOwnerMap, ShardedOwnerWriteCut},
    state::{AcceptedAtMillis, OwnedTx, ProposalId, RawTxHash, RemoteDeadline},
};
use ckb_network::PeerIndex;
use ckb_util::parking_lot::RwLockReadGuard;
use std::{
    collections::{HashMap, HashSet},
    ops::Bound::{Excluded, Unbounded},
};

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::authority) struct DeadlineKey {
    // Remote residency expiry is immutable for one owner and is validated
    // against the current primary entry when consumed. Compute phase/version
    // churn therefore must not detach and reinsert the same deadline.
    expires_at: RemoteDeadline,
    hash: RawTxHash,
}

/// Accepted expiry is tied to the immutable admission timestamp, not the OCC
/// version. Status-only version changes therefore do not churn this index.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::authority) struct AcceptedDeadlineKey {
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

#[derive(Debug)]
pub(super) struct AuthorityIndexes {
    entries: ShardedOwnerMap,
}

pub(super) struct ContextSensitiveAcceptedReadCut<'authority> {
    shards: [RwLockReadGuard<'authority, super::shard::AuthorityShard>; AUTHORITY_SHARD_COUNT],
}

impl ContextSensitiveAcceptedReadCut<'_> {
    pub(super) fn iter(&self) -> impl Iterator<Item = &RawTxHash> {
        self.shards
            .iter()
            .flat_map(|shard| shard.context_sensitive_accepted.iter())
    }
}

#[expect(
    clippy::indexing_slicing,
    reason = "the sole shard router and fixed-array enumerations stay within the 64-shard layout"
)]
impl AuthorityIndexes {
    pub(super) fn for_entries(entries: &ShardedOwnerMap) -> Self {
        Self {
            entries: entries.clone(),
        }
    }

    fn proposal_shard(&self, proposal: &ProposalId) -> usize {
        self.entries
            .layout
            .router
            .shard(b"index/proposal", proposal)
    }

    fn context_shard(&self, key: &RawTxHash) -> usize {
        self.entries.layout.router.shard(b"index/context", key)
    }

    fn peer_shard(&self, peer: &PeerIndex) -> usize {
        self.entries.layout.router.shard(b"index/peer", peer)
    }

    fn deadline_shard(&self, key: &DeadlineKey) -> usize {
        self.entries.layout.router.shard(b"index/deadline", key)
    }

    fn accepted_deadline_shard(&self, key: &AcceptedDeadlineKey) -> usize {
        self.entries
            .layout
            .router
            .shard(b"index/accepted-deadline", key)
    }

    fn proposal_owner_ref(&self, proposal: &ProposalId) -> Option<RawTxHash> {
        self.entries.layout.shards[self.proposal_shard(proposal)]
            .read()
            .proposals
            .get(proposal)
            .cloned()
    }

    fn contains_context(&self, key: &RawTxHash) -> bool {
        self.entries.layout.shards[self.context_shard(key)]
            .read()
            .context_sensitive_accepted
            .contains(key)
    }

    fn contains_accepted_deadline(&self, key: &AcceptedDeadlineKey) -> bool {
        self.entries.layout.shards[self.accepted_deadline_shard(key)]
            .read()
            .accepted_deadlines
            .contains(key)
    }

    fn contains_deadline(&self, key: &DeadlineKey) -> bool {
        self.entries.layout.shards[self.deadline_shard(key)]
            .read()
            .deadlines
            .contains(key)
    }

    fn peer_contains(&self, peer: PeerIndex, key: &RawTxHash) -> bool {
        self.entries.layout.shards[self.peer_shard(&peer)]
            .read()
            .preaccepted_by_peer
            .get(&peer)
            .is_some_and(|owners| owners.contains(key))
    }
}

#[expect(
    clippy::indexing_slicing,
    reason = "the sole shard router and fixed-array enumerations stay within the 64-shard layout"
)]
impl AuthorityIndexes {
    fn validate_present_fact(&self, key: &RawTxHash, fact: &IndexFact) -> Result<(), IndexError> {
        if self.proposal_owner_ref(&fact.proposal).as_ref() != Some(key) {
            return Err(IndexError::Projection);
        }
        if let Some(peer) = fact.preaccepted_peer
            && !self.peer_contains(peer, key)
        {
            return Err(IndexError::Projection);
        }
        if self.contains_context(key) != fact.context_sensitive_accepted {
            return Err(IndexError::Projection);
        }
        if fact
            .deadline_key(key)
            .is_some_and(|deadline| !self.contains_deadline(&deadline))
        {
            return Err(IndexError::Projection);
        }
        if fact
            .accepted_deadline_key(key)
            .is_some_and(|deadline| !self.contains_accepted_deadline(&deadline))
        {
            return Err(IndexError::Projection);
        }
        Ok(())
    }

    pub(super) fn proposal_owner(&self, proposal: &ProposalId) -> Option<RawTxHash> {
        self.proposal_owner_ref(proposal)
    }

    pub(super) fn preaccepted_for_peer(&self, peer: PeerIndex) -> Option<HashSet<RawTxHash>> {
        self.entries.layout.shards[self.peer_shard(&peer)]
            .read()
            .preaccepted_by_peer
            .get(&peer)
            .cloned()
    }

    pub(super) fn context_sensitive_accepted(&self) -> ContextSensitiveAcceptedReadCut<'_> {
        ContextSensitiveAcceptedReadCut {
            shards: std::array::from_fn(|shard| self.entries.layout.shards[shard].read()),
        }
    }

    pub(super) fn due_remote(
        &self,
        now: RemoteDeadline,
        limit: usize,
    ) -> Result<Vec<DueRemote>, IndexError> {
        let shards: [RwLockReadGuard<'_, super::shard::AuthorityShard>; AUTHORITY_SHARD_COUNT] =
            std::array::from_fn(|shard| self.entries.layout.shards[shard].read());
        let mut rows: [std::collections::btree_set::Iter<'_, DeadlineKey>; AUTHORITY_SHARD_COUNT] =
            std::array::from_fn(|shard| shards[shard].deadlines.iter());
        let mut heads: [Option<&DeadlineKey>; AUTHORITY_SHARD_COUNT] =
            std::array::from_fn(|shard| rows[shard].next());
        let mut due = Vec::new();
        due.try_reserve(limit).map_err(|_| IndexError::Allocation)?;
        while due.len() < limit {
            let Some((shard, deadline)) = heads
                .iter()
                .enumerate()
                .filter_map(|(shard, row)| row.map(|row| (shard, row)))
                .min_by(|(_, left), (_, right)| left.cmp(right))
            else {
                break;
            };
            if deadline.expires_at > now {
                break;
            }
            heads[shard] = rows[shard].next();
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
        let shards: [RwLockReadGuard<'_, super::shard::AuthorityShard>; AUTHORITY_SHARD_COUNT] =
            std::array::from_fn(|shard| self.entries.layout.shards[shard].read());
        let total = shards.iter().map(|shard| shard.deadlines.len()).sum();
        if page.capacity() < limit.min(total) {
            return Err(IndexError::Allocation);
        }
        let after = after.map(|cursor| DeadlineKey {
            expires_at: cursor.expires_at,
            hash: cursor.hash.clone(),
        });
        let start = after.as_ref().map_or(Unbounded, Excluded);
        let mut rows: [std::collections::btree_set::Range<'_, DeadlineKey>; AUTHORITY_SHARD_COUNT] =
            std::array::from_fn(|shard| shards[shard].deadlines.range((start, Unbounded)));
        let mut heads: [Option<&DeadlineKey>; AUTHORITY_SHARD_COUNT] =
            std::array::from_fn(|shard| rows[shard].next());
        while page.len() < limit {
            let Some((shard, deadline)) = heads
                .iter()
                .enumerate()
                .filter_map(|(shard, row)| row.map(|row| (shard, row)))
                .min_by(|(_, left), (_, right)| left.cmp(right))
            else {
                break;
            };
            heads[shard] = rows[shard].next();
            page.push(DueRemote {
                expires_at: deadline.expires_at,
                hash: deadline.hash.clone(),
            });
        }
        Ok(heads.iter().any(Option::is_some))
    }

    pub(super) fn due_accepted(
        &self,
        cutoff: AcceptedAtMillis,
        limit: usize,
    ) -> Result<Vec<DueAccepted>, IndexError> {
        let shards: [RwLockReadGuard<'_, super::shard::AuthorityShard>; AUTHORITY_SHARD_COUNT] =
            std::array::from_fn(|shard| self.entries.layout.shards[shard].read());
        let mut rows: [std::collections::btree_set::Iter<'_, AcceptedDeadlineKey>;
            AUTHORITY_SHARD_COUNT] =
            std::array::from_fn(|shard| shards[shard].accepted_deadlines.iter());
        let mut heads: [Option<&AcceptedDeadlineKey>; AUTHORITY_SHARD_COUNT] =
            std::array::from_fn(|shard| rows[shard].next());
        let mut due = Vec::new();
        due.try_reserve(limit).map_err(|_| IndexError::Allocation)?;
        while due.len() < limit {
            let Some((shard, deadline)) = heads
                .iter()
                .enumerate()
                .filter_map(|(shard, row)| row.map(|row| (shard, row)))
                .min_by(|(_, left), (_, right)| left.cmp(right))
            else {
                break;
            };
            if deadline.accepted_at > cutoff {
                break;
            }
            heads[shard] = rows[shard].next();
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
        &self,
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
                if self.proposal_owner_ref(&after.proposal).is_some() {
                    return Err(IndexError::ProposalCollision);
                }
                self.entries.layout.shards[self.proposal_shard(&after.proposal)]
                    .write()
                    .proposals
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
                if self.peer_contains(peer, key) {
                    return Err(IndexError::Projection);
                }
                let mut shard = self.entries.layout.shards[self.peer_shard(&peer)].write();
                if let Some(row) = shard.preaccepted_by_peer.get_mut(&peer) {
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
                    shard
                        .preaccepted_by_peer
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
                if self.contains_context(key) {
                    return Err(IndexError::Projection);
                }
                self.entries.layout.shards[self.context_shard(key)]
                    .write()
                    .context_sensitive_accepted
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
                if self.contains_deadline(&deadline) {
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
                if self.contains_accepted_deadline(&deadline) {
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
        &self,
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
        &self,
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
                if self.proposal_owner_ref(&before.proposal).as_ref() != Some(&change.key) {
                    return Err(IndexError::Projection);
                }
                if let Some(peer) = before.preaccepted_peer
                    && !self.peer_contains(peer, &change.key)
                {
                    return Err(IndexError::Projection);
                }
                if self.contains_context(&change.key) != before.context_sensitive_accepted {
                    return Err(IndexError::Projection);
                }
                if before
                    .deadline_key(&change.key)
                    .is_some_and(|deadline| !self.contains_deadline(&deadline))
                {
                    return Err(IndexError::Projection);
                }
                if before
                    .accepted_deadline_key(&change.key)
                    .is_some_and(|deadline| !self.contains_accepted_deadline(&deadline))
                {
                    return Err(IndexError::Projection);
                }
            } else if self.contains_context(&change.key) {
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
                self.contains_deadline(deadline)
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
                self.contains_accepted_deadline(deadline)
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
            if let Some(current) = self.proposal_owner_ref(proposal)
                && proposal_removals
                    .binary_search_by(|(removed, _)| removed.cmp(proposal))
                    .ok()
                    .and_then(|index| proposal_removals.get(index))
                    .map(|(_, hash)| hash)
                    != Some(&current)
            {
                return Err(IndexError::ProposalCollision);
            }
        }
        let new_proposals = proposal_insertions
            .iter()
            .filter(|(proposal, _)| self.proposal_owner_ref(proposal).is_none())
            .count();
        if new_proposals != 0 {
            let mut additions = [0usize; AUTHORITY_SHARD_COUNT];
            for (proposal, _) in &proposal_insertions {
                if self.proposal_owner_ref(proposal).is_none() {
                    let shard = self.proposal_shard(proposal);
                    additions[shard] = additions[shard]
                        .checked_add(1)
                        .ok_or(IndexError::Arithmetic)?;
                }
            }
            for (shard, additional) in self.entries.layout.shards.iter().zip(additions) {
                if additional == 0 {
                    continue;
                }
                shard
                    .write()
                    .proposals
                    .try_reserve(additional)
                    .map_err(|_| IndexError::Allocation)?;
            }
        }

        for key in &context_insertions {
            if self.contains_context(key) {
                return Err(IndexError::Projection);
            }
        }
        let mut context_additions = [0usize; AUTHORITY_SHARD_COUNT];
        for key in &context_insertions {
            let shard = self.context_shard(key);
            context_additions[shard] = context_additions[shard]
                .checked_add(1)
                .ok_or(IndexError::Arithmetic)?;
        }
        for (shard, additional) in self.entries.layout.shards.iter().zip(context_additions) {
            if additional == 0 {
                continue;
            }
            shard
                .write()
                .context_sensitive_accepted
                .try_reserve(additional)
                .map_err(|_| IndexError::Allocation)?;
        }

        let mut additions_by_peer = HashMap::<PeerIndex, usize>::new();
        additions_by_peer
            .try_reserve(peer_insertions.len())
            .map_err(|_| IndexError::Allocation)?;
        for (peer, key) in &peer_insertions {
            if self.peer_contains(*peer, key) {
                return Err(IndexError::Projection);
            }
            let count = additions_by_peer.entry(*peer).or_default();
            *count = count.checked_add(1).ok_or(IndexError::Arithmetic)?;
        }
        let new_peer_count = additions_by_peer
            .keys()
            .filter(|peer| self.preaccepted_for_peer(**peer).is_none())
            .count();
        let mut new_peers_by_shard = [0usize; AUTHORITY_SHARD_COUNT];
        for peer in additions_by_peer.keys() {
            if self.preaccepted_for_peer(*peer).is_none() {
                let shard = self.peer_shard(peer);
                new_peers_by_shard[shard] = new_peers_by_shard[shard]
                    .checked_add(1)
                    .ok_or(IndexError::Arithmetic)?;
            }
        }
        for (shard, additional) in self.entries.layout.shards.iter().zip(new_peers_by_shard) {
            if additional == 0 {
                continue;
            }
            shard
                .write()
                .preaccepted_by_peer
                .try_reserve(additional)
                .map_err(|_| IndexError::Allocation)?;
        }

        let mut new_rows = HashMap::<PeerIndex, HashSet<RawTxHash>>::new();
        new_rows
            .try_reserve(new_peer_count)
            .map_err(|_| IndexError::Allocation)?;
        for (peer, additions) in additions_by_peer {
            let mut shard = self.entries.layout.shards[self.peer_shard(&peer)].write();
            if let Some(row) = shard.preaccepted_by_peer.get_mut(&peer) {
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

    pub(super) fn apply(&self, mut delta: IndexDelta) {
        let support = delta.sharded_write_support(&self.entries);
        let mut cut = self.entries.write_cut(support);
        delta.apply_sharded(&self.entries, &mut cut);
    }
}

impl IndexDelta {
    pub(in crate::authority) fn sharded_write_support(
        &self,
        entries: &ShardedOwnerMap,
    ) -> super::shard::ShardWriteSupport {
        let mut support = super::shard::ShardWriteSupport::default();
        for (proposal, key) in self
            .proposal_removals
            .iter()
            .chain(&self.proposal_insertions)
        {
            support.insert(entries.layout.router.shard(b"index/proposal", proposal));
            support.insert(entries.layout.router.shard(b"index/context", key));
        }
        for (peer, _) in self.peer_removals.iter().chain(&self.peer_insertions) {
            support.insert(entries.layout.router.shard(b"index/peer", peer));
        }
        for (peer, _) in &self.new_peer_rows {
            support.insert(entries.layout.router.shard(b"index/peer", peer));
        }
        for peer in &self.touched_peers {
            support.insert(entries.layout.router.shard(b"index/peer", peer));
        }
        for key in self.context_removals.iter().chain(&self.context_insertions) {
            support.insert(entries.layout.router.shard(b"index/context", key));
        }
        for deadline in self
            .deadline_removals
            .iter()
            .chain(&self.deadline_insertions)
        {
            support.insert(entries.layout.router.shard(b"index/deadline", deadline));
        }
        for deadline in self
            .accepted_deadline_removals
            .iter()
            .chain(&self.accepted_deadline_insertions)
        {
            support.insert(
                entries
                    .layout
                    .router
                    .shard(b"index/accepted-deadline", deadline),
            );
        }
        support
    }

    pub(in crate::authority) fn apply_sharded(
        &mut self,
        entries: &ShardedOwnerMap,
        cut: &mut ShardedOwnerWriteCut<'_>,
    ) {
        for (proposal, _) in std::mem::take(&mut self.proposal_removals) {
            let shard = entries.layout.router.shard(b"index/proposal", &proposal);
            cut.projection_shard_mut(shard).proposals.remove(&proposal);
        }
        for (proposal, key) in std::mem::take(&mut self.proposal_insertions) {
            let shard = entries.layout.router.shard(b"index/proposal", &proposal);
            cut.projection_shard_mut(shard)
                .proposals
                .insert(proposal, key);
        }
        for (peer, key) in std::mem::take(&mut self.peer_removals) {
            let shard = entries.layout.router.shard(b"index/peer", &peer);
            if let Some(row) = cut
                .projection_shard_mut(shard)
                .preaccepted_by_peer
                .get_mut(&peer)
            {
                row.remove(&key);
            }
        }
        for (peer, row) in std::mem::take(&mut self.new_peer_rows) {
            let shard = entries.layout.router.shard(b"index/peer", &peer);
            cut.projection_shard_mut(shard)
                .preaccepted_by_peer
                .insert(peer, row);
        }
        for (peer, key) in std::mem::take(&mut self.peer_insertions) {
            let shard = entries.layout.router.shard(b"index/peer", &peer);
            if let Some(row) = cut
                .projection_shard_mut(shard)
                .preaccepted_by_peer
                .get_mut(&peer)
            {
                row.insert(key);
            }
        }
        for peer in std::mem::take(&mut self.touched_peers) {
            let shard = entries.layout.router.shard(b"index/peer", &peer);
            let rows = &mut cut.projection_shard_mut(shard).preaccepted_by_peer;
            if rows.get(&peer).is_some_and(HashSet::is_empty) {
                rows.remove(&peer);
            }
        }
        for key in std::mem::take(&mut self.context_removals) {
            let shard = entries.layout.router.shard(b"index/context", &key);
            cut.projection_shard_mut(shard)
                .context_sensitive_accepted
                .remove(&key);
        }
        for key in std::mem::take(&mut self.context_insertions) {
            let shard = entries.layout.router.shard(b"index/context", &key);
            cut.projection_shard_mut(shard)
                .context_sensitive_accepted
                .insert(key);
        }
        for deadline in std::mem::take(&mut self.deadline_removals) {
            let shard = entries.layout.router.shard(b"index/deadline", &deadline);
            cut.projection_shard_mut(shard).deadlines.remove(&deadline);
        }
        for deadline in std::mem::take(&mut self.deadline_insertions) {
            let shard = entries.layout.router.shard(b"index/deadline", &deadline);
            cut.projection_shard_mut(shard).deadlines.insert(deadline);
        }
        for deadline in std::mem::take(&mut self.accepted_deadline_removals) {
            let shard = entries
                .layout
                .router
                .shard(b"index/accepted-deadline", &deadline);
            cut.projection_shard_mut(shard)
                .accepted_deadlines
                .remove(&deadline);
        }
        for deadline in std::mem::take(&mut self.accepted_deadline_insertions) {
            let shard = entries
                .layout
                .router
                .shard(b"index/accepted-deadline", &deadline);
            cut.projection_shard_mut(shard)
                .accepted_deadlines
                .insert(deadline);
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
