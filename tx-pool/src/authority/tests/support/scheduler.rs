use super::super::exchange::ComputeWorkerSlot;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::authority) enum SchedulerSetStageObservation {
    Resolve(WorkOwner),
    Verify(WorkOwner, VerifyCycleClass),
    Ready,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::authority) struct SchedulerSetMemberObservation {
    pub(in crate::authority) hash: RawTxHash,
    pub(in crate::authority) version: EntryVersion,
    pub(in crate::authority) stage: SchedulerSetStageObservation,
}

#[derive(Debug, PartialEq, Eq)]
pub(in crate::authority) struct SchedulerWaveObservation {
    pub(in crate::authority) assignments: Vec<(
        super::super::exchange::ComputeWorkerSlot,
        super::super::state::WorkPermit,
        RawTxHash,
        EntryVersion,
    )>,
    pub(in crate::authority) idle: Vec<super::super::exchange::ComputeWorkerSlot>,
    pub(in crate::authority) cursors: (Option<WorkOwner>, Option<WorkOwner>),
}

/// Test-only sequential scheduler assignment. Production uses
/// `SchedulerExchangeWave`, whose selections are compiled directly into the
/// authoritative compute exchange.
struct SchedulerWaveAssignment {
    slot: ComputeWorkerSlot,
    permit: super::super::state::WorkPermit,
    ticket: CheckoutTicket,
}

impl SchedulerWaveAssignment {
    fn slot(&self) -> ComputeWorkerSlot {
        self.slot
    }

    fn permit(&self) -> super::super::state::WorkPermit {
        self.permit
    }

    fn ticket(&self) -> &CheckoutTicket {
        &self.ticket
    }
}

#[must_use = "the sequential scheduler oracle must be observed or dropped unchanged"]
struct SchedulerWavePlan {
    cursor: SchedulerWaveCursor,
    assignments: Vec<SchedulerWaveAssignment>,
    idle: Vec<ComputeWorkerSlot>,
}

impl SchedulerWavePlan {
    fn into_parts(
        self,
    ) -> (
        SchedulerWaveCursor,
        Vec<SchedulerWaveAssignment>,
        Vec<ComputeWorkerSlot>,
    ) {
        (self.cursor, self.assignments, self.idle)
    }
}

impl FairLane {
    fn next_after_excluding_for_reference(
        &self,
        lane: QueueLane,
        capability: VerifyCapability,
        cursor: Option<WorkOwner>,
        excluded_versions: &[EntryVersion],
    ) -> Option<(WorkOwner, &QueueKey)> {
        let mut cursor = cursor;
        for _ in 0..self.owner_count(lane, capability) {
            let owner = self.next_owner(lane, capability, cursor)?;
            if let Some(key) = self
                .by_owner
                .get(&owner)
                .and_then(|entries| entries.head_excluding(lane, capability, excluded_versions))
            {
                return Some((owner, key));
            }
            cursor = Some(owner);
        }
        None
    }

    fn next_excluding_for_reference(
        &self,
        lane: QueueLane,
        capability: VerifyCapability,
        cursor: Option<WorkOwner>,
        excluded_versions: &[EntryVersion],
    ) -> Option<(WorkOwner, &QueueKey)> {
        if cursor.is_none()
            && self.owner_is_eligible(lane, capability, WorkOwner::Trusted)
            && let Some(key) = self
                .by_owner
                .get(&WorkOwner::Trusted)
                .and_then(|entries| entries.head_excluding(lane, capability, excluded_versions))
        {
            return Some((WorkOwner::Trusted, key));
        }
        self.next_after_excluding_for_reference(lane, capability, cursor, excluded_versions)
    }

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
    pub(in crate::authority) fn next_queued_in_wave_for_reference(
        &self,
        wave: &SchedulerWaveCursor,
        permit: super::super::state::WorkPermit,
    ) -> Option<CheckoutTicket> {
        let lane = QueueLane::for_permit(permit);
        let capability = QueueLane::capability(permit);
        let frontier = match lane {
            QueueLane::Resolve => &self.resolve,
            QueueLane::Verify => &self.verify,
        };
        frontier
            .next_excluding_for_reference(
                lane,
                capability,
                wave.lane_cursor(lane),
                &wave.selected_versions,
            )
            .map(|(owner, key)| CheckoutTicket {
                lane,
                owner,
                key: key.clone(),
            })
    }

