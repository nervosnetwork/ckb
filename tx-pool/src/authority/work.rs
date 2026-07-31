#[cfg(test)]
use super::chain::ValidationRulesId;
use super::chain::{CellLocationReceipt, TimeContextReceipt, VerificationContextReceipt};
use super::rejection::CommittedPublicReject;
use super::resources::AcceptedCost;
#[cfg(test)]
use super::state::FoundationResolution;
use super::state::{
    CandidateMetrics, ChainViewId, ComputeGrant, ComputeLeaseId, DependencyCut, DependencyKey,
    DependencySetError, EntryVersion, InputEvidenceError, KnownDependencies, MissingDependencies,
    QueuedWork, RawTxHash, ResolvedFacts, ResolvedPayload, TxIdentity, VerifiedFacts,
    VerifyCapability, VerifyCycleClass, WorkPermit,
};
use crate::error::Reject;
use ckb_types::core::TransactionView;
use ckb_types::core::{Capacity, cell::ResolvedTransaction};
use std::sync::Arc;

#[derive(Debug)]
pub(super) struct SettlementToken {
    pub(super) hash: RawTxHash,
    pub(super) version: EntryVersion,
    pub(super) lease: ComputeLeaseId,
}

#[derive(Debug)]
pub(super) struct LeaseToken {
    pub(super) settlement: SettlementToken,
    pub(super) chain_view: ChainViewId,
    pub(super) dependency_cut: DependencyCut,
    pub(super) permit: WorkPermit,
    pub(super) grant: ComputeGrant,
}

impl LeaseToken {
    fn chain_view(&self) -> &ChainViewId {
        &self.chain_view
    }

    fn settle(self, next: SettlementNext) -> ComputeSettlement {
        ComputeSettlement {
            token: self.settlement,
            next,
        }
    }
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
    resolved: Arc<ResolvedTransaction>,
    fee: Capacity,
    resident_bytes: usize,
    verify_class: VerifyCycleClass,
}

