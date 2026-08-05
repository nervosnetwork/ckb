//! Expiring ingress-peer revocation owned by the transaction-pool authority.
//!
//! This is not a second network ban database. It is the authority-local fence
//! that makes an already queued, checked-out, or Ready `PreAccepted` owner
//! race atomically with the ban transition that removes its complete ingress
//! cohort. The network consumes the committed effect afterwards.

use crate::constants::{MALFORMED_TX_BAN_SECONDS, PEER_BAN_FENCE_CAPACITY};
use ckb_network::PeerIndex;
use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PeerBanDeadline {
    At(Instant),
    ProcessLifetime,
}

impl PeerBanDeadline {
    fn after_malformed_ban(now: Instant) -> Self {
        now.checked_add(Duration::from_secs(MALFORMED_TX_BAN_SECONDS))
            .map_or(Self::ProcessLifetime, Self::At)
    }

    fn is_active_at(self, now: Instant) -> bool {
        match self {
            Self::At(deadline) => deadline > now,
            Self::ProcessLifetime => true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PeerBanDelta {
    lease: PeerBanLease,
    observed_at: Instant,
    record: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PeerBanLease {
    peer: PeerIndex,
    deadline: PeerBanDeadline,
}

impl PeerBanLease {
    pub(super) const fn peer(self) -> PeerIndex {
        self.peer
    }

    /// Remaining external network-ban duration for the same authority lease.
    /// Expired committed work still publishes its filter reset and diagnostic,
    /// but must not start a fresh three-day network ban after the authority
    /// fence has already elapsed.
    pub(super) fn remaining_at(self, now: Instant) -> Option<Duration> {
        match self.deadline {
            PeerBanDeadline::At(deadline) => deadline.checked_duration_since(now),
            PeerBanDeadline::ProcessLifetime => Some(Duration::from_secs(MALFORMED_TX_BAN_SECONDS)),
        }
        .filter(|duration| !duration.is_zero())
    }
}

impl PeerBanDelta {
    pub(super) const fn lease(&self) -> PeerBanLease {
        self.lease
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PeerBanError {
    Allocation,
}

#[derive(Debug)]
pub(super) struct PeerBanRegistry {
    entries: HashMap<PeerIndex, PeerBanDeadline>,
    /// One row per peer in first-ban order. Fixed-duration deadlines are also
    /// expiry ordered; process-lifetime overflow fallbacks remain at the tail.
    /// A live marker is never extended, so the queue has no stale duplicates.
    order: VecDeque<(PeerBanDeadline, PeerIndex)>,
    capacity: usize,
}

impl Default for PeerBanRegistry {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            capacity: PEER_BAN_FENCE_CAPACITY,
        }
    }
}

impl PeerBanRegistry {
    /// Reserve physical map capacity and seal the exact semantic mutation.
    /// Expiry pruning and marker publication happen only in `apply`.
    pub(super) fn plan_record(
        &mut self,
        peer: PeerIndex,
        observed_at: Instant,
    ) -> Result<PeerBanDelta, PeerBanError> {
        if let Some(deadline) = self
            .entries
            .get(&peer)
            .copied()
            .filter(|deadline| deadline.is_active_at(observed_at))
        {
            return Ok(PeerBanDelta {
                lease: PeerBanLease { peer, deadline },
                observed_at,
                record: false,
            });
        }

        // At the hard bound Apply retires one existing row first, so both
        // collections reuse their already-owned slots. Below the bound the
        // physical growth is reserved while Plan still owns the exact input.
        if self.entries.len() < self.capacity {
            self.entries
                .try_reserve(1)
                .map_err(|_| PeerBanError::Allocation)?;
            self.order
                .try_reserve(1)
                .map_err(|_| PeerBanError::Allocation)?;
        }
        let deadline = PeerBanDeadline::after_malformed_ban(observed_at);
        Ok(PeerBanDelta {
            lease: PeerBanLease { peer, deadline },
            observed_at,
            record: true,
        })
    }

    pub(super) fn apply(&mut self, delta: PeerBanDelta) {
        while let Some(&(deadline, peer)) = self.order.front() {
            if deadline.is_active_at(delta.observed_at) {
                break;
            }
            self.order.pop_front();
            if self.entries.get(&peer) == Some(&deadline) {
                self.entries.remove(&peer);
            }
        }
        if !delta.record {
            return;
        }
        while self.entries.len() >= self.capacity {
            let Some((deadline, peer)) = self.order.pop_front() else {
                // The queue is derived solely from `entries`. If a programmer
                // defect ever desynchronizes it, preserve the hard security
                // bound and degrade to an empty fence set instead of panicking
                // or retaining attacker-controlled memory.
                self.entries.clear();
                break;
            };
            if self.entries.get(&peer) == Some(&deadline) {
                self.entries.remove(&peer);
            }
        }
        self.entries.insert(delta.lease.peer, delta.lease.deadline);
        self.order
            .push_back((delta.lease.deadline, delta.lease.peer));
    }

    pub(super) fn contains_at(&self, peer: PeerIndex, now: Instant) -> bool {
        self.entries
            .get(&peer)
            .is_some_and(|deadline| deadline.is_active_at(now))
    }
}

#[cfg(test)]
#[path = "tests/support/ban.rs"]
mod test_support;
