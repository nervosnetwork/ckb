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
    pub(in crate::authority) fn cursors_for_refinement(
        &self,
    ) -> (Option<WorkOwner>, Option<WorkOwner>) {
        (self.resolve.owner_cursor, self.verify.owner_cursor)
    }

    pub(in crate::authority) fn snapshot(&self) -> SchedulerSnapshot {
        SchedulerSnapshot {
            verify_order: self.verify_order,
            slots: self.slots(),
            resolve_owner_cursor: self.resolve.owner_cursor,
            verify_owner_cursor: self.verify.owner_cursor,
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
                    .cloned()
                    .map(|key| SchedulerSlot::Queue {
                        lane: QueueLane::Verify,
                        owner: *owner,
                        key,
                    }),
            );
        }
        actual.extend(self.ready.iter().cloned().map(SchedulerSlot::Ready));
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

    pub(in crate::authority) fn semantically_matches(
        &self,
        entries: &std::collections::HashMap<RawTxHash, OwnedTx>,
    ) -> bool {
        let Ok(expected) = entries
            .values()
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