impl ResolutionEvidence {
    pub(super) fn new(
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

#[derive(Debug)]
pub(super) struct ResolveWork {
    token: LeaseToken,
    tx: Arc<TransactionView>,
    declared_dependencies: KnownDependencies,
}

#[derive(Debug)]
pub(super) struct ContinuousResolveWork {
    token: LeaseToken,
    tx: Arc<TransactionView>,
    declared_dependencies: KnownDependencies,
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
    EmptyDependencies,
    DependencyAllocation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VerificationReceiptError {
    TransactionMismatch,
    ResidentBelowResolved,
    ContextMismatch,
}

#[derive(Debug)]
#[must_use = "a rejected compute receipt still owns the exact lease settlement"]
pub(super) struct ReceiptFailure<E> {
    error: E,
    token: SettlementToken,
}

impl<E> ReceiptFailure<E> {
    fn new(token: LeaseToken, error: E) -> Self {
        Self {
            error,
            token: token.settlement,
        }
    }

    pub(super) fn error(&self) -> &E {
        &self.error
    }

    pub(super) fn into_settlement(self) -> ComputeSettlement {
        ComputeSettlement {
            token: self.token,
            next: SettlementNext::Retry,
        }
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
    Waiting(MissingResolution),
    Ready(VerifiedFacts),
    Rejected(SettlementRejection),
    Retry,
}

/// Exact worker rejection plus the minimum validity domain needed when a
/// settlement races a chain transition. The authority can never infer this
/// distinction from an error string or from the caller that returned it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SettlementRejection {
    /// Resolve/verify evidence is bound to the checked-out chain view.
    ChainBound(CommittedPublicReject),
    /// The sealed compute/residency envelope is independent of chain state.
    ResourceBound(CommittedPublicReject),
}

impl SettlementRejection {
    fn chain_bound(reason: impl Into<CommittedPublicReject>) -> Self {
        Self::ChainBound(reason.into())
    }

    fn resource_bound(reason: impl Into<CommittedPublicReject>) -> Self {
        Self::ResourceBound(reason.into())
    }

    pub(super) const fn remains_valid_after_chain_change(&self) -> bool {
        matches!(self, Self::ResourceBound(_))
    }

    pub(super) fn into_public(self) -> CommittedPublicReject {
        match self {
            Self::ChainBound(rejection) | Self::ResourceBound(rejection) => rejection,
        }
    }
}

#[derive(Debug)]
pub(super) struct MissingResolution {
    missing: MissingDependencies,
    dependencies: KnownDependencies,
}

impl MissingResolution {
    pub(super) fn missing(&self) -> &MissingDependencies {
        &self.missing
    }

    pub(super) fn dependencies(&self) -> &KnownDependencies {
        &self.dependencies
    }
}

#[derive(Debug)]
#[must_use = "a settlement must be planned and applied or explicitly discarded as stale"]
pub(super) struct ComputeSettlement {
    pub(super) token: SettlementToken,
    pub(super) next: SettlementNext,
}

fn internal_failure(token: LeaseToken) -> ComputeSettlement {
    token.settle(SettlementNext::Retry)
}

fn budget_denied(token: LeaseToken) -> ComputeSettlement {
    token.settle(SettlementNext::Rejected(
        SettlementRejection::resource_bound(Reject::Full(
            "transaction exceeds the tx-pool compute residency envelope".to_owned(),
        )),
    ))
}

fn missing_settlement(
    token: LeaseToken,
    declared_dependencies: KnownDependencies,
    keys: Vec<DependencyKey>,
) -> Result<ComputeSettlement, ReceiptFailure<ResolutionReceiptError>> {
    let missing = match MissingDependencies::new(keys, token.grant.max_edges) {
        Ok(missing) => missing,
        Err(DependencySetError::TooMany) => {
            return Ok(budget_denied(token));
        }
        Err(DependencySetError::Empty) => {
            return Err(ReceiptFailure::new(
                token,
                ResolutionReceiptError::EmptyDependencies,
            ));
        }
        Err(DependencySetError::Allocation) => {
            return Err(ReceiptFailure::new(
                token,
                ResolutionReceiptError::DependencyAllocation,
            ));
        }
        Err(DependencySetError::Arithmetic) => {
            return Err(ReceiptFailure::new(
                token,
                ResolutionReceiptError::InvalidEvidence(InputEvidenceError::DependencySet(
                    DependencySetError::Arithmetic,
                )),
            ));
        }
    };
    let dependencies = match declared_dependencies.with_missing(&missing, token.grant.max_edges) {
        Ok(dependencies) => dependencies,
        Err(DependencySetError::TooMany) => {
            return Ok(budget_denied(token));
        }
        Err(DependencySetError::Allocation) => {
            return Err(ReceiptFailure::new(
                token,
                ResolutionReceiptError::DependencyAllocation,
            ));
        }
        Err(error @ (DependencySetError::Empty | DependencySetError::Arithmetic)) => {
            return Err(ReceiptFailure::new(
                token,
                ResolutionReceiptError::InvalidEvidence(InputEvidenceError::DependencySet(error)),
            ));
        }
    };
    Ok(token.settle(SettlementNext::Waiting(MissingResolution {
        missing,
        dependencies,
    })))
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
) -> Result<Option<(ResolvedPayload, CellLocationReceipt, VerifyCycleClass)>, ResolutionReceiptError>
{
    if evidence.resident_bytes > token.grant.max_resident_bytes {
        return Ok(None);
    }
    let ResolutionEvidence {
        resolved,
        fee,
        resident_bytes,
        verify_class,
    } = evidence;
    if &resolved.transaction != tx {
        return Err(ResolutionReceiptError::TransactionMismatch);
    }
    match ResolvedPayload::from_resolution(
        ResolutionSeal(()),
        resolved,
        token.grant.max_edges,
        fee,
        resident_bytes,
    ) {
        Ok(payload) => {
            // `token.chain_view` and the resolved metadata were captured by
            // the same checked-out resolve operation. Do not split this
            // provenance when wiring the production resolver in G5.
            let location = CellLocationReceipt::from_resolution(token.chain_view(), &payload);
            Ok(Some((payload, location, verify_class)))
        }
        Err(
            InputEvidenceError::Footprint(super::state::FootprintError::TooManyEdges)
            | InputEvidenceError::DependencySet(DependencySetError::TooMany),
        ) => Ok(None),
        Err(InputEvidenceError::DependencySet(DependencySetError::Allocation)) => {
            Err(ResolutionReceiptError::DependencyAllocation)
        }
        Err(error) => Err(ResolutionReceiptError::InvalidEvidence(error)),
    }
}

fn verified(
    token: LeaseToken,
    tx: Arc<TransactionView>,
    resolved: ResolvedFacts,
    accepted_resident_bytes: usize,
    cycles: u64,
    context: VerificationContextReceipt,
) -> Result<ComputeSettlement, ReceiptFailure<VerificationReceiptError>> {
    if !payload_matches(&tx, resolved.payload()) {
        return Err(ReceiptFailure::new(
            token,
            VerificationReceiptError::TransactionMismatch,
        ));
    }
    let serialized_bytes = resolved.payload().serialized_bytes();
    if accepted_resident_bytes < resolved.payload().resolved_resident_bytes() {
        return Err(ReceiptFailure::new(
            token,
            VerificationReceiptError::ResidentBelowResolved,
        ));
    }
    if accepted_resident_bytes > token.grant.max_resident_bytes
        || resolved.payload().footprint.edge_count() > token.grant.max_edges
    {
        return Ok(budget_denied(token));
    }
    let metrics = CandidateMetrics {
        fee: resolved.payload().fee(),
        cost: AcceptedCost::new(serialized_bytes, accepted_resident_bytes, cycles),
    };
    if !context.is_for(token.chain_view()) {
        return Err(ReceiptFailure::new(
            token,
            VerificationReceiptError::ContextMismatch,
        ));
    }
    let (dependency_cut, content, _location, _) =
        resolved.into_verification_parts(VerificationSeal(()));
    Ok(
        token.settle(SettlementNext::Ready(VerifiedFacts::from_verification(
            VerificationSeal(()),
            dependency_cut,
            content,
            context,
            metrics,
        ))),
    )
}

impl ResolveWork {
    pub(super) fn transaction(&self) -> &TransactionView {
        &self.tx
    }

    pub(super) fn missing(
        self,
        keys: Vec<DependencyKey>,
    ) -> Result<ComputeSettlement, ReceiptFailure<ResolutionReceiptError>> {
        missing_settlement(self.token, self.declared_dependencies, keys)
    }

    pub(super) fn resolution_grant(&self) -> ComputeGrant {
        self.token.grant
    }

    pub(super) fn resolved(
        self,
        evidence: ResolutionEvidence,
    ) -> Result<ComputeSettlement, ReceiptFailure<ResolutionReceiptError>> {
        let next = match build_resolved_payload(&self.token, &self.tx, evidence) {
            Err(error) => return Err(ReceiptFailure::new(self.token, error)),
            Ok(Some((payload, location, verify_class))) => {
                SettlementNext::QueuedVerify(ResolvedFacts::from_resolution(
                    ResolutionSeal(()),
                    self.token.chain_view().clone(),
                    self.token.dependency_cut,
                    Arc::new(payload),
                    location,
                    verify_class,
                ))
            }
            Ok(None) => {
                return Ok(budget_denied(self.token));
            }
        };
        Ok(self.token.settle(next))
    }

    #[cfg(test)]
    pub(super) fn yield_verify(
        self,
        resolution: FoundationResolution,
    ) -> Result<ComputeSettlement, ReceiptFailure<ResolutionReceiptError>> {
        self.yield_verify_as(resolution, VerifyCycleClass::Small)
    }

    #[cfg(test)]
    pub(super) fn yield_verify_as(
        self,
        resolution: FoundationResolution,
        verify_class: VerifyCycleClass,
    ) -> Result<ComputeSettlement, ReceiptFailure<ResolutionReceiptError>> {
        let (payload, location) = resolution.into_parts();
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
                location,
                verify_class,
            ))
        } else {
            return Ok(budget_denied(self.token));
        };
        Ok(self.token.settle(next))
    }

