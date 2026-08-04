use super::*;

impl PeerBanLease {
    pub(in crate::authority) const fn for_foundation(peer: PeerIndex) -> Self {
        Self {
            peer,
            deadline: PeerBanDeadline::ProcessLifetime,
        }
    }
}

impl PeerBanRegistry {
    pub(in crate::authority) fn snapshot(&self) -> HashMap<PeerIndex, PeerBanDeadline> {
        self.entries.clone()
    }

    pub(in crate::authority) fn semantically_consistent(&self) -> bool {
        let expiring_entries = self
            .entries
            .values()
            .filter(|deadline| matches!(deadline, PeerBanDeadline::At(_)))
            .count();
        expiring_entries == self.expirations.len()
            && self
                .expirations
                .iter()
                .zip(self.expirations.iter().skip(1))
                .all(|((left, _), (right, _))| left <= right)
            && self.expirations.iter().all(|(deadline, peer)| {
                self.entries.get(peer) == Some(&PeerBanDeadline::At(*deadline))
            })
            && self.entries.iter().all(|(peer, deadline)| match deadline {
                PeerBanDeadline::At(deadline) => self
                    .expirations
                    .iter()
                    .any(|candidate| candidate == &(*deadline, *peer)),
                PeerBanDeadline::ProcessLifetime => true,
            })
    }
}
