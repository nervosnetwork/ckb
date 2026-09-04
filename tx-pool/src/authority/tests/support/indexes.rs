use super::*;
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) struct IndexSnapshot {
    pub(in crate::authority) by_proposal: HashMap<ProposalId, RawTxHash>,
    pub(in crate::authority) peer_ingress_rows: HashMap<PeerIndex, HashSet<RawTxHash>>,
    pub(in crate::authority) context_sensitive_accepted: HashSet<RawTxHash>,
    deadlines: BTreeSet<DeadlineKey>,
    accepted_deadlines: BTreeSet<AcceptedDeadlineKey>,
}

impl AuthorityIndexes {
    pub(in crate::authority) fn replace_proposal_owner_for_test(
        &self,
        proposal: &ProposalId,
        owner: Option<RawTxHash>,
    ) -> Option<RawTxHash> {
        let shard = self.proposal_shard(proposal);
        let proposals = &mut self.entries.layout.shards[shard].write().proposals;
        match owner {
            Some(owner) => proposals.insert(proposal.clone(), owner),
            None => proposals.remove(proposal),
        }
    }

    pub(in crate::authority) fn peer_ban_snapshot(
        &self,
    ) -> HashMap<PeerIndex, crate::authority::ban::PeerBanDeadline> {
        let shards: [ckb_util::parking_lot::RwLockReadGuard<
            '_,
            crate::authority::shard::AuthorityShard,
        >; crate::authority::shard::AUTHORITY_SHARD_COUNT] =
            std::array::from_fn(|shard| self.entries.layout.shards[shard].read());
        shards
            .iter()
            .flat_map(|shard| shard.peer_fences.iter())
            .filter_map(|(peer, fence)| {
                fence.logical_lease().map(|lease| (*peer, lease.deadline()))
            })
            .collect()
    }

    pub(in crate::authority) fn snapshot(&self) -> IndexSnapshot {
        let shards: [ckb_util::parking_lot::RwLockReadGuard<
            '_,
            crate::authority::shard::AuthorityShard,
        >; crate::authority::shard::AUTHORITY_SHARD_COUNT] =
            std::array::from_fn(|shard| self.entries.layout.shards[shard].read());
        let mut by_proposal = HashMap::new();
        let mut peer_ingress_rows = HashMap::new();
        let mut context_sensitive_accepted = HashSet::new();
        let mut deadlines = BTreeSet::new();
        let mut accepted_deadlines = BTreeSet::new();
        for shard in &shards {
            by_proposal.extend(
                shard
                    .proposals
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
            context_sensitive_accepted.extend(shard.context_sensitive_accepted.iter().cloned());
            peer_ingress_rows.extend(
                shard
                    .peer_ingress_owners
                    .iter()
                    .filter(|(_, owners)| !owners.is_empty())
                    .map(|(key, owners)| (*key, owners.clone())),
            );
            deadlines.extend(shard.deadlines.iter().cloned());
            accepted_deadlines.extend(shard.accepted_deadlines.iter().cloned());
        }
        IndexSnapshot {
            by_proposal,
            peer_ingress_rows,
            context_sensitive_accepted,
            deadlines,
            accepted_deadlines,
        }
    }

    pub(in crate::authority) fn semantically_matches(
        &self,
        entries: &crate::authority::shard::ShardedOwnerMap,
    ) -> bool {
        let mut expected = IndexSnapshot {
            by_proposal: HashMap::new(),
            peer_ingress_rows: HashMap::new(),
            context_sensitive_accepted: HashSet::new(),
            deadlines: BTreeSet::new(),
            accepted_deadlines: BTreeSet::new(),
        };
        let snapshot = entries.snapshot_for_test();
        for (key, owner) in &snapshot {
            if expected
                .by_proposal
                .insert(owner.record().identity.proposal.clone(), key.clone())
                .is_some()
            {
                return false;
            }
            if let OwnedTx::PreAccepted(entry) = owner
                && let Some(peer) = entry.source.ingress_peer()
            {
                expected
                    .peer_ingress_rows
                    .entry(peer)
                    .or_default()
                    .insert(key.clone());
            }
            if let OwnedTx::PreAccepted(entry) = owner
                && let Some(expires_at) = entry.source.active_remote_deadline()
            {
                expected.deadlines.insert(DeadlineKey {
                    expires_at,
                    hash: key.clone(),
                });
            }
            match owner {
                OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_) => {}
                OwnedTx::Accepted(entry) => {
                    expected.accepted_deadlines.insert(AcceptedDeadlineKey {
                        accepted_at: entry.accepted_at,
                        hash: key.clone(),
                    });
                    if entry.proof.sensitivity().requires_reorg_revalidation() {
                        expected.context_sensitive_accepted.insert(key.clone());
                    }
                }
            }
        }
        self.snapshot() == expected
    }
}
