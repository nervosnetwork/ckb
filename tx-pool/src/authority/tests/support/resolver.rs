use super::*;

impl ResolutionJob {
    pub(in crate::authority) fn retry_for_foundation(self) -> ComputeSettlement {
        self.work.retry()
    }
}

impl VerificationExecution {
    pub(in crate::authority) fn into_parts_for_foundation(
        self,
    ) -> (ComputeSettlement, Option<VerificationCacheUpdate>) {
        match self {
            Self::Settlement {
                settlement,
                cache_update,
            } => (settlement, cache_update),
            Self::Structural {
                settlement: _,
                fault,
            } => panic!("fixture reached a structural verification fault: {fault:?}"),
        }
    }
}

impl DirectVerifiedCandidate {
    pub(in crate::authority) fn with_cache_update_for_foundation(
        mut self,
        key: TxVerificationCacheKey,
        completed: Completed,
    ) -> Self {
        self.cache_update = Some(VerificationCacheUpdate { key, completed });
        self
    }
}

impl DirectResolutionJob {
    fn prepare_transaction_for_foundation(
        tx: Arc<TransactionView>,
        max_resident_bytes: usize,
        max_edges: usize,
    ) -> Result<PreparedDirectResolutionJob, DirectComputationError> {
        let overlay = AcceptedOverlay::prepare(&tx, max_edges).map_err(|kind| match kind {
            ResolutionExecutionKind::ResourceUnavailable => {
                DirectComputationError::ResourceUnavailable
            }
            ResolutionExecutionKind::ComputeBudget
            | ResolutionExecutionKind::StaleView
            | ResolutionExecutionKind::InvalidReceipt(_) => DirectComputationError::InvalidEvidence,
        })?;
        Ok(PreparedDirectResolutionJob {
            tx,
            command: DirectCommand::TestAccept,
            overlay,
            max_resident_bytes,
            max_edges,
        })
    }

    pub(in crate::authority) fn capture_for_foundation(
        authority: &TxPoolAuthority,
        snapshot: Arc<Snapshot>,
        tx: Arc<TransactionView>,
        max_resident_bytes: usize,
        max_edges: usize,
    ) -> Result<Self, DirectComputationError> {
        let mut prepared =
            Self::prepare_transaction_for_foundation(tx, max_resident_bytes, max_edges)?;
        if snapshot.tip_hash() != authority.chain_view().tip().0 {
            return Err(DirectComputationError::StaleView);
        }
        prepared.overlay.populate_initial(authority);
        Ok(Self {
            tx: prepared.tx,
            command: prepared.command,
            view: authority.chain_view().clone(),
            accepted_source: authority.accepted_source_cut(),
            dependency_cut: authority.dependency_observation_cut(),
            snapshot,
            overlay: prepared.overlay,
            max_resident_bytes: prepared.max_resident_bytes,
            max_edges: prepared.max_edges,
        })
    }
}

impl DirectResolutionProbe {
    pub(in crate::authority) fn missing_keys_for_foundation(&self) -> Vec<DependencyKey> {
        self.missing
            .iter()
            .map(|missing| DependencyKey::Cell(missing.out_point.clone()))
            .collect()
    }
}

impl ResolutionProbe {
    pub(in crate::authority) fn missing_keys_for_foundation(&self) -> Vec<DependencyKey> {
        self.missing
            .iter()
            .map(|cell| DependencyKey::Cell(cell.out_point.clone()))
            .collect()
    }
}
