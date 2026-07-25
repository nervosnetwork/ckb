use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CoordinatorAuditError {
    GlobalUsage,
    PeerUsage,
    ShortIdIndex,
    PeerIndex,
    ParentIndex,
    WaitingParentCount,
    QueueLogicalIndex,
    QueuePhysicalIndex,
    ConflictEdgeCount,
    ConflictCandidateIndex,
    ConflictCohortIndex,
    ConflictRelationIndex,
    DeadlineIndex,
    StateInvariant(Byte32),
    MetadataCharge,
    ActiveWork,
    DependencyMaintenanceIndex,
    VictimPriorityIndex,
    EntryTransactionDepth,
    BudgetExceeded,
}

impl CoordinatorLimits {
    pub(crate) const fn with_capacity_reconciliation_limits(
        mut self,
        max_dependency_ancestors: usize,
        max_capacity_evictions_per_transition: usize,
    ) -> Self {
        self.max_dependency_ancestors = max_dependency_ancestors;
        self.max_capacity_evictions_per_transition = max_capacity_evictions_per_transition;
        self
    }
}

impl TicketQueue {
    pub(in crate::component::pipeline_coordinator) fn physical_len(&self) -> usize {
        self.physical_len
    }

    pub(in crate::component::pipeline_coordinator) fn take_selection_probes(&mut self) -> usize {
        std::mem::take(&mut self.selection_probes)
    }

    pub(in crate::component::pipeline_coordinator) fn tickets(
        &self,
    ) -> impl Iterator<Item = &CoordinatorTicket> {
        self.owners.values().flat_map(|owner| {
            owner
                .small
                .iter()
                .chain(owner.large.iter())
                .map(|ranked| &ranked.ticket)
        })
    }

    pub(in crate::component::pipeline_coordinator) fn structure_valid(&self) -> bool {
        let physical_len = self
            .owners
            .values()
            .map(|owner| owner.small.len().saturating_add(owner.large.len()))
            .sum::<usize>();
        let head_limit = self.owners.len().saturating_mul(2).saturating_add(64);
        physical_len == self.physical_len
            && self.heads_any.len() <= head_limit
            && self.heads_small.len() <= head_limit
            && self.live.iter().all(|ticket| {
                self.tickets()
                    .filter(|physical| *physical == ticket)
                    .count()
                    == 1
            })
            && self.owners.iter().all(|(owner_key, owner)| {
                let small_live = self
                    .live
                    .iter()
                    .filter(|ticket| {
                        ticket.owner == *owner_key && !ticket.verify_schedule.is_large_cycle
                    })
                    .count();
                let large_live = self
                    .live
                    .iter()
                    .filter(|ticket| {
                        ticket.owner == *owner_key && ticket.verify_schedule.is_large_cycle
                    })
                    .count();
                owner.reserved_len() == 0
                    && owner.small_live == small_live
                    && owner.large_live == large_live
                    && owner.published_small.as_ref().map(|head| &head.ranked)
                        == owner
                            .small
                            .iter()
                            .filter(|ranked| self.live.contains(&ranked.ticket))
                            .max()
                    && owner.published_any.as_ref().map(|head| &head.ranked)
                        == owner
                            .small
                            .iter()
                            .chain(owner.large.iter())
                            .filter(|ranked| self.live.contains(&ranked.ticket))
                            .max()
                    && owner.published_any.as_ref().is_none_or(|head| {
                        self.heads_any
                            .iter()
                            .filter(|physical| *physical == head)
                            .count()
                            == 1
                    })
                    && owner.published_small.as_ref().is_none_or(|head| {
                        self.heads_small
                            .iter()
                            .filter(|physical| *physical == head)
                            .count()
                            == 1
                    })
            })
    }
}
