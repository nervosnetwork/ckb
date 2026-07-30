use super::resources::AcceptedCost;
use super::state::{
    CandidateMetrics, ChainEpoch, ComputeGrant, ComputeLeaseId, ComputedOutcome, EntryVersion,
    InputEvidenceError, ObservedDependencies, QueuedWork, RawTxHash, RejectionKind, ResolvedFacts,
    ResolvedPayload, TxIdentity, VerifiedFacts, VerifyCapability, VerifyCycleClass, WaitCondition,
    WorkPermit,
};
use ckb_types::core::TransactionView;
use ckb_types::{core::Capacity, packed::OutPoint};
use std::sync::Arc;

#[derive(Debug)]
pub(super) struct LeaseToken {
    pub(super) hash: RawTxHash,
    pub(super) version: EntryVersion,
    pub(super) lease: ComputeLeaseId,
    pub(super) chain_epoch: ChainEpoch,
    pub(super) permit: WorkPermit,
    pub(super) grant: ComputeGrant,
}

/// Constructor capability for `ResolvedPayload`. Its field is private to this
/// module, so no sibling can manufacture retained resolution evidence without
/// consuming checked-out work.
pub(super) struct ResolutionSeal(());

/// Constructor capability binding post-script metrics to the exact resolved
/// payload consumed by this module's move-only verify work.
pub(super) struct VerificationSeal(());

#[derive(Debug)]
pub(super) struct ResolutionEvidence {
    expanded_dependencies: Vec<OutPoint>,
    chain_inputs: Vec<OutPoint>,
    fee: Capacity,
    resident_bytes: usize,
    verify_class: VerifyCycleClass,
}

impl ResolutionEvidence {
    pub(super) fn new(
        expanded_dependencies: Vec<OutPoint>,
        chain_inputs: Vec<OutPoint>,
        fee: Capacity,
        resident_bytes: usize,
        verify_class: VerifyCycleClass,
    ) -> Self {
        Self {
            expanded_dependencies,
            chain_inputs,
            fee,
            resident_bytes,
            verify_class,
        }
    }
}

#[derive(Debug)]
pub(super) struct ResolveWork {
    token: LeaseToken,
    tx: Arc<TransactionView>,
}

#[derive(Debug)]
pub(super) struct ContinuousResolveWork {
    token: LeaseToken,
    tx: Arc<TransactionView>,
    capability: VerifyCapability,
}

#[derive(Debug)]
pub(super) struct VerifyWork {
    token: LeaseToken,
    tx: Arc<TransactionView>,
    resolved: ResolvedFacts,
}

#[derive(Debug)]
pub(super) struct ContinuousVerifyWork {
    token: LeaseToken,
    tx: Arc<TransactionView>,
    resolved: ResolvedFacts,
}

