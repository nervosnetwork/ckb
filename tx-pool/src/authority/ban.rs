//! Expiring ingress-peer revocation owned by the transaction-pool authority.
//!
//! This is not a second network ban database. It is the authority-local fence
//! that makes an already queued, checked-out, or Ready `PreAccepted` owner
//! race atomically with the ban transition that removes its complete ingress
//! cohort. The network consumes the committed effect afterwards.

use crate::constants::{MALFORMED_TX_BAN_SECONDS, PEER_BAN_FENCE_CAPACITY};
use ckb_network::PeerIndex;
use ckb_util::parking_lot::Mutex;
use std::time::{Duration, Instant};

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

    pub(super) fn is_active_at(self, now: Instant) -> bool {
        match self {
            Self::At(deadline) => deadline > now,
            Self::ProcessLifetime => true,
        }
    }
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
    pub(super) const fn deadline(self) -> PeerBanDeadline {
        self.deadline
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PeerBanError {
    /// A live staged slot conflicts with this request and may complete or roll
    /// back without rebuilding the authority generation.
    Contention,
    /// The slot bank observed a structural contradiction and is permanently
    /// fail-closed for this authority generation.
    Faulted,
    CounterExhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ActivePeerBanSlot {
    lease: PeerBanLease,
    order_ticket: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PeerBanSlot {
    Free,
    Active(ActivePeerBanSlot),
    Reserved {
        stage_id: u64,
        previous: Option<ActivePeerBanSlot>,
        next: PeerBanLease,
        order_ticket: u64,
        record: bool,
    },
    Committing {
        stage_id: u64,
        previous: Option<ActivePeerBanSlot>,
        next: PeerBanLease,
        order_ticket: u64,
        record: bool,
    },
}

/// Test-only legacy single-delta fixture.
///
/// Production peer revocation uses the staged shared peer-row protocol. This
/// carrier remains only for narrow test construction and has no route or
/// architecture authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg(test)]
pub(super) struct PeerBanDelta {
    slot: usize,
    stage_id: u64,
    lease: PeerBanLease,
    victim: Option<PeerBanLease>,
    order_ticket: u64,
    record: bool,
}

#[cfg(test)]
impl PeerBanDelta {
    pub(super) const fn stage_id(&self) -> u64 {
        self.stage_id
    }

    pub(super) const fn lease(&self) -> PeerBanLease {
        self.lease
    }

    pub(super) const fn victim(&self) -> Option<PeerBanLease> {
        self.victim
    }

    pub(super) const fn records_new(&self) -> bool {
        self.record
    }
}

#[derive(Debug)]
pub(super) struct PeerBanSlotBank {
    state: Mutex<PeerBanSlotState>,
}

#[derive(Debug)]
struct PeerBanSlotState {
    /// Fixed at construction. A reservation owns one exact physical slot, so
    /// reverse completion and rollback never trigger HashMap growth or change
    /// the selected oldest victim after owner mutation.
    slots: Box<[PeerBanSlot]>,
    next_stage_id: u64,
    next_order_ticket: u64,
    faulted: bool,
}

/// Linear reservation of one bounded ban-order slot. It owns capacity and an
/// optional oldest victim, but not the active peer-ban truth; that truth lives
/// in the routed `index/peer` row and becomes visible only in the owner cut.
#[must_use = "a staged peer-ban slot must finish with its peer fence or roll back by Drop"]
pub(super) struct StagedPeerBanSlot<'registry> {
    registry: &'registry PeerBanSlotBank,
    slot: usize,
    stage_id: u64,
    lease: PeerBanLease,
    previous: Option<PeerBanLease>,
    victim: Option<PeerBanLease>,
    order_ticket: u64,
    record: bool,
    finished: bool,
}

#[must_use = "a begun peer-ban slot commit must finish after the owner cut"]
pub(super) struct PeerBanCommitPermit<'registry> {
    registry: &'registry PeerBanSlotBank,
    slot: usize,
    stage_id: u64,
    lease: PeerBanLease,
    previous: Option<PeerBanLease>,
    order_ticket: u64,
    record: bool,
    finished: bool,
}

impl Default for PeerBanSlotBank {
    fn default() -> Self {
        Self {
            state: Mutex::new(PeerBanSlotState {
                slots: vec![PeerBanSlot::Free; PEER_BAN_FENCE_CAPACITY].into_boxed_slice(),
                next_stage_id: 1,
                next_order_ticket: 1,
                faulted: false,
            }),
        }
    }
}

impl PeerBanSlotBank {
    fn select_slot(
        state: &mut PeerBanSlotState,
        peer: PeerIndex,
        observed_at: Instant,
    ) -> Result<(usize, Option<ActivePeerBanSlot>, PeerBanLease, bool), PeerBanError> {
        if state.faulted {
            return Err(PeerBanError::Faulted);
        }
        if state.slots.iter().any(|slot| {
            matches!(
                slot,
                PeerBanSlot::Reserved { previous, next, .. }
                    | PeerBanSlot::Committing { previous, next, .. }
                    if next.peer == peer
                        || previous.is_some_and(|previous| previous.lease.peer == peer)
            )
        }) {
            return Err(PeerBanError::Contention);
        }
        if let Some((index, previous)) =
            state
                .slots
                .iter()
                .enumerate()
                .find_map(|(index, slot)| match slot {
                    PeerBanSlot::Active(previous) if previous.lease.peer == peer => {
                        Some((index, *previous))
                    }
                    PeerBanSlot::Free
                    | PeerBanSlot::Active(_)
                    | PeerBanSlot::Reserved { .. }
                    | PeerBanSlot::Committing { .. } => None,
                })
        {
            let active = previous.lease.deadline.is_active_at(observed_at);
            let next = if active {
                previous.lease
            } else {
                PeerBanLease {
                    peer,
                    deadline: PeerBanDeadline::after_malformed_ban(observed_at),
                }
            };
            return Ok((index, Some(previous), next, !active));
        }
        if let Some((index, previous)) = state
            .slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| match slot {
                PeerBanSlot::Active(previous)
                    if !previous.lease.deadline.is_active_at(observed_at) =>
                {
                    Some((index, *previous))
                }
                PeerBanSlot::Free
                | PeerBanSlot::Active(_)
                | PeerBanSlot::Reserved { .. }
                | PeerBanSlot::Committing { .. } => None,
            })
            .min_by_key(|(_, previous)| previous.order_ticket)
        {
            return Ok((
                index,
                Some(previous),
                PeerBanLease {
                    peer,
                    deadline: PeerBanDeadline::after_malformed_ban(observed_at),
                },
                true,
            ));
        }
        if let Some(index) = state
            .slots
            .iter()
            .position(|slot| matches!(slot, PeerBanSlot::Free))
        {
            return Ok((
                index,
                None,
                PeerBanLease {
                    peer,
                    deadline: PeerBanDeadline::after_malformed_ban(observed_at),
                },
                true,
            ));
        }
        // At a physically full bank, an earlier rollback-capable replacement
        // can change which row is oldest. Only one live-oldest replacement may
        // therefore be staged; free/expired distinct slots remain concurrent.
        if state.slots.iter().any(|slot| {
            matches!(
                slot,
                PeerBanSlot::Reserved { .. } | PeerBanSlot::Committing { .. }
            )
        }) {
            return Err(PeerBanError::Contention);
        }
        let Some(oldest) = state
            .slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| match slot {
                PeerBanSlot::Active(active) => Some((index, *active)),
                PeerBanSlot::Free
                | PeerBanSlot::Reserved { .. }
                | PeerBanSlot::Committing { .. } => None,
            })
            .min_by_key(|(_, active)| active.order_ticket)
        else {
            state.faulted = true;
            return Err(PeerBanError::Faulted);
        };
        Ok((
            oldest.0,
            Some(oldest.1),
            PeerBanLease {
                peer,
                deadline: PeerBanDeadline::after_malformed_ban(observed_at),
            },
            true,
        ))
    }

    fn next_identities(
        state: &mut PeerBanSlotState,
        record: bool,
    ) -> Result<(u64, u64), PeerBanError> {
        let stage_id = state.next_stage_id;
        state.next_stage_id = state
            .next_stage_id
            .checked_add(1)
            .ok_or(PeerBanError::CounterExhausted)?;
        let order_ticket = if record {
            let ticket = state.next_order_ticket;
            state.next_order_ticket = state
                .next_order_ticket
                .checked_add(1)
                .ok_or(PeerBanError::CounterExhausted)?;
            ticket
        } else {
            0
        };
        Ok((stage_id, order_ticket))
    }

    #[cfg(test)]
    pub(super) fn plan_exclusive_record(
        &self,
        peer: PeerIndex,
        observed_at: Instant,
    ) -> Result<PeerBanDelta, PeerBanError> {
        let mut state = self.state.lock();
        if state.faulted {
            return Err(PeerBanError::Faulted);
        }
        if state.slots.iter().any(|slot| {
            matches!(
                slot,
                PeerBanSlot::Reserved { .. } | PeerBanSlot::Committing { .. }
            )
        }) {
            return Err(PeerBanError::Contention);
        }
        let (slot, previous, lease, record) = Self::select_slot(&mut state, peer, observed_at)?;
        let (stage_id, mut order_ticket) = Self::next_identities(&mut state, record)?;
        if !record {
            order_ticket = previous.map_or(0, |previous| previous.order_ticket);
        }
        Ok(PeerBanDelta {
            slot,
            stage_id,
            lease,
            victim: previous
                .map(|previous| previous.lease)
                .filter(|victim| record && victim.peer != peer),
            order_ticket,
            record,
        })
    }

    /// Reserve one exact slot without publishing a ban. Different peers can
    /// reserve concurrently; virtual occupancy includes every staged target,
    /// and an oldest committed victim can be owned by only one reservation.
    pub(super) fn plan_record(
        &self,
        peer: PeerIndex,
        observed_at: Instant,
    ) -> Result<StagedPeerBanSlot<'_>, PeerBanError> {
        let mut state = self.state.lock();
        let (slot, previous, lease, record) = Self::select_slot(&mut state, peer, observed_at)?;
        let (stage_id, mut order_ticket) = Self::next_identities(&mut state, record)?;
        if !record {
            order_ticket = previous.map_or(0, |previous| previous.order_ticket);
        }
        let Some(selected) = state.slots.get_mut(slot) else {
            state.faulted = true;
            return Err(PeerBanError::Faulted);
        };
        *selected = PeerBanSlot::Reserved {
            stage_id,
            previous,
            next: lease,
            order_ticket,
            record,
        };
        let previous_lease = previous.map(|previous| previous.lease);
        Ok(StagedPeerBanSlot {
            registry: self,
            slot,
            stage_id,
            lease,
            previous: previous_lease,
            victim: previous_lease.filter(|victim| record && victim.peer != peer),
            order_ticket,
            record,
            finished: false,
        })
    }

    #[cfg(test)]
    pub(super) fn contains_at(&self, peer: PeerIndex, now: Instant) -> bool {
        self.state
            .lock()
            .slots
            .iter()
            .filter_map(|slot| match slot {
                PeerBanSlot::Active(active) => Some(active.lease),
                PeerBanSlot::Reserved { previous, .. } => previous.map(|previous| previous.lease),
                PeerBanSlot::Free | PeerBanSlot::Committing { previous: None, .. } => None,
                PeerBanSlot::Committing {
                    previous: Some(previous),
                    ..
                } => Some(previous.lease),
            })
            .any(|lease| lease.peer == peer && lease.deadline.is_active_at(now))
    }

    #[cfg(test)]
    pub(super) fn apply(&self, delta: PeerBanDelta) {
        if !delta.record {
            return;
        }
        let mut state = self.state.lock();
        let Some(slot) = state.slots.get_mut(delta.slot) else {
            state.faulted = true;
            return;
        };
        *slot = PeerBanSlot::Active(ActivePeerBanSlot {
            lease: delta.lease,
            order_ticket: delta.order_ticket,
        });
    }
}

