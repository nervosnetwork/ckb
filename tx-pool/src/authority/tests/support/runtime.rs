use super::super::state::{ValidatedAdmission, VerifyCapability};
use super::super::{
    ingress::{
        RemoteIngressPressure, RetainedIngress, RetainedIngressBoundaryError, RetainedIngressError,
        RetainedIngressRejection, proposal, remote, remote_pressure_rejection,
        test_support::{IngressRejectionCommit, RetainedIngressCommit},
    },
    plan::test_support::RetainedAdmissionDisposition,
};
use super::*;
use ckb_network::PeerIndex;
use ckb_types::core::Cycle;

impl ComputeGate {
    fn try_acquire(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.permits).try_acquire_owned().ok()
    }

    fn available_permits(&self) -> usize {
        self.permits.available_permits()
    }
}

impl AuthorityComputeJob {
    pub(in crate::authority) fn retry_for_foundation(self) -> AuthorityComputeSettlement {
        let settlement = match self.inner {
            AuthorityComputeKind::Resolution(job) => job.retry_for_foundation(),
            AuthorityComputeKind::Verification(job) => job.retry(),
        };
        AuthorityComputeSettlement {
            settlement,
            execution: self.execution,
        }
    }
}

#[derive(Debug)]
pub(in crate::authority) enum FoundationCheckoutError {
    ComputeCapacity,
    Authority(AuthorityComputeError),
}

impl From<AuthorityComputeError> for FoundationCheckoutError {
    fn from(error: AuthorityComputeError) -> Self {
        Self::Authority(error)
    }
}

impl AuthorityPendingSettlement {
    pub(in crate::authority) fn recovery(&self) -> &super::super::plan::ComputeSettlementRecovery {
        self.failure.recovery()
    }
}

impl AuthorityDirectRejection {
    pub(in crate::authority) fn reason(&self) -> &CommittedPublicReject {
        self.rejection.reason()
    }
}

impl AuthorityDirectVerifiedCandidate {
    pub(in crate::authority) fn with_cache_update_for_foundation(
        mut self,
        key: ckb_verification::cache::TxVerificationCacheKey,
        completed: ckb_verification::cache::Completed,
    ) -> Self {
        self.candidate = self
            .candidate
            .with_cache_update_for_foundation(key, completed);
        self
    }
}

impl AuthorityRuntime {
    pub(crate) fn new(
        config: &TxPoolConfig,
        consensus: &Consensus,
        snapshot: Arc<Snapshot>,
    ) -> Result<Self, RuntimeConfigError> {
        let runtime = AuthorityRuntimeConfig::from_runtime(config, consensus)?;
        Self::from_config(runtime, snapshot)
    }

    pub(in crate::authority) fn new_with_effect_limits_for_foundation(
        config: &TxPoolConfig,
        consensus: &Consensus,
        snapshot: Arc<Snapshot>,
        effects: EffectLimits,
    ) -> Result<Self, RuntimeConfigError> {
        let mut runtime = AuthorityRuntimeConfig::from_runtime(config, consensus)?;
        runtime.effects = effects;
        Self::from_config(runtime, snapshot)
    }

    pub(in crate::authority) fn commit_retained_ingress(
        &self,
        ingress: RetainedIngress,
    ) -> Result<RetainedIngressCommit, PlanError> {
        let (outcome, committed) = {
            let mut store = self.store.write();
            match store.authority.plan_retained_admission(ingress)? {
                RetainedAdmissionDisposition::ProposalUnchanged => {
                    return Ok(RetainedIngressCommit::ProposalUnchanged);
                }
                RetainedAdmissionDisposition::ProposalPayloadVariant => {
                    return Ok(RetainedIngressCommit::ProposalPayloadVariant);
                }
                RetainedAdmissionDisposition::Retained(plan) => {
                    (RetainedIngressCommit::Retained, plan.apply())
                }
                RetainedAdmissionDisposition::AcceptedDuplicate(plan) => {
                    (RetainedIngressCommit::AcceptedDuplicate, plan.apply())
                }
                RetainedAdmissionDisposition::RemoteReleased(plan) => {
                    (RetainedIngressCommit::RemoteReleased, plan.apply())
                }
            }
        };
        self.publish_committed(committed);
        Ok(outcome)
    }

    pub(in crate::authority) fn commit_retained_ingress_rejection(
        &self,
        rejection: RetainedIngressRejection,
    ) -> Result<IngressRejectionCommit, PlanError> {
        let committed = {
            let mut store = self.store.write();
            store
                .authority
                .plan_retained_ingress_rejection(rejection)?
                .apply()
        };
        self.publish_committed(committed);
        Ok(IngressRejectionCommit)
    }

