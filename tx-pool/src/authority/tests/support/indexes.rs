use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) struct IndexSnapshot {
    pub(in crate::authority) by_proposal: HashMap<ProposalId, RawTxHash>,
    pub(in crate::authority) preaccepted_by_peer: HashMap<PeerIndex, HashSet<RawTxHash>>,
    pub(in crate::authority) context_sensitive_accepted: HashSet<RawTxHash>,
    deadlines: BTreeSet<DeadlineKey>,
    accepted_deadlines: BTreeSet<AcceptedDeadlineKey>,
}

impl AuthorityIndexes {
    pub(in crate::authority) fn snapshot(&self) -> IndexSnapshot {
        IndexSnapshot {
            by_proposal: self.by_proposal.clone(),
            preaccepted_by_peer: self.preaccepted_by_peer.clone(),
            context_sensitive_accepted: self.context_sensitive_accepted.clone(),
            deadlines: self.deadlines.clone(),
            accepted_deadlines: self.accepted_deadlines.clone(),
        }
    }

    pub(in crate::authority) fn semantically_matches(
        &self,
        entries: &crate::authority::shard::ShardedOwnerMap,
    ) -> bool {
        let mut expected = Self::default();
        for (key, owner) in entries {
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
                    .preaccepted_by_peer
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
        self.by_proposal == expected.by_proposal
            && self.preaccepted_by_peer == expected.preaccepted_by_peer
            && self.context_sensitive_accepted == expected.context_sensitive_accepted
            && self.deadlines == expected.deadlines
            && self.accepted_deadlines == expected.accepted_deadlines
    }
}