impl<'registry> StagedPeerBanSlot<'registry> {
    pub(super) const fn stage_id(&self) -> u64 {
        self.stage_id
    }

    pub(super) const fn lease(&self) -> PeerBanLease {
        self.lease
    }

    /// The physical slot may previously belong to a different peer selected
    /// as the bounded-capacity victim. Only a same-peer slot is the prior
    /// routed truth for the target peer row.
    pub(super) fn target_previous(&self) -> Option<PeerBanLease> {
        self.previous
            .filter(|previous| previous.peer() == self.lease.peer())
    }

    pub(super) const fn victim(&self) -> Option<PeerBanLease> {
        self.victim
    }

    /// Seal the fixed slot immediately before owner mutation. A mismatch is a
    /// structural bank fault and occurs while the caller can still abort.
    #[cfg(test)]
    pub(super) fn begin(mut self) -> Result<PeerBanCommitPermit<'registry>, PeerBanError> {
        self.begin_in_place()
    }

    /// Move the already-reserved physical slot into its commit state without
    /// consuming the rollback owner. A caller which cannot continue retains
    /// `self`, so its Drop still restores the exact prior slot.
    pub(in crate::authority) fn begin_in_place(
        &mut self,
    ) -> Result<PeerBanCommitPermit<'registry>, PeerBanError> {
        let mut state = self.registry.state.lock();
        if state.faulted {
            return Err(PeerBanError::Faulted);
        }
        let previous = match state.slots.get(self.slot).copied() {
            Some(PeerBanSlot::Reserved {
                stage_id,
                previous,
                next,
                order_ticket,
                record,
            }) if stage_id == self.stage_id
                && previous.map(|previous| previous.lease) == self.previous
                && next == self.lease
                && order_ticket == self.order_ticket
                && record == self.record =>
            {
                previous
            }
            None
            | Some(
                PeerBanSlot::Free
                | PeerBanSlot::Active(_)
                | PeerBanSlot::Reserved { .. }
                | PeerBanSlot::Committing { .. },
            ) => {
                state.faulted = true;
                return Err(PeerBanError::Faulted);
            }
        };
        let Some(slot) = state.slots.get_mut(self.slot) else {
            state.faulted = true;
            return Err(PeerBanError::Faulted);
        };
        *slot = PeerBanSlot::Committing {
            stage_id: self.stage_id,
            previous,
            next: self.lease,
            order_ticket: self.order_ticket,
            record: self.record,
        };
        self.finished = true;
        Ok(PeerBanCommitPermit {
            registry: self.registry,
            slot: self.slot,
            stage_id: self.stage_id,
            lease: self.lease,
            previous: self.previous,
            order_ticket: self.order_ticket,
            record: self.record,
            finished: false,
        })
    }

    pub(super) fn mark_faulted(&mut self) {
        self.registry.state.lock().faulted = true;
        self.finished = true;
    }
}

