use super::*;
use std::collections::HashMap;

impl PeerBanLease {
    pub(in crate::authority) const fn for_foundation(peer: PeerIndex) -> Self {
        Self {
            peer,
            deadline: PeerBanDeadline::ProcessLifetime,
        }
    }
}

impl PeerBanSlotBank {
    pub(in crate::authority) fn with_limit_for_test(capacity: usize) -> Self {
        Self {
            state: Mutex::new(PeerBanSlotState {
                slots: vec![PeerBanSlot::Free; capacity.max(1)].into_boxed_slice(),
                next_stage_id: 1,
                next_order_ticket: 1,
                faulted: false,
            }),
        }
    }

    pub(in crate::authority) fn snapshot(&self) -> HashMap<PeerIndex, PeerBanDeadline> {
        self.state
            .lock()
            .slots
            .iter()
            .filter_map(|slot| match slot {
                PeerBanSlot::Active(active) => Some((active.lease.peer, active.lease.deadline)),
                PeerBanSlot::Reserved {
                    previous: Some(previous),
                    ..
                }
                | PeerBanSlot::Committing {
                    previous: Some(previous),
                    ..
                } => Some((previous.lease.peer, previous.lease.deadline)),
                PeerBanSlot::Free
                | PeerBanSlot::Reserved { previous: None, .. }
                | PeerBanSlot::Committing { previous: None, .. } => None,
            })
            .collect()
    }

    pub(in crate::authority) fn semantically_consistent(&self) -> bool {
        let state = self.state.lock();
        if state.faulted || state.slots.is_empty() {
            return false;
        }
        let mut peers = std::collections::HashSet::new();
        let mut orders = std::collections::HashSet::new();
        state.slots.iter().all(|slot| match slot {
            PeerBanSlot::Free => true,
            PeerBanSlot::Active(active) => {
                peers.insert(active.lease.peer) && orders.insert(active.order_ticket)
            }
            PeerBanSlot::Reserved {
                previous,
                next,
                order_ticket,
                record,
                ..
            }
            | PeerBanSlot::Committing {
                previous,
                next,
                order_ticket,
                record,
                ..
            } => {
                if *record {
                    previous.is_none_or(|previous| peers.insert(previous.lease.peer))
                        && peers.insert(next.peer)
                        && orders.insert(*order_ticket)
                } else {
                    previous.is_some_and(|previous| {
                        previous.lease == *next
                            && previous.order_ticket == *order_ticket
                            && peers.insert(previous.lease.peer)
                            && orders.insert(previous.order_ticket)
                    })
                }
            }
        })
    }

    pub(in crate::authority) fn invalidate_reserved_stage_for_test(&self, stage_id: u64) -> bool {
        let mut state = self.state.lock();
        let Some((index, previous)) =
            state
                .slots
                .iter()
                .enumerate()
                .find_map(|(index, slot)| match slot {
                    PeerBanSlot::Reserved {
                        stage_id: candidate,
                        previous,
                        ..
                    } if *candidate == stage_id => Some((index, *previous)),
                    PeerBanSlot::Free
                    | PeerBanSlot::Active(_)
                    | PeerBanSlot::Reserved { .. }
                    | PeerBanSlot::Committing { .. } => None,
                })
        else {
            return false;
        };
        state.slots[index] = previous.map_or(PeerBanSlot::Free, PeerBanSlot::Active);
        true
    }
}