    pub(super) fn rejected(self, reason: impl Into<CommittedPublicReject>) -> ComputeSettlement {
        self.token
            .settle(SettlementNext::Rejected(SettlementRejection::chain_bound(
                reason,
            )))
    }

    pub(super) fn internal_failure(self) -> ComputeSettlement {
        internal_failure(self.token)
    }
}

impl ContinuousResolveWork {
    pub(super) fn transaction(&self) -> &TransactionView {
        &self.tx
    }

    pub(super) fn missing(
        self,
        keys: Vec<DependencyKey>,
    ) -> Result<ComputeSettlement, ReceiptFailure<ResolutionReceiptError>> {
        ResolveWork {
            token: self.token,
            tx: self.tx,
            declared_dependencies: self.declared_dependencies,
        }
        .missing(keys)
    }

    pub(super) fn resolution_grant(&self) -> ComputeGrant {
        self.token.grant
    }

    pub(super) fn resolved(
        self,
        evidence: ResolutionEvidence,
    ) -> Result<ContinuousResolution, ReceiptFailure<ResolutionReceiptError>> {
        let payload = match build_resolved_payload(&self.token, &self.tx, evidence) {
            Ok(payload) => payload,
            Err(error) => return Err(ReceiptFailure::new(self.token, error)),
        };
        let Some((payload, location, verify_class)) = payload else {
            return Ok(ContinuousResolution::Settle(budget_denied(self.token)));
        };
        let chain_view = self.token.chain_view().clone();
        let resolved = ResolvedFacts::from_resolution(
            ResolutionSeal(()),
            chain_view,
            self.token.dependency_cut,
            Arc::new(payload),
            location,
            verify_class,
        );
        if self.capability.permits(verify_class) {
            Ok(ContinuousResolution::Verify(ContinuousVerifyWork {
                token: self.token,
                tx: self.tx,
                resolved,
            }))
        } else {
            Ok(ContinuousResolution::Settle(
                self.token.settle(SettlementNext::QueuedVerify(resolved)),
            ))
        }
    }

