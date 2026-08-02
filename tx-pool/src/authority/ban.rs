//! Expiring ingress-peer revocation owned by the transaction-pool authority.
//!
//! This is not a second network ban database. It is the authority-local fence
//! that makes an already queued, checked-out, or Ready `PreAccepted` owner
//! race atomically with the ban transition that removes its complete ingress
//! cohort. The network consumes the committed effect afterwards.

use crate::constants::MALFORMED_TX_BAN_SECONDS;
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
    expiration: Option<Instant>,
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

    #[cfg(test)]
    pub(super) const fn for_foundation(peer: PeerIndex) -> Self {
        Self {
            peer,
            deadline: PeerBanDeadline::ProcessLifetime,
        }
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

#[derive(Debug, Default)]
pub(super) struct PeerBanRegistry {
    entries: HashMap<PeerIndex, PeerBanDeadline>,
    /// One deadline per expiring peer, ordered because every first ban uses
    /// the same fixed duration and `Instant` observations are monotonic.
    /// A live marker is never extended, so the queue has no stale duplicates.
    expirations: VecDeque<(Instant, PeerIndex)>,
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
                expiration: None,
            });
        }

        self.entries
            .try_reserve(1)
            .map_err(|_| PeerBanError::Allocation)?;
        let deadline = PeerBanDeadline::after_malformed_ban(observed_at);
        let expiration = match deadline {
            PeerBanDeadline::At(deadline) => {
                self.expirations
                    .try_reserve(1)
                    .map_err(|_| PeerBanError::Allocation)?;
                Some(deadline)
            }
            PeerBanDeadline::ProcessLifetime => None,
        };
        Ok(PeerBanDelta {
            lease: PeerBanLease { peer, deadline },
            observed_at,
            expiration,
        })
    }

    pub(super) fn apply(&mut self, delta: PeerBanDelta) {
        while let Some(&(deadline, peer)) = self.expirations.front() {
            if deadline > delta.observed_at {
                break;
            }
            self.expirations.pop_front();
            if self.entries.get(&peer) == Some(&PeerBanDeadline::At(deadline)) {
                self.entries.remove(&peer);
            }
        }
        self.entries.insert(delta.lease.peer, delta.lease.deadline);
        if let Some(deadline) = delta.expiration {
            self.expirations.push_back((deadline, delta.lease.peer));
        }
    }

    pub(super) fn contains_at(&self, peer: PeerIndex, now: Instant) -> bool {
        self.entries
            .get(&peer)
            .is_some_and(|deadline| deadline.is_active_at(now))
    }

    #[cfg(test)]
    pub(super) fn snapshot(&self) -> HashMap<PeerIndex, PeerBanDeadline> {
        self.entries.clone()
    }

    #[cfg(test)]
    pub(super) fn semantically_consistent(&self) -> bool {
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
