//! Payload-free projections derived from the primary owner map.
//!
//! Every index transition is compiled from the same owner before/after set.
//! Callers cannot update proposal and ingress-peer views independently.

use super::{
    shard::{
        AUTHORITY_SHARD_COUNT, AuthorityShard, PeerIngressRow, ShardReadSupport, ShardWriteSupport,
        ShardedOwnerMap, ShardedOwnerReadCut, ShardedOwnerWriteCut,
    },
    state::{AcceptedAtMillis, EntryVersion, OwnedTx, ProposalId, RawTxHash, RemoteDeadline},
};
use ckb_network::PeerIndex;
use ckb_util::parking_lot::RwLockReadGuard;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::authority) struct DeadlineKey {
    // Remote residency expiry is immutable for one owner and is validated
    // against the current primary entry when consumed. Compute phase/version
    // churn therefore must not detach and reinsert the same deadline.
    pub(in crate::authority) expires_at: RemoteDeadline,
    pub(in crate::authority) hash: RawTxHash,
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
struct RemoteExpiryMember {
    due: DueRemote,
    version: EntryVersion,
}

/// Exact bounded Remote-expiry selection captured from one fixed-layout read
/// cut. `prefix` contains at most the configured maintenance slice and `head`
/// is the first unselected deadline, including a non-due deadline. Together
/// they distinguish an unchanged prefix from an insertion/removal immediately
/// after it without retaining an unbounded index snapshot.
pub(super) struct RemoteExpiryWitness {
    cutoff: RemoteDeadline,
    prefix: Vec<RemoteExpiryMember>,
    head: Option<DueRemote>,
}

impl RemoteExpiryWitness {
    pub(super) fn is_empty(&self) -> bool {
        self.prefix.is_empty()
    }

    pub(super) fn len(&self) -> usize {
        self.prefix.len()
    }

    pub(super) fn members(&self) -> impl ExactSizeIterator<Item = (&DueRemote, EntryVersion)> {
        self.prefix
            .iter()
            .map(|member| (&member.due, member.version))
    }

    /// Effect policy may select a strict prefix of the caller's maintenance
    /// slice. The first discarded due member becomes the new exact head; no
    /// rescan or allocation is needed.
    pub(super) fn truncate(&mut self, len: usize) -> Result<(), IndexError> {
        if len == 0 || len > self.prefix.len() {
            return Err(IndexError::Projection);
        }
        if len < self.prefix.len() {
            self.head = self.prefix.get(len).map(|member| member.due.clone());
            self.prefix.truncate(len);
        }
        Ok(())
    }

    /// Remote deadlines can be inserted on any owner shard. Reading every
    /// non-written shard is therefore the smallest exact global-order proof;
    /// the layout is fixed at 64 shards, so support and lock work stay O(64).
    pub(in crate::authority) fn extend_final_read_support(&self, reads: &mut ShardReadSupport) {
        for shard in 0..AUTHORITY_SHARD_COUNT {
            reads.insert(shard);
        }
    }

    /// Compare the already-allocated prefix and next head against the final
    /// mixed owner/index cut. This performs O(64 + prefix.len()) bounded work
    /// and allocates nothing while any shard guard is held.
    pub(in crate::authority) fn prestate_is_fresh(
        &self,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
    ) -> bool {
        let mut rows: [std::collections::btree_set::Iter<'_, DeadlineKey>; AUTHORITY_SHARD_COUNT] =
            std::array::from_fn(|shard| cut.projection_shard(shard).deadlines.iter());
        let mut heads: [Option<&DeadlineKey>; AUTHORITY_SHARD_COUNT] =
            std::array::from_fn(|shard| rows.get_mut(shard).and_then(|row| row.next()));
        for expected in &self.prefix {
            let Some((shard, actual)) = heads
                .iter()
                .enumerate()
                .filter_map(|(shard, row)| row.map(|row| (shard, row)))
                .min_by(|(_, left), (_, right)| left.cmp(right))
            else {
                return false;
            };
            if actual.expires_at > self.cutoff
                || actual.expires_at != expected.due.expires_at
                || actual.hash != expected.due.hash
                || cut
                    .owner(entries, &actual.hash)
                    .is_none_or(|owner| owner.record().version != expected.version)
            {
                return false;
            }
            let Some(row) = rows.get_mut(shard) else {
                return false;
            };
            let Some(head) = heads.get_mut(shard) else {
                return false;
            };
            *head = row.next();
        }
        let actual_head = heads
            .iter()
            .filter_map(|row| *row)
            .min()
            .map(|deadline| (deadline.expires_at, &deadline.hash));
        let expected_head = self
            .head
            .as_ref()
            .map(|deadline| (deadline.expires_at, &deadline.hash));
        actual_head == expected_head
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DueAccepted {
    pub(super) accepted_at: AcceptedAtMillis,
    pub(super) hash: RawTxHash,
}

/// Oldest due Accepted owner selected from one coherent fixed-layout cut.
/// Unlike Remote prefix expiry, the witness intentionally carries no next
/// head: an independently inserted earlier Accepted deadline overlaps this
/// selection and may linearize after it without invalidating the selected
/// owner's exact removal.
pub(super) struct AcceptedExpiryHead {
    due: DueAccepted,
    version: EntryVersion,
}

impl AcceptedExpiryHead {
    pub(super) fn due(&self) -> &DueAccepted {
        &self.due
    }

    pub(super) const fn version(&self) -> EntryVersion {
        self.version
    }

    /// Revalidate the exact selected deadline and owner incarnation without
    /// constraining unrelated deadline heads that were inserted after the
    /// coherent selection cut.
    pub(in crate::authority) fn prestate_is_fresh(
        &self,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
    ) -> bool {
        let shard = entries.owner_shard(&self.due.hash);
        cut.projection_shard(shard)
            .accepted_deadlines
            .contains(&AcceptedDeadlineKey {
                accepted_at: self.due.accepted_at,
                hash: self.due.hash.clone(),
            })
            && matches!(
                cut.owner(entries, &self.due.hash),
                Some(OwnedTx::Accepted(owner))
                    if owner.record.version == self.version
                        && owner.accepted_at == self.due.accepted_at
            )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum IndexError {
    Stale,
    ProposalCollision,
    Projection,
    Arithmetic,
    Allocation,
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
    prestate: IndexPrestate,
}

#[derive(Default, PartialEq, Eq)]
struct IndexPrestate {
    proposals: Vec<(ProposalId, Option<RawTxHash>)>,
    peers: Vec<(PeerIndex, Option<PeerIngressRow>)>,
    contexts: Vec<(RawTxHash, bool)>,
    deadlines: Vec<(DeadlineKey, bool)>,
    accepted_deadlines: Vec<(AcceptedDeadlineKey, bool)>,
}

pub(in crate::authority) struct RetainedIndexPremise(IndexPrestate);

trait IndexReadCut {
    fn index_shard(&self, shard: usize) -> &AuthorityShard;
}

impl IndexReadCut for ShardedOwnerReadCut<'_> {
    fn index_shard(&self, shard: usize) -> &AuthorityShard {
        self.projection_shard(shard)
    }
}

impl IndexReadCut for ShardedOwnerWriteCut<'_> {
    fn index_shard(&self, shard: usize) -> &AuthorityShard {
        self.projection_shard(shard)
    }
}

impl IndexPrestate {
    fn capture_retained<'entry>(
        indexes: &AuthorityIndexes,
        changes: impl IntoIterator<
            Item = (
                &'entry RawTxHash,
                Option<&'entry OwnedTx>,
                Option<&'entry OwnedTx>,
            ),
        >,
        cut: &impl IndexReadCut,
    ) -> Result<RetainedIndexPremise, IndexError> {
        let mut proposals = Vec::new();
        let mut peers = Vec::new();
        for (key, before, after) in changes {
            for owner in [before, after].into_iter().flatten() {
                let fact = IndexFact::from_owner(key, owner)?;
                proposals.push(fact.proposal);
                peers.extend(fact.preaccepted_peer);
            }
        }
        proposals.sort_unstable();
        proposals.dedup();
        peers.sort_unstable();
        peers.dedup();

        let mut prestate = Self::default();
        prestate.proposals.reserve_exact(proposals.len());
        prestate
            .proposals
            .extend(proposals.into_iter().map(|proposal| {
                let shard = indexes.proposal_shard(&proposal);
                let owner = cut.index_shard(shard).proposals.get(&proposal).cloned();
                (proposal, owner)
            }));
        prestate.peers.reserve_exact(peers.len());
        prestate.peers.extend(peers.into_iter().map(|peer| {
            let shard = indexes.peer_shard(&peer);
            (peer, cut.index_shard(shard).peer_ingress_row(peer))
        }));
        Ok(RetainedIndexPremise(prestate))
    }

    fn capture_in_cut(
        indexes: &AuthorityIndexes,
        delta: &IndexDelta,
        cut: &impl IndexReadCut,
    ) -> Result<Self, IndexError> {
        let mut prestate = Self::default();

        let mut proposals = Vec::new();
        proposals.reserve_exact(
            delta
                .proposal_removals
                .len()
                .checked_add(delta.proposal_insertions.len())
                .ok_or(IndexError::Arithmetic)?,
        );
        proposals.extend(
            delta
                .proposal_removals
                .iter()
                .map(|(proposal, _)| proposal.clone()),
        );
        proposals.extend(
            delta
                .proposal_insertions
                .iter()
                .map(|(proposal, _)| proposal.clone()),
        );
        proposals.sort_unstable();
        proposals.dedup();
        prestate.proposals.reserve_exact(proposals.len());
        prestate
            .proposals
            .extend(proposals.into_iter().map(|proposal| {
                let shard = indexes.proposal_shard(&proposal);
                let owner = cut.index_shard(shard).proposals.get(&proposal).cloned();
                (proposal, owner)
            }));

        let mut peers = Vec::new();
        peers.reserve_exact(
            delta
                .peer_removals
                .len()
                .checked_add(delta.peer_insertions.len())
                .and_then(|count| count.checked_add(delta.new_peer_rows.len()))
                .and_then(|count| count.checked_add(delta.touched_peers.len()))
                .ok_or(IndexError::Arithmetic)?,
        );
        peers.extend(delta.peer_removals.iter().map(|(peer, _)| *peer));
        peers.extend(delta.peer_insertions.iter().map(|(peer, _)| *peer));
        peers.extend(delta.new_peer_rows.iter().map(|(peer, _)| *peer));
        peers.extend(delta.touched_peers.iter().copied());
        peers.sort_unstable();
        peers.dedup();
        prestate.peers.reserve_exact(peers.len());
        prestate.peers.extend(peers.into_iter().map(|peer| {
            let shard = indexes.peer_shard(&peer);
            (peer, cut.index_shard(shard).peer_ingress_row(peer))
        }));

        let mut contexts = Vec::new();
        contexts.reserve_exact(
            delta
                .context_removals
                .len()
                .checked_add(delta.context_insertions.len())
                .ok_or(IndexError::Arithmetic)?,
        );
        contexts.extend(delta.context_removals.iter().cloned());
        contexts.extend(delta.context_insertions.iter().cloned());
        contexts.sort_unstable();
        contexts.dedup();
        prestate.contexts.reserve_exact(contexts.len());
        prestate.contexts.extend(contexts.into_iter().map(|key| {
            let present = cut
                .index_shard(indexes.context_shard(&key))
                .context_sensitive_accepted
                .contains(&key);
            (key, present)
        }));

        let mut deadlines = Vec::new();
        deadlines.reserve_exact(
            delta
                .deadline_removals
                .len()
                .checked_add(delta.deadline_insertions.len())
                .ok_or(IndexError::Arithmetic)?,
        );
        deadlines.extend(delta.deadline_removals.iter().cloned());
        deadlines.extend(delta.deadline_insertions.iter().cloned());
        deadlines.sort_unstable();
        deadlines.dedup();
        prestate.deadlines.reserve_exact(deadlines.len());
        prestate.deadlines.extend(deadlines.into_iter().map(|key| {
            let present = cut
                .index_shard(indexes.deadline_shard(&key))
                .deadlines
                .contains(&key);
            (key, present)
        }));

        let mut accepted_deadlines = Vec::new();
        accepted_deadlines.reserve_exact(
            delta
                .accepted_deadline_removals
                .len()
                .checked_add(delta.accepted_deadline_insertions.len())
                .ok_or(IndexError::Arithmetic)?,
        );
        accepted_deadlines.extend(delta.accepted_deadline_removals.iter().cloned());
        accepted_deadlines.extend(delta.accepted_deadline_insertions.iter().cloned());
        accepted_deadlines.sort_unstable();
        accepted_deadlines.dedup();
        prestate
            .accepted_deadlines
            .reserve_exact(accepted_deadlines.len());
        prestate
            .accepted_deadlines
            .extend(accepted_deadlines.into_iter().map(|key| {
                let present = cut
                    .index_shard(indexes.accepted_deadline_shard(&key))
                    .accepted_deadlines
                    .contains(&key);
                (key, present)
            }));

        Ok(prestate)
    }

    fn is_fresh(
        &self,
        entries: &ShardedOwnerMap,
        cut: &impl IndexReadCut,
        allowed_hidden_peer: Option<(PeerIndex, u64)>,
    ) -> bool {
        self.proposals.iter().all(|(proposal, expected)| {
            let shard = entries.layout.router.shard(b"index/proposal", proposal);
            cut.index_shard(shard).proposals.get(proposal) == expected.as_ref()
        }) && self.peers.iter().all(|(peer, expected)| {
            let shard = entries.layout.router.shard(b"index/peer", peer);
            let hidden_allowed = allowed_hidden_peer.is_some_and(|(allowed_peer, stage_id)| {
                allowed_peer == *peer
                    && expected.as_ref().and_then(|row| row.fence.hidden_stage()) == Some(stage_id)
            });
            (hidden_allowed
                || !expected
                    .as_ref()
                    .is_some_and(PeerIngressRow::has_hidden_fence))
                && cut
                    .index_shard(shard)
                    .peer_ingress_row_matches(*peer, expected.as_ref())
        }) && self.contexts.iter().all(|(key, expected)| {
            cut.index_shard(entries.owner_shard(key))
                .context_sensitive_accepted
                .contains(key)
                == *expected
        }) && self.deadlines.iter().all(|(key, expected)| {
            cut.index_shard(entries.owner_shard(&key.hash))
                .deadlines
                .contains(key)
                == *expected
        }) && self.accepted_deadlines.iter().all(|(key, expected)| {
            cut.index_shard(entries.owner_shard(&key.hash))
                .accepted_deadlines
                .contains(key)
                == *expected
        })
    }
}

impl RetainedIndexPremise {
    fn is_current(&self, indexes: &AuthorityIndexes) -> bool {
        let mut reads = ShardReadSupport::default();
        for (proposal, _) in &self.0.proposals {
            reads.insert(indexes.proposal_shard(proposal));
        }
        for (peer, _) in &self.0.peers {
            reads.insert(indexes.peer_shard(peer));
        }
        let cut = indexes
            .entries
            .mixed_cut(reads, super::shard::ShardWriteSupport::default());
        self.0.is_fresh(&indexes.entries, &cut, None)
    }
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
    pub(in crate::authority) fn capture_retained_premise<'entry>(
        &self,
        changes: impl IntoIterator<
            Item = (
                &'entry RawTxHash,
                Option<&'entry OwnedTx>,
                Option<&'entry OwnedTx>,
            ),
        >,
        cut: &ShardedOwnerWriteCut<'_>,
    ) -> Result<RetainedIndexPremise, IndexError> {
        IndexPrestate::capture_retained(self, changes, cut)
    }

    pub(in crate::authority) fn plan_retained_replacements<'entry>(
        &self,
        changes: impl IntoIterator<
            Item = (
                &'entry RawTxHash,
                Option<&'entry OwnedTx>,
                Option<&'entry OwnedTx>,
            ),
        >,
        premise: RetainedIndexPremise,
    ) -> Result<IndexDelta, IndexError> {
        match self.plan_replacements(changes) {
            Ok(delta) => delta.bind_retained_premise(premise),
            Err(_) if !premise.is_current(self) => Err(IndexError::Stale),
            Err(error) => Err(error),
        }
    }

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
        self.entries.owner_shard(key)
    }

    fn peer_shard(&self, peer: &PeerIndex) -> usize {
        self.entries.layout.router.shard(b"index/peer", peer)
    }

    fn deadline_shard(&self, key: &DeadlineKey) -> usize {
        self.entries.owner_shard(&key.hash)
    }

    fn accepted_deadline_shard(&self, key: &AcceptedDeadlineKey) -> usize {
        self.entries.owner_shard(&key.hash)
    }

    fn proposal_owner_ref(&self, proposal: &ProposalId) -> Option<RawTxHash> {
        self.entries.layout.shards[self.proposal_shard(proposal)]
            .read()
            .proposals
            .get(proposal)
            .cloned()
    }
}

