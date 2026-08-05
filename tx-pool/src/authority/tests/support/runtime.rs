use super::super::state::ValidatedAdmission;
use super::*;

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
            let signal = match permit {
                WorkPermit::ResolveOnly => self.resolver_signal(),
                WorkPermit::VerifyOnly(capability) | WorkPermit::ResolveThenVerify(capability) => {
                    self.verifier_signal(capability)
                }
            };
            let notified = signal.notified();
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
            tokio::select! {
                _ = cancel.cancelled() => return Ok(ControlFlow::Continue(None)),
                _ = notified => {}
            }
        }
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
