use super::chain::{CellContentReceipt, TimeContextReceipt};
use super::rejection::{CommittedPublicReject, duplicate_inputs_reject};
use super::resources::{AcceptedCost, ComputeGrant};
use super::state::{
    AsyncProcessStart, CandidateMetrics, ChainViewId, DependencyCut, DependencyKey,
    DependencySetError, EntryVersion, InputEvidenceDisposition, InputEvidenceError,
    KnownDependencies, MissingDependencies, PayloadPolicy, QueuedWork, RawTxHash, ResolvedFacts,
    ResolvedPayload, VerifiedFacts, VerifyCapability, VerifyCycleClass, WorkPermit,
};
use crate::error::Reject;
use ckb_types::core::TransactionView;
use ckb_types::core::{Capacity, cell::ResolvedTransaction};
use std::sync::Arc;

#[derive(Debug)]
pub(super) struct SettlementToken {
    pub(super) hash: RawTxHash,
    pub(super) version: EntryVersion,
}

#[derive(Debug)]
pub(super) struct LeaseToken {
    pub(super) settlement: SettlementToken,
    pub(super) chain_view: ChainViewId,
    pub(super) dependency_cut: DependencyCut,
    pub(super) permit: WorkPermit,
    pub(super) grant: ComputeGrant,
    pub(super) payload_policy: PayloadPolicy,
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
    pub(super) fn from_resolution(
        _seal: super::resolver::ResolutionEvidenceSeal,
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
    resolved: ResolvedFacts,
}

#[derive(Debug)]
pub(super) struct ContinuousVerifyWork {
    token: LeaseToken,
    resolved: ResolvedFacts,
}

/// A verify capability whose retained resolution facts and checkout token are
/// proven to name the same chain view. Only this type can consume a VM result.
/// A queued old-view capability instead yields its exact retry settlement
/// before any VM work starts.
#[derive(Debug)]
pub(super) struct SnapshotBoundVerifyWork {
    token: LeaseToken,
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
    /// A negative script-verification result is valid only under both the
    /// checked-out chain view and its sealed payload policy. Retaining the
    /// resolved evidence lets Apply retry verification without repeating
    /// resolution when a same-witness trusted promotion supersedes a peer's
    /// cycle limit while work is active.
    VerificationRejected {
        rejection: CommittedPublicReject,
        resolved: ResolvedFacts,
    },
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

#[derive(Clone, Debug)]
pub(super) struct MissingResolution {
    missing: MissingDependencies,
    dependencies: KnownDependencies,
    /// Canonical transaction origins of the complete missing Cell frontier.
    ///
    /// This projection is built outside the authority guard. It deliberately
    /// excludes header dependencies: production resolution rejects an invalid
    /// header directly, while the relayer can only request transactions.
    parent_transactions: Arc<[RawTxHash]>,
}

impl MissingResolution {
    pub(super) fn missing(&self) -> &MissingDependencies {
        &self.missing
    }

    pub(super) fn dependencies(&self) -> &KnownDependencies {
        &self.dependencies
    }