#[expect(
    clippy::indexing_slicing,
    reason = "the sole shard router and fixed-array enumerations stay within the 64-shard layout"
)]
impl AuthorityIndexes {
    fn validate_present_fact_in_cut(
        &self,
        key: &RawTxHash,
        fact: &IndexFact,
        cut: &impl IndexReadCut,
    ) -> Result<(), IndexError> {
        let proposal = cut
            .index_shard(self.proposal_shard(&fact.proposal))
            .proposals
            .get(&fact.proposal);
        if proposal != Some(key) {
            return Err(IndexError::Projection);
        }
        if let Some(peer) = fact.preaccepted_peer
            && !cut
                .index_shard(self.peer_shard(&peer))
                .peer_ingress_owners
                .get(&peer)
                .is_some_and(|owners| owners.contains(key))
        {
            return Err(IndexError::Projection);
        }
        if cut
            .index_shard(self.context_shard(key))
            .context_sensitive_accepted
            .contains(key)
            != fact.context_sensitive_accepted
        {
            return Err(IndexError::Projection);
        }
        if fact.deadline_key(key).is_some_and(|deadline| {
            !cut.index_shard(self.deadline_shard(&deadline))
                .deadlines
                .contains(&deadline)
        }) {
            return Err(IndexError::Projection);
        }
        if fact.accepted_deadline_key(key).is_some_and(|deadline| {
            !cut.index_shard(self.accepted_deadline_shard(&deadline))
                .accepted_deadlines
                .contains(&deadline)
        }) {
            return Err(IndexError::Projection);
        }
        Ok(())
    }

