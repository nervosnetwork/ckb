use super::*;

impl<R, U, V> PipelineCoordinator<R, U, V> {
    pub(crate) fn remove_peer_membership_for_test(&mut self, peer: PeerIndex, hash: &Byte32) {
        self.by_peer.get_mut(&peer).unwrap().remove(hash);
    }

    pub(crate) fn parent_available(
        &mut self,
        parent: &Byte32,
    ) -> Result<Vec<CoordinatorTicket>, CoordinatorError> {
        let undo: Vec<_> = self
            .by_parent
            .get(parent)
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        self.with_entry_undo(&undo, |coordinator| {
            coordinator.parent_available_apply(parent)
        })
    }

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
        let children: Vec<_> = self
            .by_parent
            .get(parent)
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        let undo = self.conflict_undo_hashes(&children);
        self.with_entry_undo(&undo, |coordinator| {
            coordinator.mark_children_invalid(parent, parent)
        })
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
