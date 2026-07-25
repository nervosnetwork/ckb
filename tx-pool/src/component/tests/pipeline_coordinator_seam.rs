use super::*;

impl<R, U, V> PipelineCoordinator<R, U, V> {
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn usage(&self) -> CoordinatorResidency {
        self.global_usage
    }

    pub(crate) fn active_conflict_owner(&self, input: &OutPoint) -> Option<&Byte32> {
        self.conflicts.by_input.get(input).and_then(|candidates| {
            candidates
                .iter()
                .filter_map(|hash| self.candidate_rank(hash).ok().map(|rank| (hash, rank)))
                .max_by(|(_, left), (_, right)| left.cmp(right))
                .map(|(hash, _)| hash)
        })
    }

    pub(crate) fn conflict_edge_count(&self) -> usize {
        self.conflicts.input_memberships
    }

    pub(crate) fn deadline_len(&self) -> usize {
        self.live_deadlines.len()
    }

    pub(crate) fn active_work(&self) -> usize {
        self.active_work
    }
}