    pub(in crate::authority) fn submit_remote_ingress(
        &self,
        tx: TransactionView,
        declared_cycles: Cycle,
        peer: PeerIndex,
    ) -> Result<RetainedIngressCommit, RetainedIngressBoundaryError> {
        let consensus = self.paired_consensus();
        match remote(tx, declared_cycles, peer, &consensus) {
            Ok(ingress) => self
                .commit_retained_ingress(ingress)
                .map_err(RetainedIngressBoundaryError::from_plan),
            Err(RetainedIngressError::Rejected(rejection)) => self
                .commit_retained_ingress_rejection(rejection)
                .map(|_| RetainedIngressCommit::Rejected)
                .map_err(RetainedIngressBoundaryError::from_plan),
            Err(RetainedIngressError::Admission(error)) => Err(
                RetainedIngressBoundaryError::from_admission_for_foundation(error),
            ),
        }
    }

    pub(in crate::authority) fn submit_proposal_ingress(
        &self,
        tx: TransactionView,
    ) -> Result<RetainedIngressCommit, RetainedIngressBoundaryError> {
        let consensus = self.paired_consensus();
        match proposal(tx, &consensus) {
            Ok(ingress) => self
                .commit_retained_ingress(ingress)
                .map_err(RetainedIngressBoundaryError::from_plan),
            Err(RetainedIngressError::Rejected(rejection)) => self
                .commit_retained_ingress_rejection(rejection)
                .map(|_| RetainedIngressCommit::Rejected)
                .map_err(RetainedIngressBoundaryError::from_plan),
            Err(RetainedIngressError::Admission(error)) => Err(
                RetainedIngressBoundaryError::from_admission_for_foundation(error),
            ),
        }
    }

    pub(in crate::authority) fn reject_remote_ingress_pressure(
        &self,
        tx: TransactionView,
        peer: PeerIndex,
        pressure: RemoteIngressPressure,
    ) -> Result<IngressRejectionCommit, RetainedIngressBoundaryError> {
        let rejection = remote_pressure_rejection(tx, peer, pressure);
        self.commit_retained_ingress_rejection(rejection)
            .map_err(RetainedIngressBoundaryError::from_plan)
    }

    pub(in crate::authority) fn template_capture_count_for_foundation(&self) -> usize {
        self.template_captures.load(Ordering::Relaxed)
    }

    pub(in crate::authority) fn try_compute_execution_for_foundation(
        &self,
    ) -> Option<AuthorityComputeExecutionPermit> {
        self.transient_compute
            .try_acquire()
            .map(|permit| AuthorityComputeExecutionPermit { _permit: permit })
    }

    pub(in crate::authority) fn available_compute_permits_for_foundation(&self) -> usize {
        self.transient_compute.available_permits()
    }

    pub(in crate::authority) fn try_checkout_for_foundation(
        &self,
        permit: WorkPermit,
    ) -> Result<
        ControlFlow<AuthorityPendingSettlement, Option<AuthorityComputeJob>>,
        FoundationCheckoutError,
    > {
        let execution = self
            .try_compute_execution_for_foundation()
            .ok_or(FoundationCheckoutError::ComputeCapacity)?;
        match self.try_checkout(permit, execution)? {
            ControlFlow::Break(pending) => Ok(ControlFlow::Break(pending)),
            ControlFlow::Continue(AuthorityComputeCheckout::Job(job)) => {
                Ok(ControlFlow::Continue(Some(job)))
            }
            ControlFlow::Continue(AuthorityComputeCheckout::Idle(execution)) => {
                drop(execution);
                Ok(ControlFlow::Continue(None))
            }
        }
    }

    pub(in crate::authority) fn normalized_snapshot_for_foundation(
        &self,
    ) -> super::super::plan::test_support::AuthoritySnapshot {
        self.store.read().authority.normalized_snapshot()
    }

    pub(in crate::authority) fn paired_chain_for_foundation(&self) -> (ChainViewId, Arc<Snapshot>) {
        let store = self.store.read();
        (
            store.authority.chain_view().clone(),
            Arc::clone(&store.snapshot),
        )
    }

    pub(in crate::authority) fn committed_hash_for_foundation(
        &self,
        proposal: &ProposalShortId,
    ) -> Option<Byte32> {
        self.store
            .write()
            .committed_txs_hash_cache
            .get(proposal)
            .cloned()
    }

    pub(in crate::authority) fn with_authority_for_foundation<T>(
        &self,
        inspect: impl FnOnce(&mut TxPoolAuthority) -> T,
    ) -> T {
        inspect(&mut self.store.write().authority)
    }

    pub(in crate::authority) fn queue_effect_for_foundation(
        &self,
        policy: super::super::effect::EffectPolicy,
        effect: super::super::effect::CommittedEffect,
    ) -> Result<(), FoundationEffectQueueError> {
        self.queue_effects_for_foundation(policy, vec![effect])
    }

