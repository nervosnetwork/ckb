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
    pub(in crate::authority) fn with_limit_for_test(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    pub(in crate::authority) fn snapshot(&self) -> HashMap<PeerIndex, PeerBanDeadline> {
        self.entries.clone()
    }

    pub(in crate::authority) fn semantically_consistent(&self) -> bool {
        self.capacity != 0
            && self.entries.len() <= self.capacity
            && self.entries.len() == self.order.len()
            && self
                .order
                .iter()
                .all(|(deadline, peer)| self.entries.get(peer) == Some(deadline))
            && self.entries.iter().all(|(peer, deadline)| match deadline {
                PeerBanDeadline::At(_) | PeerBanDeadline::ProcessLifetime => self
                    .order
                    .iter()
                    .any(|candidate| candidate == &(*deadline, *peer)),
            })
    }
}