    pub(in crate::authority) fn next_queued_after_in_wave_for_reference(
        &self,
        wave: &SchedulerWaveCursor,
        permit: super::super::state::WorkPermit,
        cursor: WorkOwner,
    ) -> Option<CheckoutTicket> {
        let lane = QueueLane::for_permit(permit);
        let capability = QueueLane::capability(permit);
        let frontier = match lane {
            QueueLane::Resolve => &self.resolve,
            QueueLane::Verify => &self.verify,
        };
        frontier
            .next_after_excluding_for_reference(
                lane,
                capability,
                Some(cursor),
                &wave.selected_versions,
            )
            .map(|(owner, key)| CheckoutTicket {
                lane,
                owner,
                key: key.clone(),
            })
    }

    pub(in crate::authority) fn owner_count_for_reference(
        &self,
        permit: super::super::state::WorkPermit,
    ) -> usize {
        let lane = QueueLane::for_permit(permit);
        let capability = QueueLane::capability(permit);
        match lane {
            QueueLane::Resolve => self.resolve.owner_count(lane, capability),
            QueueLane::Verify => self.verify.owner_count(lane, capability),
        }
    }

    fn plan_worker_wave(
        &self,
        slots: &[ComputeWorkerSlot],
    ) -> Result<SchedulerWavePlan, SchedulerError> {
        if slots.len() > crate::constants::MAX_POOL_MUTATION_CANDIDATES {
            return Err(SchedulerError::Projection);
        }
        let mut ordered = Vec::new();
        ordered
            .try_reserve(slots.len())
            .map_err(|_| SchedulerError::Allocation)?;
        ordered.extend_from_slice(slots);
        ordered.sort_unstable_by_key(|slot| slot.id());
        if ordered
            .windows(2)
            .any(|pair| matches!(pair, [left, right] if left.id() == right.id()))
        {
            return Err(SchedulerError::Projection);
        }
        ordered.sort_unstable_by_key(|slot| slot.canonical_key());

        let mut assignments = Vec::new();
        assignments
            .try_reserve(ordered.len())
            .map_err(|_| SchedulerError::Allocation)?;
        let mut idle = Vec::new();
        idle.try_reserve(ordered.len())
            .map_err(|_| SchedulerError::Allocation)?;
        let mut cursor = self.checkout_wave(ordered.len())?;
        for slot in ordered {
            let primary = slot.primary_permit();
            let selected = self
                .next_queued_in_wave_for_reference(&cursor, primary)
                .map(|ticket| (primary, ticket))
                .or_else(|| {
                    slot.fallback_permit().and_then(|fallback| {
                        self.next_queued_in_wave_for_reference(&cursor, fallback)
                            .map(|ticket| (fallback, ticket))
                    })
                });
            let Some((permit, ticket)) = selected else {
                idle.push(slot);
                continue;
            };
            cursor.select(&ticket)?;
            assignments.push(SchedulerWaveAssignment {
                slot,
                permit,
                ticket,
            });
        }
        Ok(SchedulerWavePlan {
            cursor,
            assignments,
            idle,
        })
    }

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

    /// Observe the identities and stages resident in the derived scheduler
    /// sets without re-running the production owner-to-slot projection.
    pub(in crate::authority) fn stored_set_observation(
        &self,
    ) -> BTreeSet<SchedulerSetMemberObservation> {
        self.slots()
            .into_iter()
            .map(|slot| match slot {
                SchedulerSlot::Queue {
                    owner,
                    key: QueueKey::Resolve(key),
                    ..
                } => SchedulerSetMemberObservation {
                    hash: key.hash,
                    version: key.version,
                    stage: SchedulerSetStageObservation::Resolve(owner),
                },
                SchedulerSlot::Queue {
                    owner,
                    key: QueueKey::Verify(key),
                    ..
                } => SchedulerSetMemberObservation {
                    hash: key.hash,
                    version: key.version,
                    stage: SchedulerSetStageObservation::Verify(owner, key.class),
                },
                SchedulerSlot::Ready(key) => SchedulerSetMemberObservation {
                    hash: key.hash,
                    version: key.version,
                    stage: SchedulerSetStageObservation::Ready,
                },
            })
            .collect()
    }

    pub(in crate::authority) fn worker_wave_observation(
        &self,
        slots: &[super::super::exchange::ComputeWorkerSlot],
    ) -> Result<SchedulerWaveObservation, SchedulerError> {
        let wave = self.plan_worker_wave(slots)?;
        let (cursor, assignments, idle) = wave.into_parts();
        Ok(SchedulerWaveObservation {
            assignments: assignments
                .into_iter()
                .map(|assignment| {
                    (
                        assignment.slot(),
                        assignment.permit(),
                        assignment.ticket().hash().clone(),
                        assignment.ticket().version(),
                    )
                })
                .collect(),
            idle,
            cursors: (cursor.resolve_cursor, cursor.verify_cursor),
        })
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
