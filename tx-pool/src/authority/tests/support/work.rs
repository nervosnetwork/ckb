use super::super::state::{TxIdentity, test_support::FoundationResolution};
use super::*;
use ckb_verification::cache::ScriptVerificationRules;

fn payload_matches(tx: &TransactionView, payload: &ResolvedPayload) -> bool {
    payload.identity() == &TxIdentity::from_transaction(tx)
}

fn resolved_within_grant(grant: ComputeGrant, payload: &ResolvedPayload) -> bool {
    payload.resolved_resident_bytes() <= grant.max_resident_bytes
        && payload.footprint.edge_count() <= grant.max_edges
}

impl ResolutionEvidence {
    pub(in crate::authority) fn for_foundation(
        resolved: Arc<ResolvedTransaction>,
        fee: Capacity,
        resident_bytes: usize,
        verify_class: VerifyCycleClass,
    ) -> Self {
        Self {
            resolved,
            fee,
            resident_bytes,
            verify_class,
        }
    }
}

impl ResolveWork {
    pub(in crate::authority) fn yield_verify(
        self,
        resolution: FoundationResolution,
    ) -> Result<ComputeSettlement, ReceiptFailure<ResolutionReceiptError>> {
        self.yield_verify_as(resolution, VerifyCycleClass::Small)
    }

    pub(in crate::authority) fn yield_verify_as(
        self,
        resolution: FoundationResolution,
        verify_class: VerifyCycleClass,
    ) -> Result<ComputeSettlement, ReceiptFailure<ResolutionReceiptError>> {
        let payload = resolution.into_payload();
        if !payload_matches(&self.tx, &payload) {
            return Err(ReceiptFailure::new(
                self.token,
                ResolutionReceiptError::TransactionMismatch,
            ));
        }
        let next = if resolved_within_grant(self.token.grant, &payload) {
            SettlementNext::QueuedVerify(ResolvedFacts::from_resolution(
                ResolutionSeal(()),
                self.token.chain_view().clone(),
                self.token.dependency_cut,
                Arc::new(payload),
                verify_class,
            ))
        } else {
            return Ok(budget_denied(self.token));
        };
        Ok(self.token.settle(next))
    }
}

impl ContinuousResolveWork {
    pub(in crate::authority) fn into_verify(
        self,
        resolution: FoundationResolution,
    ) -> Result<ContinuousResolution, ReceiptFailure<ResolutionReceiptError>> {
        self.into_verify_as(resolution, VerifyCycleClass::Small)
    }

    pub(in crate::authority) fn into_verify_as(
        self,
        resolution: FoundationResolution,
        verify_class: VerifyCycleClass,
    ) -> Result<ContinuousResolution, ReceiptFailure<ResolutionReceiptError>> {
        let payload = resolution.into_payload();
        if !payload_matches(&self.tx, &payload) {
            return Err(ReceiptFailure::new(
                self.token,
                ResolutionReceiptError::TransactionMismatch,
            ));
        }
        if !resolved_within_grant(self.token.grant, &payload) {
            return Ok(ContinuousResolution::Settle(budget_denied(self.token)));
        }
        let chain_view = self.token.chain_view().clone();
        let resolved = ResolvedFacts::from_resolution(
            ResolutionSeal(()),
            chain_view,
            self.token.dependency_cut,
            Arc::new(payload),
            verify_class,
        );
        if self.capability.permits(verify_class) {
            Ok(ContinuousResolution::Verify(ContinuousVerifyWork {
                token: self.token,
                resolved,
            }))
        } else {
            Ok(ContinuousResolution::Settle(
                self.token.settle(SettlementNext::QueuedVerify(resolved)),
            ))
        }
    }
}

impl VerifyWork {
    pub(in crate::authority) fn transaction(&self) -> &TransactionView {
        &self.resolved.payload().resolved_transaction().transaction
    }

    pub(in crate::authority) fn payload_policy(&self) -> PayloadPolicy {
        self.token.payload_policy
    }

    pub(in crate::authority) fn resolved_transaction(&self) -> &Arc<ResolvedTransaction> {
        self.resolved.payload().resolved_transaction()
    }

    pub(in crate::authority) fn rejected(
        self,
        reason: impl Into<CommittedPublicReject>,
    ) -> ComputeSettlement {
        self.token.settle(SettlementNext::VerificationRejected {
            rejection: reason.into(),
            resolved: self.resolved,
        })
    }

    pub(in crate::authority) fn internal_failure(self) -> ComputeSettlement {
        internal_failure(self.token)
    }

    pub(in crate::authority) fn verified(self, cycles: u64) -> ComputeSettlement {
        self.verified_under(cycles, ScriptVerificationRules::V0)
    }

    pub(in crate::authority) fn verified_under(
        self,
        cycles: u64,
        rules: ScriptVerificationRules,
    ) -> ComputeSettlement {
        let tip = self.token.chain_view().tip().0.clone();
        match self.bind_current(&tip) {
            Ok(work) => work.verified_with_time_context(
                cycles,
                TimeContextReceipt::from_validation(rules),
                AsyncProcessStart::now(),
            ),
            Err(stale) => stale,
        }
    }
}

impl ContinuousVerifyWork {
    pub(in crate::authority) fn internal_failure(self) -> ComputeSettlement {
        internal_failure(self.token)
    }

    pub(in crate::authority) fn rejected(
        self,
        reason: impl Into<CommittedPublicReject>,
    ) -> ComputeSettlement {
        self.token.settle(SettlementNext::VerificationRejected {
            rejection: reason.into(),
            resolved: self.resolved,
        })
    }

    pub(in crate::authority) fn verified(self, cycles: u64) -> ComputeSettlement {
        self.verified_under(cycles, ScriptVerificationRules::V0)
    }

    pub(in crate::authority) fn verified_under(
        self,
        cycles: u64,
        rules: ScriptVerificationRules,
    ) -> ComputeSettlement {
        self.into_current().verified_with_time_context(
            cycles,
            TimeContextReceipt::from_validation(rules),
            AsyncProcessStart::now(),
        )
    }
}

impl CheckedOutWork {
    pub(in crate::authority) fn cancelled(self) -> ComputeSettlement {
        let token = match self {
            Self::Resolve(work) => work.token,
            Self::ContinuousResolve(work) => work.token,
            Self::Verify(work) => work.token,
        };
        internal_failure(token)
    }
}
