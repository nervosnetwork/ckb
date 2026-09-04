use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) struct SchedulerSnapshot {
    verify_order: VerifyOrder,
    slots: BTreeSet<SchedulerSlot>,
    resolve_owner_cursor: Option<WorkOwner>,
    verify_owner_cursor: Option<WorkOwner>,
    resolve_small_owners: BTreeSet<WorkOwner>,
    verify_small_owners: BTreeSet<WorkOwner>,
}

impl FairLane {
    fn secondary_index_consistent(&self) -> bool {
        self.small_owners
            == self
                .by_owner
                .iter()
                .filter_map(|(owner, queue)| (!queue.small.is_empty()).then_some(*owner))
                .collect()
    }
}

impl FairFrontier {
    pub(in crate::authority) fn ready_physical_counts_for_foundation(
        &self,
    ) -> (usize, usize, usize) {
        (
            self.ready.len(),
            self.ready_reserved.len(),
            self.ready_slot_claim_count(),
        )
    }

    pub(in crate::authority) fn ready_reserved_len_for_foundation(&self) -> usize {
        self.ready_reserved
            .iter()
            .filter(|(_, entry)| {
                !entry.claim().is_some_and(|claim| {
                    matches!(
                        claim.state(),
                        READY_SLOT_COMMITTED | READY_SLOT_RETIRED | READY_SLOT_POISONED
                    )
                })
            })
            .count()
    }

    pub(in crate::authority) fn snapshot(&self) -> SchedulerSnapshot {
        SchedulerSnapshot {
            verify_order: self.verify_order,
            slots: self.slots(),
            resolve_owner_cursor: self.resolve.owner_cursor.map(|cursor| cursor.owner),
            verify_owner_cursor: self.verify.owner_cursor.map(|cursor| cursor.owner),
            resolve_small_owners: self.resolve.small_owners.clone(),
            verify_small_owners: self.verify.small_owners.clone(),
        }
    }

    fn slots(&self) -> BTreeSet<SchedulerSlot> {
        let mut actual = BTreeSet::new();
        for (owner, entries) in &self.resolve.by_owner {
            actual.extend(
                entries
                    .small
                    .iter()
                    .chain(&entries.large)
                    .filter(|key| {
                        self.staged_queue_marker_for(key)
                            .is_none_or(StagedSchedulerMarker::logical_is_visible)
                    })
                    .cloned()
                    .map(|key| SchedulerSlot::Queue {
                        lane: QueueLane::Resolve,
                        owner: *owner,
                        key,
                    }),
            );
        }
        for (owner, entries) in &self.verify.by_owner {
            actual.extend(
                entries
                    .small
                    .iter()
                    .chain(&entries.large)
                    .filter(|key| {
                        self.staged_queue_marker_for(key)
                            .is_none_or(StagedSchedulerMarker::logical_is_visible)
                    })
                    .cloned()
                    .map(|key| SchedulerSlot::Queue {
                        lane: QueueLane::Verify,
                        owner: *owner,
                        key,
                    }),
            );
        }
        actual.extend(
            self.ready
                .iter()
                .filter(|key| self.logical_ready_contains(key))
                .cloned()
                .map(SchedulerSlot::Ready),
        );
        actual
    }

    pub(in crate::authority) fn ticket_for_foundation(
        &self,
        hash: &RawTxHash,
        version: EntryVersion,
        permit: super::super::state::WorkPermit,
    ) -> Option<CheckoutTicket> {
        let expected_lane = QueueLane::for_permit(permit);
        self.slots().into_iter().find_map(|slot| match slot {
            SchedulerSlot::Queue { lane, owner, key }
                if lane == expected_lane && key.hash() == hash && key.version() == version =>
            {
                Some(CheckoutTicket { lane, owner, key })
            }
            SchedulerSlot::Queue { .. } | SchedulerSlot::Ready(_) => None,
        })
    }

    pub(in crate::authority) fn semantically_matches_snapshot(
        &self,
        entries: &[(RawTxHash, OwnedTx)],
    ) -> bool {
        let Ok(expected) = entries
            .iter()
            .map(|(_, owner)| owner)
            .map(|owner| self.slot(owner))
            .collect::<Result<Vec<_>, _>>()
        else {
            return false;
        };
        let expected = expected.into_iter().flatten().collect::<BTreeSet<_>>();
        self.resolve.secondary_index_consistent()
            && self.verify.secondary_index_consistent()
            && self.slots() == expected
    }
}
