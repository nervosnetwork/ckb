use super::*;

impl AuthorityShutdownReport {
    pub(in crate::authority) fn persistence_eligible(&self) -> bool {
        matches!(self.status, AuthorityShutdownStatus::PersistenceEligible)
    }
}

impl AuthorityTaskTopology {
    pub(in crate::authority) fn install_template_task_for_foundation(
        &mut self,
        task: AuthorityTemplateTask,
    ) {
        self.templates = Some([Some(task), None, None, None, None]);
    }
}