    fn replacement_write_support(&self, changes: &[IndexChange]) -> ShardWriteSupport {
        let mut support = ShardWriteSupport::default();
        for change in changes {
            support.insert(self.context_shard(&change.key));
            for fact in [change.before.as_ref(), change.after.as_ref()]
                .into_iter()
                .flatten()
            {
                support.insert(self.proposal_shard(&fact.proposal));
                if let Some(peer) = fact.preaccepted_peer {
                    support.insert(self.peer_shard(&peer));
                }
            }
        }
        support
    }

    pub(super) fn proposal_owner(&self, proposal: &ProposalId) -> Option<RawTxHash> {
        self.proposal_owner_ref(proposal)
    }

    pub(super) fn preaccepted_for_peer(&self, peer: PeerIndex) -> Option<HashSet<RawTxHash>> {
        let owners = self.entries.layout.shards[self.peer_shard(&peer)]
            .read()
            .peer_ingress_owners
            .get(&peer)
            .cloned();
        owners.filter(|owners| !owners.is_empty())
    }

    pub(super) fn context_sensitive_accepted(&self) -> ContextSensitiveAcceptedReadCut<'_> {
        ContextSensitiveAcceptedReadCut {
            shards: std::array::from_fn(|shard| self.entries.layout.shards[shard].read()),
        }
    }

    pub(super) fn remote_expiry_witness(
        &self,
        cutoff: RemoteDeadline,
        limit: usize,
    ) -> Result<RemoteExpiryWitness, IndexError> {
        let mut prefix = Vec::with_capacity(limit);
        let mut reads = ShardReadSupport::default();
        for shard in 0..AUTHORITY_SHARD_COUNT {
            reads.insert(shard);
        }
        let cut = self
            .entries
            .mixed_cut(reads, super::shard::ShardWriteSupport::default());
        let mut rows: [std::collections::btree_set::Iter<'_, DeadlineKey>; AUTHORITY_SHARD_COUNT] =
            std::array::from_fn(|shard| cut.projection_shard(shard).deadlines.iter());
        let mut heads: [Option<&DeadlineKey>; AUTHORITY_SHARD_COUNT] =
            std::array::from_fn(|shard| rows.get_mut(shard).and_then(|row| row.next()));
        while prefix.len() < limit {
            let Some((shard, deadline)) = heads
                .iter()
                .enumerate()
                .filter_map(|(shard, row)| row.map(|row| (shard, row)))
                .min_by(|(_, left), (_, right)| left.cmp(right))
            else {
                break;
            };
            if deadline.expires_at > cutoff {
                break;
            }
            let Some(OwnedTx::PreAccepted(owner)) = cut.owner(&self.entries, &deadline.hash) else {
                return Err(IndexError::Projection);
            };
            if owner.source.active_remote_deadline() != Some(deadline.expires_at) {
                return Err(IndexError::Projection);
            }
            prefix.push(RemoteExpiryMember {
                due: DueRemote {
                    expires_at: deadline.expires_at,
                    hash: deadline.hash.clone(),
                },
                version: owner.record.version,
            });
            let row = rows.get_mut(shard).ok_or(IndexError::Projection)?;
            let head = heads.get_mut(shard).ok_or(IndexError::Projection)?;
            *head = row.next();
        }
        let head = heads
            .iter()
            .filter_map(|row| *row)
            .min()
            .map(|deadline| DueRemote {
                expires_at: deadline.expires_at,
                hash: deadline.hash.clone(),
            });
        Ok(RemoteExpiryWitness {
            cutoff,
            prefix,
            head,
        })
    }

    /// Select one oldest due Accepted owner from a coherent 64-shard read
    /// cut. The fixed fan-in is index work, not a retained final-cut gate.
    pub(super) fn accepted_expiry_head(
        &self,
        cutoff: AcceptedAtMillis,
    ) -> Result<Option<AcceptedExpiryHead>, IndexError> {
        let mut reads = ShardReadSupport::default();
        for shard in 0..AUTHORITY_SHARD_COUNT {
            reads.insert(shard);
        }
        let cut = self
            .entries
            .mixed_cut(reads, super::shard::ShardWriteSupport::default());
        let head = (0..AUTHORITY_SHARD_COUNT)
            .filter_map(|shard| cut.projection_shard(shard).accepted_deadlines.first())
            .min();
        let Some(head) = head else {
            return Ok(None);
        };
        if head.accepted_at > cutoff {
            return Ok(None);
        }
        let Some(OwnedTx::Accepted(owner)) = cut.owner(&self.entries, &head.hash) else {
            return Err(IndexError::Projection);
        };
        if owner.accepted_at != head.accepted_at {
            return Err(IndexError::Projection);
        }
        Ok(Some(AcceptedExpiryHead {
            due: DueAccepted {
                accepted_at: head.accepted_at,
                hash: head.hash.clone(),
            },
            version: owner.record.version,
        }))
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
        let mut support = ShardWriteSupport::default();
        support.insert(self.context_shard(key));
        for fact in [before.as_ref(), after.as_ref()].into_iter().flatten() {
            support.insert(self.proposal_shard(&fact.proposal));
            if let Some(peer) = fact.preaccepted_peer {
                support.insert(self.peer_shard(&peer));
            }
        }
        let mut cut = self.entries.write_cut(support);
        if let Some(before) = &before {
            self.validate_present_fact_in_cut(key, before, &cut)?;
        }

        let mut delta = IndexDelta::default();
        if before.as_ref().map(|fact| &fact.proposal) != after.as_ref().map(|fact| &fact.proposal) {
            if let Some(before) = &before {
                delta.proposal_removals.reserve(1);
                delta
                    .proposal_removals
                    .push((before.proposal.clone(), key.clone()));
            }
            if let Some(after) = &after {
                if cut
                    .projection_shard(self.proposal_shard(&after.proposal))
                    .proposals
                    .contains_key(&after.proposal)
                {
                    return Err(IndexError::ProposalCollision);
                }
                cut.projection_shard_mut(self.proposal_shard(&after.proposal))
                    .proposals
                    .reserve(1);
                delta.proposal_insertions.reserve(1);
                delta
                    .proposal_insertions
                    .push((after.proposal.clone(), key.clone()));
            }
        }

        let before_peer = before.as_ref().and_then(|fact| fact.preaccepted_peer);
        let after_peer = after.as_ref().and_then(|fact| fact.preaccepted_peer);
        for peer in [before_peer, after_peer].into_iter().flatten() {
            delta.touched_peers.reserve(1);
            delta.touched_peers.push(peer);
        }
        if before_peer != after_peer {
            if let Some(peer) = before_peer {
                delta.peer_removals.reserve(1);
                delta.peer_removals.push((peer, key.clone()));
            }
            if let Some(peer) = after_peer {
                if cut
                    .projection_shard(self.peer_shard(&peer))
                    .peer_ingress_owners
                    .get(&peer)
                    .is_some_and(|owners| owners.contains(key))
                {
                    return Err(IndexError::Projection);
                }
                let shard = cut.projection_shard_mut(self.peer_shard(&peer));
                if let Some(owners) = shard.peer_ingress_owners.get_mut(&peer) {
                    owners.reserve(1);
                    delta.peer_insertions.reserve(1);
                    delta.peer_insertions.push((peer, key.clone()));
                } else {
                    shard.peer_ingress_owners.reserve(1);
                    let mut row = HashSet::with_capacity(1);
                    row.insert(key.clone());
                    delta.new_peer_rows.reserve(1);
                    delta.new_peer_rows.push((peer, row));
                }
            }
        }
        delta.touched_peers.sort_unstable();
        delta.touched_peers.dedup();
        let before_context = before
            .as_ref()
            .is_some_and(|fact| fact.context_sensitive_accepted);
        let after_context = after
            .as_ref()
            .is_some_and(|fact| fact.context_sensitive_accepted);
        if before_context != after_context {
            if before_context {
                delta.context_removals.reserve(1);
                delta.context_removals.push(key.clone());
            }
            if after_context {
                if cut
                    .projection_shard(self.context_shard(key))
                    .context_sensitive_accepted
                    .contains(key)
                {
                    return Err(IndexError::Projection);
                }
                cut.projection_shard_mut(self.context_shard(key))
                    .context_sensitive_accepted
                    .reserve(1);
                delta.context_insertions.reserve(1);
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
                if cut
                    .projection_shard(self.deadline_shard(&deadline))
                    .deadlines
                    .contains(&deadline)
                {
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
                if cut
                    .projection_shard(self.accepted_deadline_shard(&deadline))
                    .accepted_deadlines
                    .contains(&deadline)
                {
                    return Err(IndexError::Projection);
                }
                delta.accepted_deadline_insertions.push(deadline);
            }
        }
        delta.prestate = IndexPrestate::capture_in_cut(self, &delta, &cut)?;
        Ok(delta)
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
        let mut changes = Vec::with_capacity(input.size_hint().1.unwrap_or(0));
        for (key, before, after) in input.by_ref() {
            if changes.len() == changes.capacity() {
                changes.reserve(1);
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
        let support = self.replacement_write_support(&changes);
        let mut cut = self.entries.write_cut(support);

        let mut proposal_removals = Vec::with_capacity(changes.len());
        let mut proposal_insertions = Vec::with_capacity(changes.len());
        let mut peer_removals = Vec::with_capacity(changes.len());
        let mut peer_insertions = Vec::with_capacity(changes.len());
        let mut context_removals = Vec::with_capacity(changes.len());
        let mut context_insertions = Vec::with_capacity(changes.len());
        let mut deadline_removals = Vec::with_capacity(changes.len());
        let mut deadline_insertions = Vec::with_capacity(changes.len());
        let mut accepted_deadline_removals = Vec::with_capacity(changes.len());
        let mut accepted_deadline_insertions = Vec::with_capacity(changes.len());
        for change in &changes {
            if let Some(before) = &change.before {
                self.validate_present_fact_in_cut(&change.key, before, &cut)?;
            } else if cut
                .projection_shard(self.context_shard(&change.key))
                .context_sensitive_accepted
                .contains(&change.key)
            {
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
                cut.projection_shard(self.deadline_shard(deadline))
                    .deadlines
                    .contains(deadline)
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
                cut.projection_shard(self.accepted_deadline_shard(deadline))
                    .accepted_deadlines
                    .contains(deadline)
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
            let current = cut
                .projection_shard(self.proposal_shard(proposal))
                .proposals
                .get(proposal);
            if let Some(current) = current
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
            .filter(|(proposal, _)| {
                !cut.projection_shard(self.proposal_shard(proposal))
                    .proposals
                    .contains_key(proposal)
            })
            .count();
        if new_proposals != 0 {
            let mut additions = [0usize; AUTHORITY_SHARD_COUNT];
            for (proposal, _) in &proposal_insertions {
                if !cut
                    .projection_shard(self.proposal_shard(proposal))
                    .proposals
                    .contains_key(proposal)
                {
                    let shard = self.proposal_shard(proposal);
                    additions[shard] = additions[shard]
                        .checked_add(1)
                        .ok_or(IndexError::Arithmetic)?;
                }
            }
            for (shard, additional) in additions.into_iter().enumerate() {
                if additional == 0 {
                    continue;
                }
                cut.projection_shard_mut(shard)
                    .proposals
                    .reserve(additional);
            }
        }

        for key in &context_insertions {
            if cut
                .projection_shard(self.context_shard(key))
                .context_sensitive_accepted
                .contains(key)
            {
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
        for (shard, additional) in context_additions.into_iter().enumerate() {
            if additional == 0 {
                continue;
            }
            cut.projection_shard_mut(shard)
                .context_sensitive_accepted
                .reserve(additional);
        }

        let mut additions_by_peer =
            HashMap::<PeerIndex, usize>::with_capacity(peer_insertions.len());
        for (peer, key) in &peer_insertions {
            if cut
                .projection_shard(self.peer_shard(peer))
                .peer_ingress_owners
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
            .filter(|peer| {
                !cut.projection_shard(self.peer_shard(peer))
                    .peer_ingress_owners
                    .contains_key(peer)
            })
            .count();
        let mut new_peers_by_shard = [0usize; AUTHORITY_SHARD_COUNT];
        for peer in additions_by_peer.keys() {
            if !cut
                .projection_shard(self.peer_shard(peer))
                .peer_ingress_owners
                .contains_key(peer)
            {
                let shard = self.peer_shard(peer);
                new_peers_by_shard[shard] = new_peers_by_shard[shard]
                    .checked_add(1)
                    .ok_or(IndexError::Arithmetic)?;
            }
        }
        for (shard, additional) in new_peers_by_shard.into_iter().enumerate() {
            if additional == 0 {
                continue;
            }
            cut.projection_shard_mut(shard)
                .peer_ingress_owners
                .reserve(additional);
        }

        let mut new_rows = HashMap::<PeerIndex, HashSet<RawTxHash>>::with_capacity(new_peer_count);
        for (peer, additions) in additions_by_peer {
            let shard = cut.projection_shard_mut(self.peer_shard(&peer));
            if let Some(owners) = shard.peer_ingress_owners.get_mut(&peer) {
                owners.reserve(additions);
            } else {
                let row = HashSet::with_capacity(additions);
                new_rows.insert(peer, row);
            }
        }

        let mut retained_peer_insertions = Vec::with_capacity(peer_insertions.len());
        for (peer, key) in peer_insertions {
            if let Some(row) = new_rows.get_mut(&peer) {
                if !row.insert(key) {
                    return Err(IndexError::Projection);
                }
            } else {
                retained_peer_insertions.push((peer, key));
            }
        }

        let mut touched_peers =
            Vec::with_capacity(changes.len().checked_mul(2).ok_or(IndexError::Arithmetic)?);
        for change in &changes {
            touched_peers.extend(
                [
                    change
                        .before
                        .as_ref()
                        .and_then(|fact| fact.preaccepted_peer),
                    change.after.as_ref().and_then(|fact| fact.preaccepted_peer),
                ]
                .into_iter()
                .flatten(),
            );
        }
        touched_peers.sort_unstable();
        touched_peers.dedup();

        let mut new_peer_rows = Vec::with_capacity(new_rows.len());
        new_peer_rows.extend(new_rows);
        new_peer_rows.sort_unstable_by_key(|(peer, _)| *peer);
        peer_removals.sort_unstable();
        retained_peer_insertions.sort_unstable();
        let mut delta = IndexDelta {
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
            prestate: IndexPrestate::default(),
        };
        delta.prestate = IndexPrestate::capture_in_cut(self, &delta, &cut)?;
        Ok(delta)
    }

    pub(super) fn apply(&self, mut delta: IndexDelta) {
        let support = delta.sharded_write_support(&self.entries);
        let mut cut = self.entries.write_cut(support);
        delta.apply_sharded(&self.entries, &mut cut);
    }
}

impl IndexDelta {
    fn bind_retained_premise(mut self, premise: RetainedIndexPremise) -> Result<Self, IndexError> {
        for (proposal, expected) in premise.0.proposals {
            match self
                .prestate
                .proposals
                .binary_search_by(|(current, _)| current.cmp(&proposal))
            {
                Ok(index)
                    if self
                        .prestate
                        .proposals
                        .get(index)
                        .is_some_and(|(_, current)| current == &expected) =>
                {
                    continue;
                }
                Ok(_) => return Err(IndexError::Stale),
                Err(_) => self.prestate.proposals.push((proposal, expected)),
            }
        }
        self.prestate
            .proposals
            .sort_unstable_by(|left, right| left.0.cmp(&right.0));
        for (peer, expected) in premise.0.peers {
            match self
                .prestate
                .peers
                .binary_search_by_key(&peer, |(current, _)| *current)
            {
                Ok(index)
                    if self
                        .prestate
                        .peers
                        .get(index)
                        .is_some_and(|(_, current)| current == &expected) =>
                {
                    continue;
                }
                Ok(_) => return Err(IndexError::Stale),
                Err(_) => self.prestate.peers.push((peer, expected)),
            }
        }
        self.prestate.peers.sort_unstable_by_key(|(peer, _)| *peer);
        Ok(self)
    }

    pub(in crate::authority) fn prestate_is_fresh(
        &self,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
    ) -> bool {
        self.prestate.is_fresh(entries, cut, None)
    }

    pub(in crate::authority) fn prestate_is_fresh_for_peer_revocation(
        &self,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
        peer: PeerIndex,
        stage_id: u64,
    ) -> bool {
        self.prestate.is_fresh(entries, cut, Some((peer, stage_id)))
    }

    pub(in crate::authority) fn sharded_write_support(
        &self,
        entries: &ShardedOwnerMap,
    ) -> super::shard::ShardWriteSupport {
        let mut support = super::shard::ShardWriteSupport::default();
        for (proposal, _) in self
            .proposal_removals
            .iter()
            .chain(&self.proposal_insertions)
        {
            support.insert(entries.layout.router.shard(b"index/proposal", proposal));
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
            support.insert(entries.owner_shard(key));
        }
        for deadline in self
            .deadline_removals
            .iter()
            .chain(&self.deadline_insertions)
        {
            support.insert(entries.owner_shard(&deadline.hash));
        }
        for deadline in self
            .accepted_deadline_removals
            .iter()
            .chain(&self.accepted_deadline_insertions)
        {
            support.insert(entries.owner_shard(&deadline.hash));
        }
        support
    }

    pub(in crate::authority) fn sharded_read_support(
        &self,
        entries: &ShardedOwnerMap,
    ) -> ShardReadSupport {
        let mut support = ShardReadSupport::default();
        for (proposal, _) in &self.prestate.proposals {
            support.insert(entries.layout.router.shard(b"index/proposal", proposal));
        }
        for (peer, _) in &self.prestate.peers {
            support.insert(entries.layout.router.shard(b"index/peer", peer));
        }
        for (key, _) in &self.prestate.contexts {
            support.insert(entries.owner_shard(key));
        }
        for (deadline, _) in &self.prestate.deadlines {
            support.insert(entries.owner_shard(&deadline.hash));
        }
        for (deadline, _) in &self.prestate.accepted_deadlines {
            support.insert(entries.owner_shard(&deadline.hash));
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
                .peer_ingress_owners
                .get_mut(&peer)
            {
                row.remove(&key);
            }
        }
        for (peer, row) in std::mem::take(&mut self.new_peer_rows) {
            let shard = entries.layout.router.shard(b"index/peer", &peer);
            cut.projection_shard_mut(shard)
                .peer_ingress_owners
                .insert(peer, row);
        }
        for (peer, key) in std::mem::take(&mut self.peer_insertions) {
            let shard = entries.layout.router.shard(b"index/peer", &peer);
            if let Some(row) = cut
                .projection_shard_mut(shard)
                .peer_ingress_owners
                .get_mut(&peer)
            {
                row.insert(key);
            }
        }
        for peer in std::mem::take(&mut self.touched_peers) {
            let shard = entries.layout.router.shard(b"index/peer", &peer);
            let rows = &mut cut.projection_shard_mut(shard).peer_ingress_owners;
            if rows.get(&peer).is_some_and(HashSet::is_empty) {
                rows.remove(&peer);
            }
        }
        for key in std::mem::take(&mut self.context_removals) {
            let shard = entries.owner_shard(&key);
            cut.projection_shard_mut(shard)
                .context_sensitive_accepted
                .remove(&key);
        }
        for key in std::mem::take(&mut self.context_insertions) {
            let shard = entries.owner_shard(&key);
            cut.projection_shard_mut(shard)
                .context_sensitive_accepted
                .insert(key);
        }
        for deadline in std::mem::take(&mut self.deadline_removals) {
            let shard = entries.owner_shard(&deadline.hash);
            cut.projection_shard_mut(shard).deadlines.remove(&deadline);
        }
        for deadline in std::mem::take(&mut self.deadline_insertions) {
            let shard = entries.owner_shard(&deadline.hash);
            cut.projection_shard_mut(shard).deadlines.insert(deadline);
        }
        for deadline in std::mem::take(&mut self.accepted_deadline_removals) {
            let shard = entries.owner_shard(&deadline.hash);
            cut.projection_shard_mut(shard)
                .accepted_deadlines
                .remove(&deadline);
        }
        for deadline in std::mem::take(&mut self.accepted_deadline_insertions) {
            let shard = entries.owner_shard(&deadline.hash);
            cut.projection_shard_mut(shard)
                .accepted_deadlines
                .insert(deadline);
        }
    }
}
#[cfg(test)]
#[path = "tests/support/indexes.rs"]
pub(in crate::authority) mod test_support;
