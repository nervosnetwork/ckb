use super::*;

impl<R, U, V> PipelineCoordinator<R, U, V> {
    /// Replace raw work with an unverified phase bundle. `charge_bytes` is
    /// the total payload residency of that entire bundle, including the raw
    /// transaction retained for dependency demotion and terminal handoff.
    pub(crate) fn complete_raw(
        &mut self,
        lease: &RawWorkLease<R>,
        unverified: U,
        charge_bytes: usize,
        verify_schedule: VerifySchedule,
    ) -> Result<(CoordinatorVersion, Vec<TerminalRecord<R>>), CoordinatorError> {
        self.complete_raw_with_dependencies(
            lease,
            unverified,
            charge_bytes,
            verify_schedule,
            HashSet::new(),
        )
    }

    pub(crate) fn parent_unavailable(
        &mut self,
        parent: &Byte32,
    ) -> Result<Vec<Byte32>, CoordinatorError> {
        self.parents_unavailable(&HashSet::from([parent.clone()]))
    }

    /// Make every direct dependent fail-closed immediately, then defer the
    /// transitive terminal cascade to bounded maintenance slices.
    pub(crate) fn schedule_parent_failure(
        &mut self,
        parent: &Byte32,
    ) -> Result<Vec<Byte32>, CoordinatorError> {
        self.mark_children_invalid(parent, parent)
    }

    /// Test convenience wrapper around the only production verified state.
    /// A unique synthetic input keeps generic state-machine tests on the same
    /// candidate/index path as real transactions without creating conflicts.
    pub(crate) fn complete_verification(
        &mut self,
        lease: &VerifyWorkLease<U>,
        verified: V,
        charge_bytes: usize,
    ) -> Result<(CoordinatorVersion, Vec<TerminalRecord<R>>), CoordinatorError> {
        let candidate = VerifiedCandidate {
            inputs: HashSet::from([OutPoint::new(lease.hash.clone(), 0)]),
            fee: 0,
            tx_size: 1,
        };
        self.complete_verification_candidate(lease, verified, charge_bytes, candidate)
    }
}
