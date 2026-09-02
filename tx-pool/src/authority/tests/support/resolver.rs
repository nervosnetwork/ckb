use super::*;
use crate::authority::state::VerifiedFacts;

impl AcceptedOverlay {
    pub(in crate::authority) fn capture_for_foundation(
        authority: &TxPoolAuthority,
        tx: &TransactionView,
        max_edges: usize,
    ) -> Result<Self, DirectComputationError> {
        Self::capture(authority, tx, max_edges).map_err(|kind| match kind {
            ResolutionExecutionKind::ResourceUnavailable => {
                DirectComputationError::ResourceUnavailable
            }
            ResolutionExecutionKind::StaleView
            | ResolutionExecutionKind::ComputeBudget
            | ResolutionExecutionKind::InvalidReceipt(_) => DirectComputationError::InvalidEvidence,
        })
    }
}

impl ResolutionJob {
    pub(in crate::authority) fn retry_for_foundation(self) -> ComputeSettlement {
        self.retry()
    }

    pub(in crate::authority) fn missing_for_foundation(
        self,
        keys: Vec<DependencyKey>,
    ) -> ComputeSettlement {
        self.work
            .missing(keys)
            .expect("the foundation missing frontier is non-empty and bounded")
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
        prepared.overlay.populate(authority);
        Ok(Self {
            tx: prepared.tx,
            command: prepared.command,
            view: authority.chain_view().clone(),
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

impl DirectVerifiedCandidate {
    pub(in crate::authority) fn local_for_foundation(
        tx: Arc<TransactionView>,
        verified: VerifiedFacts,
    ) -> Self {
        Self {
            command: DirectCommand::Local,
            work: DirectAdmissionWork::new(tx, verified)
                .expect("foundation transaction and verified identity agree"),
            cache_update: None,
        }
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