    pub(in crate::authority) fn queue_effects_for_foundation(
        &self,
        policy: super::super::effect::EffectPolicy,
        effects: Vec<super::super::effect::CommittedEffect>,
    ) -> Result<(), FoundationEffectQueueError> {
        let retirement = {
            let mut store = self.store.write();
            let publication = store
                .authority
                .effect_publication_for_foundation(policy, effects)
                .map_err(FoundationEffectQueueError::Build)?;
            store
                .authority
                .plan_effect_publication_for_foundation(&publication)
                .map_err(FoundationEffectQueueError::Plan)?
                .apply()
        };
        self.publish_committed(retirement);
        Ok(())
    }

    pub(in crate::authority) fn queue_generation_reset_for_foundation(
        &self,
    ) -> Result<(), PlanError> {
        let retirement = {
            let mut store = self.store.write();
            store
                .authority
                .plan_generation_reset_for_foundation()?
                .apply()
        };
        self.publish_committed(retirement);
        Ok(())
    }

    pub(in crate::authority) fn effect_observation_for_foundation(
        &self,
    ) -> super::super::effect::test_support::EffectObservation {
        self.store
            .read()
            .authority
            .effect_observation_for_foundation()
    }

    pub(in crate::authority) fn admit(
        &self,
        admission: ValidatedAdmission,
    ) -> Result<(), PlanError> {
        let committed = {
            let mut store = self.store.write();
            store.authority.plan_admission(admission)?.apply()
        };
        self.publish_committed(committed);
        Ok(())
    }

    pub(in crate::authority) async fn wait_checkout(
        &self,
        permit: WorkPermit,
        cancel: &CancellationToken,
    ) -> Result<
        ControlFlow<AuthorityPendingSettlement, Option<AuthorityComputeJob>>,
        AuthorityComputeError,
    > {
        loop {
            // Register every level accepted by the permit before observing the
            // authority. In particular, an Any verifier shares the Small
            // signal and receives the distinct Any signal only when the two
            // scheduler heads differ.
            let resolve_notified = self.resolve_signal().notified();
            let verify_small_notified = self.verify_small_signal().notified();
            let verify_any_notified = self.verify_any_signal().notified();
            let Some(execution) = self.acquire_compute_execution(cancel).await else {
                return Ok(ControlFlow::Continue(None));
            };
            match self.try_checkout(permit, execution)? {
                ControlFlow::Break(pending) => return Ok(ControlFlow::Break(pending)),
                ControlFlow::Continue(AuthorityComputeCheckout::Job(job)) => {
                    return Ok(ControlFlow::Continue(Some(job)));
                }
                ControlFlow::Continue(AuthorityComputeCheckout::Idle(execution)) => {
                    drop(execution);
                }
            }
            let work = async {
                match permit {
                    WorkPermit::ResolveOnly => resolve_notified.await,
                    WorkPermit::ResolveThenVerify(VerifyCapability::SmallCycleOnly) => {
                        tokio::select! {
                            _ = resolve_notified => {}
                            _ = verify_small_notified => {}
                        }
                    }
                    WorkPermit::ResolveThenVerify(VerifyCapability::Any) => {
                        tokio::select! {
                            _ = resolve_notified => {}
                            _ = verify_small_notified => {}
                            _ = verify_any_notified => {}
                        }
                    }
                    WorkPermit::VerifyOnly(VerifyCapability::SmallCycleOnly) => {
                        verify_small_notified.await
                    }
                    WorkPermit::VerifyOnly(VerifyCapability::Any) => {
                        tokio::select! {
                            _ = verify_small_notified => {}
                            _ = verify_any_notified => {}
                        }
                    }
                }
            };
            tokio::select! {
                _ = cancel.cancelled() => return Ok(ControlFlow::Continue(None)),
                _ = work => {}
            }
        }
    }

    /// Test-only level probe for effect cursor tests that exercise settlement
    /// directly. Production obtains the same receipt only while mutably
    /// borrowing `AuthorityEffectPublisherClaim`.
    pub(in crate::authority) async fn wait_effect_publication_for_foundation(
        &self,
    ) -> Option<EffectReceipt> {
        loop {
            let notified = self.effect_publisher_signal().notified();
            match self.try_effect_publication() {
                EffectPublicationState::Idle => notified.await,
                EffectPublicationState::Receipt(receipt) => return Some(receipt),
                EffectPublicationState::ClosedAndDrained => return None,
            }
        }
    }

    pub(in crate::authority) fn settle_effect_for_foundation(
        &self,
        settlement: EffectSettlement,
    ) -> Result<(), EffectSettlementFailure> {
        self.settle_effect(settlement)
    }
}

#[derive(Debug)]
pub(in crate::authority) enum FoundationEffectQueueError {
    Build(
        #[expect(
            dead_code,
            reason = "the fixture preserves the exact build error for expect/panic diagnostics"
        )]
        super::super::effect::EffectBuildError,
    ),
    Plan(
        #[expect(
            dead_code,
            reason = "the fixture preserves the exact Plan error for expect/panic diagnostics"
        )]
        PlanError,
    ),
}