    #[cfg(test)]
    pub(super) fn into_verify(
        self,
        resolution: FoundationResolution,
    ) -> Result<ContinuousResolution, ReceiptFailure<ResolutionReceiptError>> {
        self.into_verify_as(resolution, VerifyCycleClass::Small)
    }

    #[cfg(test)]
    pub(super) fn into_verify_as(
        self,
        resolution: FoundationResolution,
        verify_class: VerifyCycleClass,
    ) -> Result<ContinuousResolution, ReceiptFailure<ResolutionReceiptError>> {
        let (payload, location) = resolution.into_parts();
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
            location,
            verify_class,
        );
        if self.capability.permits(verify_class) {
            Ok(ContinuousResolution::Verify(ContinuousVerifyWork {
                token: self.token,
                tx: self.tx,
                resolved,
            }))
        } else {
            Ok(ContinuousResolution::Settle(
                self.token.settle(SettlementNext::QueuedVerify(resolved)),
            ))
        }
    }

    pub(super) fn rejected(self, reason: impl Into<CommittedPublicReject>) -> ComputeSettlement {
        self.token
            .settle(SettlementNext::Rejected(SettlementRejection::chain_bound(
                reason,
            )))
    }

    pub(super) fn internal_failure(self) -> ComputeSettlement {
        internal_failure(self.token)
    }
}

impl VerifyWork {
    pub(super) fn transaction(&self) -> &TransactionView {
        &self.tx
    }

    #[cfg(test)]
    pub(super) fn verified(
        self,
        accepted_resident_bytes: usize,
        cycles: u64,
    ) -> Result<ComputeSettlement, ReceiptFailure<VerificationReceiptError>> {
        self.verified_under(
            accepted_resident_bytes,
            cycles,
            ValidationRulesId::FOUNDATION,
        )
    }

    #[cfg(test)]
    pub(super) fn verified_under(
        self,
        accepted_resident_bytes: usize,
        cycles: u64,
        rules: ValidationRulesId,
    ) -> Result<ComputeSettlement, ReceiptFailure<VerificationReceiptError>> {
        let context = match VerificationContextReceipt::refresh_for_foundation(
            self.token.chain_view().clone(),
            self.resolved.location_receipt().clone(),
            rules,
        ) {
            Ok(context) => context,
            Err(_) => {
                return Err(ReceiptFailure::new(
                    self.token,
                    VerificationReceiptError::ContextMismatch,
                ));
            }
        };
        self.verified_with_context(accepted_resident_bytes, cycles, context)
    }

    /// Seal post-script time/rules evidence into the exact resolved payload
    /// owned by this compute capability. The location receipt and chain view
    /// cannot be supplied independently by a runtime caller.
    pub(super) fn verified_with_time_context(
        self,
        accepted_resident_bytes: usize,
        cycles: u64,
        time: TimeContextReceipt,
    ) -> Result<ComputeSettlement, ReceiptFailure<VerificationReceiptError>> {
        let context = match VerificationContextReceipt::from_validation(
            self.token.chain_view().clone(),
            self.resolved.location_receipt().clone(),
            time,
        ) {
            Ok(context) => context,
            Err(_) => {
                return Err(ReceiptFailure::new(
                    self.token,
                    VerificationReceiptError::ContextMismatch,
                ));
            }
        };
        self.verified_with_context(accepted_resident_bytes, cycles, context)
    }