impl Drop for StagedPeerBanSlot<'_> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let mut state = self.registry.state.lock();
        if let Some(PeerBanSlot::Reserved {
            stage_id,
            previous,
            next,
            order_ticket,
            record,
        }) = state.slots.get(self.slot).copied()
            && stage_id == self.stage_id
            && previous.map(|previous| previous.lease) == self.previous
            && next == self.lease
            && order_ticket == self.order_ticket
            && record == self.record
        {
            if let Some(slot) = state.slots.get_mut(self.slot) {
                *slot = previous.map_or(PeerBanSlot::Free, PeerBanSlot::Active);
            } else {
                state.faulted = true;
            }
        } else {
            state.faulted = true;
        }
    }
}

impl PeerBanCommitPermit<'_> {
    pub(super) const fn stage_id(&self) -> u64 {
        self.stage_id
    }

    pub(super) const fn lease(&self) -> PeerBanLease {
        self.lease
    }

    pub(super) fn victim(&self) -> Option<PeerBanLease> {
        self.previous
            .filter(|victim| self.record && victim.peer != self.lease.peer)
    }

    pub(super) fn finish(mut self) {
        let mut state = self.registry.state.lock();
        let previous = match state.slots.get(self.slot).copied() {
            Some(PeerBanSlot::Committing {
                stage_id,
                previous,
                next,
                order_ticket,
                record,
            }) if stage_id == self.stage_id
                && previous.map(|previous| previous.lease) == self.previous
                && next == self.lease
                && order_ticket == self.order_ticket
                && record == self.record =>
            {
                previous
            }
            None
            | Some(
                PeerBanSlot::Free
                | PeerBanSlot::Active(_)
                | PeerBanSlot::Reserved { .. }
                | PeerBanSlot::Committing { .. },
            ) => {
                state.faulted = true;
                return;
            }
        };
        let next = if self.record {
            PeerBanSlot::Active(ActivePeerBanSlot {
                lease: self.lease,
                order_ticket: self.order_ticket,
            })
        } else {
            previous.map_or(PeerBanSlot::Free, PeerBanSlot::Active)
        };
        let Some(slot) = state.slots.get_mut(self.slot) else {
            state.faulted = true;
            return;
        };
        *slot = next;
        self.finished = true;
    }
}

impl Drop for PeerBanCommitPermit<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.registry.state.lock().faulted = true;
        }
    }
}

#[cfg(test)]
#[path = "tests/support/ban.rs"]
mod test_support;
