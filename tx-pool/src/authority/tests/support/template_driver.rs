use super::*;

impl AuthorityBlockAssembler {
    pub(in crate::authority) async fn drive_replacement_once(
        &self,
    ) -> Result<AuthorityTemplateStep, AuthorityTemplateDriverFault> {
        self.attempt_replacement_once()
            .await
            .map_err(|failure| failure.error)
    }

    pub(in crate::authority) async fn drive_component_once(
        &self,
        component: TemplateComponent,
    ) -> Result<AuthorityTemplateStep, AuthorityTemplateDriverFault> {
        self.attempt_component_once(component)
            .await
            .map_err(|failure| failure.error)
    }

    async fn component_retry_source(&self, component: TemplateComponent) -> TemplateRetrySourceCut {
        let current = self.assembler.current.read().await;
        let revision = current.revision;
        drop(current);
        let pool_versions = self.runtime.template_source_versions();
        let pool = TemplatePoolSourceCut::new(pool_versions);
        let source = match component {
            TemplateComponent::Proposals => TemplateRetrySource::Proposals {
                source: pool.proposal_cut(),
                revision,
            },
            TemplateComponent::Transactions => TemplateRetrySource::Transactions {
                source: pool.transaction_cut(),
                revision,
            },
            TemplateComponent::Uncles => TemplateRetrySource::Uncles {
                source: TemplateSourceCut::new(
                    pool_versions,
                    self.assembler.candidate_uncles.lock().source_receipt(),
                )
                .uncle_cut(),
                revision,
            },
        };
        TemplateRetrySourceCut { source }
    }

    pub(in crate::authority) async fn run_notification_lane_for_foundation(
        self,
        cancel: CancellationToken,
    ) -> Result<(), AuthorityTemplateDriverFault> {
        self.run_notification_lane(cancel, true).await
    }

    pub(in crate::authority) async fn component_retry_source_for_foundation(
        &self,
        component: TemplateComponent,
    ) -> TemplateRetrySourceCut {
        self.component_retry_source(component).await
    }

    pub(in crate::authority) async fn wait_template_source_change_for_foundation(
        &self,
        cancel: &CancellationToken,
        failed: TemplateRetrySourceCut,
    ) -> bool {
        self.next_template_source_after_failure(cancel, failed)
            .await
            .is_some()
    }

    pub(in crate::authority) async fn prepare_component_for_foundation(
        &self,
        component: TemplateComponent,
    ) -> Result<Option<PreparedPartial>, AuthorityTemplateDriverFault> {
        let input = self.runtime.template_input()?;
        let current = self.assembler.current.read().await.clone();
        if current.snapshot.tip_hash() != input.snapshot().tip_hash() {
            return Ok(None);
        }
        match component {
            TemplateComponent::Proposals => self.prepare_proposals(&input, current),
            TemplateComponent::Transactions => self.prepare_transactions(&input, current),
            TemplateComponent::Uncles => self.prepare_uncles(&input, current),
        }
    }

    pub(in crate::authority) async fn prepare_reset_for_foundation(
        &self,
    ) -> Result<Option<PreparedReset>, AuthorityTemplateDriverFault> {
        let input = self.runtime.template_input()?;
        let current = self.assembler.current.read().await.clone();
        if current.snapshot.tip_hash() == input.snapshot().tip_hash() {
            return Ok(None);
        }
        self.prepare_reset(input, current.reset_epoch)
    }

    pub(in crate::authority) async fn publish_component_for_foundation(
        &self,
        prepared: PreparedPartial,
    ) -> Result<AuthorityTemplateStep, AuthorityTemplateDriverFault> {
        self.publish_partial(prepared).await
    }

    pub(in crate::authority) async fn publish_reset_for_foundation(
        &self,
        prepared: PreparedReset,
    ) -> Result<AuthorityTemplateStep, AuthorityTemplateDriverFault> {
        self.publish_reset(prepared).await
    }

    pub(in crate::authority) async fn is_converged_for_foundation(&self) -> bool {
        let published_reset = self.assembler.current.read().await.reset_epoch;
        self.convergence.lock().is_converged(published_reset)
    }
}