    /// Consume one validator-sealed location/time/view receipt. A changed-tip
    /// caller must obtain a newly validated context; it cannot restamp the
    /// location receipt retained by resolution.
    pub(super) fn verified_with_context(
        self,
        accepted_resident_bytes: usize,
        cycles: u64,
        context: VerificationContextReceipt,
    ) -> Result<ComputeSettlement, ReceiptFailure<VerificationReceiptError>> {
        verified(
            self.token,
            self.tx,
            self.resolved,
            accepted_resident_bytes,
            cycles,
            context,
        )
    }

    pub(super) fn rejected(self, reason: impl Into<CommittedPublicReject>) -> ComputeSettlement {
        self.token
            .settle(SettlementNext::Rejected(SettlementRejection::chain_bound(
                reason,
            )))
    }

    pub(super) fn internal_failure(self) -> ComputeSettlement {
        internal_failure(self.token)
    }
}

impl ContinuousVerifyWork {
    #[cfg(test)]
    pub(super) fn verified(
        self,
        accepted_resident_bytes: usize,
        cycles: u64,
    ) -> Result<ComputeSettlement, ReceiptFailure<VerificationReceiptError>> {
        self.verified_under(
            accepted_resident_bytes,
            cycles,
            ValidationRulesId::FOUNDATION,
        )
    }

    #[cfg(test)]
    pub(super) fn verified_under(
        self,
        accepted_resident_bytes: usize,
        cycles: u64,
        rules: ValidationRulesId,
    ) -> Result<ComputeSettlement, ReceiptFailure<VerificationReceiptError>> {
        let context = match VerificationContextReceipt::refresh_for_foundation(
            self.token.chain_view().clone(),
            self.resolved.location_receipt().clone(),
            rules,
        ) {
            Ok(context) => context,
            Err(_) => {
                return Err(ReceiptFailure::new(
                    self.token,
                    VerificationReceiptError::ContextMismatch,
                ));
            }
        };
        self.verified_with_context(accepted_resident_bytes, cycles, context)
    }

    pub(super) fn verified_with_time_context(
        self,
        accepted_resident_bytes: usize,
        cycles: u64,
        time: TimeContextReceipt,
    ) -> Result<ComputeSettlement, ReceiptFailure<VerificationReceiptError>> {
        let context = match VerificationContextReceipt::from_validation(
            self.token.chain_view().clone(),
            self.resolved.location_receipt().clone(),
            time,
        ) {
            Ok(context) => context,
            Err(_) => {
                return Err(ReceiptFailure::new(
                    self.token,
                    VerificationReceiptError::ContextMismatch,
                ));
            }
        };
        self.verified_with_context(accepted_resident_bytes, cycles, context)
    }

    pub(super) fn verified_with_context(
        self,
        accepted_resident_bytes: usize,
        cycles: u64,
        context: VerificationContextReceipt,
    ) -> Result<ComputeSettlement, ReceiptFailure<VerificationReceiptError>> {
        verified(
            self.token,
            self.tx,
            self.resolved,
            accepted_resident_bytes,
            cycles,
            context,
        )
    }

    pub(super) fn internal_failure(self) -> ComputeSettlement {
        internal_failure(self.token)
    }

    pub(super) fn rejected(self, reason: impl Into<CommittedPublicReject>) -> ComputeSettlement {
        self.token
            .settle(SettlementNext::Rejected(SettlementRejection::chain_bound(
                reason,
            )))
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
        declared_dependencies: KnownDependencies,
        queued: QueuedWork,
    ) -> Result<Self, WorkPermitMismatch> {
        match (token.permit, queued) {
            (WorkPermit::ResolveOnly, QueuedWork::Resolve) => Ok(Self::Resolve(ResolveWork {
                token,
                tx,
                declared_dependencies,
            })),
            (WorkPermit::ResolveThenVerify(capability), QueuedWork::Resolve) => {
                Ok(Self::ContinuousResolve(ContinuousResolveWork {
                    token,
                    tx,
                    declared_dependencies,
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