    pub(super) fn parent_transactions(&self) -> &Arc<[RawTxHash]> {
        &self.parent_transactions
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
    let missing = match MissingDependencies::new(keys, token.grant.max_edges()) {
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
    let dependencies = match declared_dependencies.with_missing(&missing, token.grant.max_edges()) {
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
    if token
        .grant
        .retained_base_charge(dependencies.len())
        .is_none()
    {
        return Ok(budget_denied(token));
    }
    let parent_transactions = match missing.parent_transactions() {
        Ok(parents) => parents,
        Err(_) => {
            return Err(ReceiptFailure::new(
                token,
                ResolutionReceiptError::DependencyAllocation,
            ));
        }
    };
    Ok(token.settle(SettlementNext::Waiting(MissingResolution {
        missing,
        dependencies,
        parent_transactions,
    })))
}

enum ResolvedPayloadBuild {
    Ready(ResolvedPayload, VerifyCycleClass),
    ResourceDenied,
    Rejected(CommittedPublicReject),
}

fn build_resolved_payload(
    token: &LeaseToken,
    tx: &TransactionView,
    evidence: ResolutionEvidence,
) -> Result<ResolvedPayloadBuild, ResolutionReceiptError> {
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
        token.grant.max_edges(),
        fee,
        resident_bytes,
    ) {
        Ok(payload)
            if token
                .grant
                .retained_charge(
                    payload.resolved_resident_bytes(),
                    payload.footprint.edge_count(),
                )
                .is_some() =>
        {
            Ok(ResolvedPayloadBuild::Ready(payload, verify_class))
        }
        Ok(_) => Ok(ResolvedPayloadBuild::ResourceDenied),
        Err(error) => match error.disposition() {
            InputEvidenceDisposition::MalformedTransaction => Ok(ResolvedPayloadBuild::Rejected(
                CommittedPublicReject::new(duplicate_inputs_reject()),
            )),
            InputEvidenceDisposition::ResourceDenied => Ok(ResolvedPayloadBuild::ResourceDenied),
            InputEvidenceDisposition::ResourceUnavailable => {
                Err(ResolutionReceiptError::DependencyAllocation)
            }
            InputEvidenceDisposition::Structural => {
                Err(ResolutionReceiptError::InvalidEvidence(error))
            }
        },
    }
}

fn verified(
    token: LeaseToken,
    resolved: ResolvedFacts,
    cycles: u64,
    time: TimeContextReceipt,
    async_process_start: AsyncProcessStart,
) -> ComputeSettlement {
    let serialized_bytes = resolved.payload().serialized_bytes();
    if resolved.payload().footprint.edge_count() > token.grant.max_edges() {
        return budget_denied(token);
    }
    let payload_policy = token.payload_policy;
    // Cycle-claim validation is part of the only constructor for Ready
    // evidence. A runner cannot accidentally bypass it; Apply later decides
    // whether this sealed peer policy is still current or was superseded by a
    // trusted same-witness promotion.
    if let PayloadPolicy::RemoteDeclaredCycles(declared) = payload_policy
        && declared != cycles
    {
        return token.settle(SettlementNext::VerificationRejected {
            rejection: CommittedPublicReject::new(Reject::DeclaredWrongCycles(declared, cycles)),
            resolved,
        });
    }
    let fee = resolved.payload().fee();
    let (dependency_cut, content, context, verify_class) =
        resolved.into_verification_parts(VerificationSeal(()), time);
    let (payload, accepted_resident_bytes) =
        ResolvedPayload::compact_after_verification(content.into_payload(), VerificationSeal(()));
    if token
        .grant
        .retained_charge(accepted_resident_bytes, payload.footprint.edge_count())
        .is_none()
    {
        return budget_denied(token);
    }
    let metrics = CandidateMetrics {
        fee,
        cost: AcceptedCost::new(serialized_bytes, accepted_resident_bytes, cycles),
    };
    token.settle(SettlementNext::Ready(VerifiedFacts::from_verification(
        VerificationSeal(()),
        dependency_cut,
        CellContentReceipt::from_resolution(payload),
        context,
        verify_class,
        metrics,
        async_process_start,
    )))
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

    pub(super) fn payload_policy(&self) -> PayloadPolicy {
        self.token.payload_policy
    }

    pub(super) fn chain_view(&self) -> &ChainViewId {
        self.token.chain_view()
    }

    pub(super) fn resolved(
        self,
        evidence: ResolutionEvidence,
    ) -> Result<ComputeSettlement, ReceiptFailure<ResolutionReceiptError>> {
        let next = match build_resolved_payload(&self.token, &self.tx, evidence) {
            Err(error) => return Err(ReceiptFailure::new(self.token, error)),
            Ok(ResolvedPayloadBuild::Ready(payload, verify_class)) => {
                SettlementNext::QueuedVerify(ResolvedFacts::from_resolution(
                    ResolutionSeal(()),
                    self.token.chain_view().clone(),
                    self.token.dependency_cut,
                    Arc::new(payload),
                    verify_class,
                ))
            }
            Ok(ResolvedPayloadBuild::ResourceDenied) => {
                return Ok(budget_denied(self.token));
            }
            Ok(ResolvedPayloadBuild::Rejected(rejection)) => {
                return Ok(self.token.settle(SettlementNext::Rejected(
                    SettlementRejection::chain_bound(rejection),
                )));
            }
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

    pub(super) fn resource_denied(self) -> ComputeSettlement {
        budget_denied(self.token)
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

    pub(super) fn payload_policy(&self) -> PayloadPolicy {
        self.token.payload_policy
    }

    pub(super) fn chain_view(&self) -> &ChainViewId {
        self.token.chain_view()
    }

    pub(super) fn resolved(
        self,
        evidence: ResolutionEvidence,
    ) -> Result<ContinuousResolution, ReceiptFailure<ResolutionReceiptError>> {
        let payload = match build_resolved_payload(&self.token, &self.tx, evidence) {
            Ok(payload) => payload,
            Err(error) => return Err(ReceiptFailure::new(self.token, error)),
        };
        let (payload, verify_class) = match payload {
            ResolvedPayloadBuild::Ready(payload, verify_class) => (payload, verify_class),
            ResolvedPayloadBuild::ResourceDenied => {
                return Ok(ContinuousResolution::Settle(budget_denied(self.token)));
            }
            ResolvedPayloadBuild::Rejected(rejection) => {
                return Ok(ContinuousResolution::Settle(self.token.settle(
                    SettlementNext::Rejected(SettlementRejection::chain_bound(rejection)),
                )));
            }
        };
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

    pub(super) fn rejected(self, reason: impl Into<CommittedPublicReject>) -> ComputeSettlement {
        self.token
            .settle(SettlementNext::Rejected(SettlementRejection::chain_bound(
                reason,
            )))
    }

    pub(super) fn internal_failure(self) -> ComputeSettlement {
        internal_failure(self.token)
    }

    pub(super) fn resource_denied(self) -> ComputeSettlement {
        budget_denied(self.token)
    }
}

impl VerifyWork {
    #[expect(
        clippy::result_large_err,
        reason = "a stale snapshot returns the exact move-only settlement capability; boxing would allocate at the worker freshness boundary"
    )]
    pub(super) fn bind_current(
        self,
        snapshot_tip: &ckb_types::packed::Byte32,
    ) -> Result<SnapshotBoundVerifyWork, ComputeSettlement> {
        if snapshot_tip != &self.token.chain_view().tip().0
            || self.resolved.chain_view() != self.token.chain_view()
        {
            return Err(internal_failure(self.token));
        }
        Ok(SnapshotBoundVerifyWork {
            token: self.token,
            resolved: self.resolved,
        })
    }
}

impl ContinuousVerifyWork {
    pub(super) fn into_current(self) -> SnapshotBoundVerifyWork {
        // Continuous verification consumes facts produced from the same
        // snapshot-bound token without an intervening authority checkout.
        SnapshotBoundVerifyWork {
            token: self.token,
            resolved: self.resolved,
        }
    }
}

impl SnapshotBoundVerifyWork {
    pub(super) fn transaction(&self) -> &TransactionView {
        &self.resolved.payload().resolved_transaction().transaction
    }

    pub(super) fn payload_policy(&self) -> PayloadPolicy {
        self.token.payload_policy
    }

    pub(super) fn resolved_transaction(&self) -> &Arc<ResolvedTransaction> {
        self.resolved.payload().resolved_transaction()
    }

    /// Seal post-script time/rules evidence into the exact resolved payload
    /// owned by this current-view compute capability. Location and view cannot
    /// be supplied independently by a runtime caller.
    pub(super) fn verified_with_time_context(
        self,
        cycles: u64,
        time: TimeContextReceipt,
        async_process_start: AsyncProcessStart,
    ) -> ComputeSettlement {
        verified(self.token, self.resolved, cycles, time, async_process_start)
    }

    pub(super) fn internal_failure(self) -> ComputeSettlement {
        internal_failure(self.token)
    }

    pub(super) fn rejected(self, reason: impl Into<CommittedPublicReject>) -> ComputeSettlement {
        self.token.settle(SettlementNext::VerificationRejected {
            rejection: reason.into(),
            resolved: self.resolved,
        })
    }
}

impl CheckedOutWork {
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
                Ok(Self::Verify(VerifyWork { token, resolved }))
            }
            _ => Err(WorkPermitMismatch::Incompatible),
        }
    }
}

#[cfg(test)]
#[path = "tests/support/work.rs"]
mod test_support;
