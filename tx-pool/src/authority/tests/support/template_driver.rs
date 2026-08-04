use super::*;

impl AuthorityBlockAssembler {
    pub(in crate::authority) async fn run_notification_lane_for_foundation(
        self,
        cancel: CancellationToken,
    ) -> Result<(), AuthorityTemplateDriverFault> {
        self.run_notification_lane(cancel, true).await
    }

    pub(in crate::authority) async fn retry_source_cut_for_foundation(
        &self,
    ) -> TemplateRetrySourceCut {
        self.retry_source_cut().await
    }

    pub(in crate::authority) async fn wait_template_source_change_for_foundation(
        &self,
        cancel: &CancellationToken,
        failed: TemplateRetrySourceCut,
    ) -> bool {
        self.wait_template_source_change(cancel, failed)
            .await
            .is_some()
    }

    pub(in crate::authority) async fn prepare_full_for_foundation(
        &self,
    ) -> Result<Option<PreparedFull>, AuthorityTemplateDriverFault> {
        let input = self.runtime.template_input()?;
        let current = self.assembler.current.read().await.clone();
        if current.snapshot.tip_hash() != input.snapshot().tip_hash() {
            return Ok(None);
        }
        self.prepare_full(input, current)
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

    pub(in crate::authority) async fn publish_full_for_foundation(
        &self,
        prepared: PreparedFull,
    ) -> Result<AuthorityTemplateStep, AuthorityTemplateDriverFault> {
        self.publish_full(prepared).await
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