#[derive(Debug)]
#[must_use = "checked-out work owns the only live compute capability"]
pub(super) enum CheckedOutWork {
    Resolve(ResolveWork),
    ContinuousResolve(ContinuousResolveWork),
    Verify(VerifyWork),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorkPermitMismatch {
    Incompatible,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResolutionReceiptError {
    TransactionMismatch,
    InvalidEvidence(InputEvidenceError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VerificationReceiptError {
    TransactionMismatch,
    ResidentBelowSerialized,
}

#[derive(Debug)]
#[must_use = "a rejected compute receipt still owns the exact lease settlement"]
pub(super) struct ReceiptFailure<E> {
    error: E,
    settlement: ComputeSettlement,
}

impl<E> ReceiptFailure<E> {
    fn new(token: LeaseToken, error: E) -> Box<Self> {
        Box::new(Self {
            error,
            settlement: internal_failure(token),
        })
    }

    pub(super) fn error(&self) -> &E {
        &self.error
    }

    pub(super) fn into_settlement(self) -> ComputeSettlement {
        self.settlement
    }
}

#[must_use = "continuous resolution must continue verification or settle its lease"]
pub(super) enum ContinuousResolution {
    Verify(ContinuousVerifyWork),
    Settle(ComputeSettlement),
}

#[derive(Debug)]
pub(super) enum SettlementNext {
    QueuedVerify(ResolvedFacts),
    Waiting(WaitCondition),
    Computed(ComputedOutcome),
}

#[derive(Debug)]
#[must_use = "a settlement must be planned and applied or explicitly discarded as stale"]
pub(super) struct ComputeSettlement {
    pub(super) token: LeaseToken,
    pub(super) next: SettlementNext,
}

fn internal_failure(token: LeaseToken) -> ComputeSettlement {
    ComputeSettlement {
        token,
        next: SettlementNext::Computed(ComputedOutcome::InternalFailure),
    }
}

fn payload_matches(tx: &TransactionView, payload: &ResolvedPayload) -> bool {
    payload.identity() == &TxIdentity::from_transaction(tx)
}

fn resolved_within_grant(grant: ComputeGrant, payload: &ResolvedPayload) -> bool {
    payload.resolved_resident_bytes() <= grant.max_resident_bytes
        && payload.footprint.edge_count() <= grant.max_edges
}

fn build_resolved_payload(
    token: &LeaseToken,
    tx: &TransactionView,
    evidence: ResolutionEvidence,
) -> Result<Option<(ResolvedPayload, VerifyCycleClass)>, ResolutionReceiptError> {
    if evidence.resident_bytes > token.grant.max_resident_bytes {
        return Ok(None);
    }
    let verify_class = evidence.verify_class;
    match ResolvedPayload::from_resolution(
        ResolutionSeal(()),
        tx,
        evidence.expanded_dependencies,
        token.grant.max_edges,
        evidence.fee,
        evidence.resident_bytes,
        evidence.chain_inputs,
    ) {
        Ok(payload) => Ok(Some((payload, verify_class))),
        Err(InputEvidenceError::Footprint(super::state::FootprintError::TooManyEdges)) => Ok(None),
        Err(error) => Err(ResolutionReceiptError::InvalidEvidence(error)),
    }
}

fn verified(
    token: LeaseToken,
    tx: Arc<TransactionView>,
    resolved: ResolvedFacts,
    accepted_resident_bytes: usize,
    cycles: u64,
) -> Result<ComputeSettlement, Box<ReceiptFailure<VerificationReceiptError>>> {
    if !payload_matches(&tx, resolved.payload()) {
        return Err(ReceiptFailure::new(
            token,
            VerificationReceiptError::TransactionMismatch,
        ));
    }
    let serialized_bytes = resolved.payload().serialized_bytes();
    if accepted_resident_bytes < serialized_bytes {
        return Err(ReceiptFailure::new(
            token,
            VerificationReceiptError::ResidentBelowSerialized,
        ));
    }
    if accepted_resident_bytes > token.grant.max_resident_bytes
        || resolved.payload().footprint.edge_count() > token.grant.max_edges
    {
        return Ok(ComputeSettlement {
            token,
            next: SettlementNext::Computed(ComputedOutcome::BudgetDenied),
        });
    }
    let identity = TxIdentity::from_transaction(&tx);
    let metrics = CandidateMetrics {
        fee: resolved.payload().fee(),
        cost: AcceptedCost::new(serialized_bytes, accepted_resident_bytes, cycles),
    };
    let (chain_epoch, payload, _) = resolved.into_verification_parts(VerificationSeal(()));
    Ok(ComputeSettlement {
        next: SettlementNext::Computed(ComputedOutcome::Verified(
            VerifiedFacts::from_verification(
                VerificationSeal(()),
                identity.witness,
                chain_epoch,
                payload,
                metrics,
            ),
        )),
        token,
    })
}

impl ResolveWork {
    pub(super) fn transaction(&self) -> &TransactionView {
        &self.tx
    }

    pub(super) fn missing(self, dependencies: ObservedDependencies) -> ComputeSettlement {
        ComputeSettlement {
            token: self.token,
            next: SettlementNext::Waiting(WaitCondition::Missing(dependencies)),
        }
    }

    pub(super) fn resolution_grant(&self) -> ComputeGrant {
        self.token.grant
    }

    pub(super) fn resolved(
        self,
        evidence: ResolutionEvidence,
    ) -> Result<ComputeSettlement, Box<ReceiptFailure<ResolutionReceiptError>>> {
        let next = match build_resolved_payload(&self.token, &self.tx, evidence) {
            Err(error) => return Err(ReceiptFailure::new(self.token, error)),
            Ok(Some((payload, verify_class))) => {
                SettlementNext::QueuedVerify(ResolvedFacts::from_resolution(
                    ResolutionSeal(()),
                    self.token.chain_epoch,
                    Arc::new(payload),
                    verify_class,
                ))
            }
            Ok(None) => SettlementNext::Computed(ComputedOutcome::BudgetDenied),
        };
        Ok(ComputeSettlement {
            token: self.token,
            next,
        })
    }

    #[cfg(test)]
    pub(super) fn yield_verify(
        self,
        payload: ResolvedPayload,
    ) -> Result<ComputeSettlement, Box<ReceiptFailure<ResolutionReceiptError>>> {
        self.yield_verify_as(payload, VerifyCycleClass::Small)
    }

    #[cfg(test)]
    pub(super) fn yield_verify_as(
        self,
        payload: ResolvedPayload,
        verify_class: VerifyCycleClass,
    ) -> Result<ComputeSettlement, Box<ReceiptFailure<ResolutionReceiptError>>> {
        if !payload_matches(&self.tx, &payload) {
            return Err(ReceiptFailure::new(
                self.token,
                ResolutionReceiptError::TransactionMismatch,
            ));
        }
        let next = if resolved_within_grant(self.token.grant, &payload) {
            SettlementNext::QueuedVerify(ResolvedFacts::from_resolution(
                ResolutionSeal(()),
                self.token.chain_epoch,
                Arc::new(payload),
                verify_class,
            ))
        } else {
            SettlementNext::Computed(ComputedOutcome::BudgetDenied)
        };
        Ok(ComputeSettlement {
            token: self.token,
            next,
        })
    }

    pub(super) fn rejected(self, reason: RejectionKind) -> ComputeSettlement {
        ComputeSettlement {
            token: self.token,
            next: SettlementNext::Computed(ComputedOutcome::Rejected(reason)),
        }
    }

    pub(super) fn internal_failure(self) -> ComputeSettlement {
        internal_failure(self.token)
    }
}

impl ContinuousResolveWork {
    pub(super) fn transaction(&self) -> &TransactionView {
        &self.tx
    }

    pub(super) fn missing(self, dependencies: ObservedDependencies) -> ComputeSettlement {
        ComputeSettlement {
            token: self.token,
            next: SettlementNext::Waiting(WaitCondition::Missing(dependencies)),
        }
    }

    pub(super) fn resolution_grant(&self) -> ComputeGrant {
        self.token.grant
    }

    pub(super) fn resolved(
        self,
        evidence: ResolutionEvidence,
    ) -> Result<ContinuousResolution, Box<ReceiptFailure<ResolutionReceiptError>>> {
        let payload = match build_resolved_payload(&self.token, &self.tx, evidence) {
            Ok(payload) => payload,
            Err(error) => return Err(ReceiptFailure::new(self.token, error)),
        };
        let Some((payload, verify_class)) = payload else {
            return Ok(ContinuousResolution::Settle(ComputeSettlement {
                token: self.token,
                next: SettlementNext::Computed(ComputedOutcome::BudgetDenied),
            }));
        };
        let chain_epoch = self.token.chain_epoch;
        let resolved = ResolvedFacts::from_resolution(
            ResolutionSeal(()),
            chain_epoch,
            Arc::new(payload),
            verify_class,
        );
        if self.capability.permits(verify_class) {
            Ok(ContinuousResolution::Verify(ContinuousVerifyWork {
                token: self.token,
                tx: self.tx,
                resolved,
            }))
        } else {
            Ok(ContinuousResolution::Settle(ComputeSettlement {
                token: self.token,
                next: SettlementNext::QueuedVerify(resolved),
            }))
        }
    }

    #[cfg(test)]
    pub(super) fn into_verify(
        self,
        payload: ResolvedPayload,
    ) -> Result<ContinuousResolution, Box<ReceiptFailure<ResolutionReceiptError>>> {
        self.into_verify_as(payload, VerifyCycleClass::Small)
    }

    #[cfg(test)]
    pub(super) fn into_verify_as(
        self,
        payload: ResolvedPayload,
        verify_class: VerifyCycleClass,
    ) -> Result<ContinuousResolution, Box<ReceiptFailure<ResolutionReceiptError>>> {
        if !payload_matches(&self.tx, &payload) {
            return Err(ReceiptFailure::new(
                self.token,
                ResolutionReceiptError::TransactionMismatch,
            ));
        }
        if !resolved_within_grant(self.token.grant, &payload) {
            return Ok(ContinuousResolution::Settle(ComputeSettlement {
                token: self.token,
                next: SettlementNext::Computed(ComputedOutcome::BudgetDenied),
            }));
        }
        let chain_epoch = self.token.chain_epoch;
        let resolved = ResolvedFacts::from_resolution(
            ResolutionSeal(()),
            chain_epoch,
            Arc::new(payload),
            verify_class,
        );
        if self.capability.permits(verify_class) {
            Ok(ContinuousResolution::Verify(ContinuousVerifyWork {
                token: self.token,
                tx: self.tx,
                resolved,
            }))
        } else {
            Ok(ContinuousResolution::Settle(ComputeSettlement {
                token: self.token,
                next: SettlementNext::QueuedVerify(resolved),
            }))
        }
    }

    pub(super) fn rejected(self, reason: RejectionKind) -> ComputeSettlement {
        ComputeSettlement {
            token: self.token,
            next: SettlementNext::Computed(ComputedOutcome::Rejected(reason)),
        }
    }

    pub(super) fn internal_failure(self) -> ComputeSettlement {
        internal_failure(self.token)
    }
}

impl VerifyWork {
    pub(super) fn transaction(&self) -> &TransactionView {
        &self.tx
    }

    pub(super) fn verified(
        self,
        accepted_resident_bytes: usize,
        cycles: u64,
    ) -> Result<ComputeSettlement, Box<ReceiptFailure<VerificationReceiptError>>> {
        verified(
            self.token,
            self.tx,
            self.resolved,
            accepted_resident_bytes,
            cycles,
        )
    }

    pub(super) fn rejected(self, reason: RejectionKind) -> ComputeSettlement {
        ComputeSettlement {
            token: self.token,
            next: SettlementNext::Computed(ComputedOutcome::Rejected(reason)),
        }
    }

    pub(super) fn internal_failure(self) -> ComputeSettlement {
        internal_failure(self.token)
    }
}

impl ContinuousVerifyWork {
    pub(super) fn verified(
        self,
        accepted_resident_bytes: usize,
        cycles: u64,
    ) -> Result<ComputeSettlement, Box<ReceiptFailure<VerificationReceiptError>>> {
        verified(
            self.token,
            self.tx,
            self.resolved,
            accepted_resident_bytes,
            cycles,
        )
    }

    pub(super) fn internal_failure(self) -> ComputeSettlement {
        internal_failure(self.token)
    }

    pub(super) fn rejected(self, reason: RejectionKind) -> ComputeSettlement {
        ComputeSettlement {
            token: self.token,
            next: SettlementNext::Computed(ComputedOutcome::Rejected(reason)),
        }
    }
}

impl CheckedOutWork {
    /// Structured runner cancellation consumes the one live capability and
    /// yields an ordinary settlement. The runner must Apply this receipt
    /// before exiting; an unexpected task loss is handled as a service fault,
    /// never by reconstructing a lease from authority state.
    pub(super) fn cancelled(self) -> ComputeSettlement {
        let token = match self {
            Self::Resolve(work) => work.token,
            Self::ContinuousResolve(work) => work.token,
            Self::Verify(work) => work.token,
        };
        internal_failure(token)
    }

    pub(super) fn new(
        token: LeaseToken,
        tx: Arc<TransactionView>,
        queued: QueuedWork,
    ) -> Result<Self, WorkPermitMismatch> {
        match (token.permit, queued) {
            (WorkPermit::ResolveOnly, QueuedWork::Resolve) => {
                Ok(Self::Resolve(ResolveWork { token, tx }))
            }
            (WorkPermit::ResolveThenVerify(capability), QueuedWork::Resolve) => {
                Ok(Self::ContinuousResolve(ContinuousResolveWork {
                    token,
                    tx,
                    capability,
                }))
            }
            (WorkPermit::VerifyOnly(capability), QueuedWork::Verify(resolved))
                if capability.permits(resolved.verify_class()) =>
            {
                Ok(Self::Verify(VerifyWork {
                    token,
                    tx,
                    resolved,
                }))
            }
            _ => Err(WorkPermitMismatch::Incompatible),
        }
    }
}
