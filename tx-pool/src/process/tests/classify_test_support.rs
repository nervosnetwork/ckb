use super::*;
use crate::component::pre_pool::PrePoolFault;
use crate::service::TxPoolService;

impl TxPoolService {
    pub(crate) async fn settle_ingress_fault_for_test(
        &self,
        tx: TransactionView,
        source: TxSource,
        fault: PrePoolFault,
    ) -> Result<bool, Reject> {
        self.finish_pipeline_admission(
            tx,
            source,
            Err(PipelineAdmissionFailure::Fault(
                TxPoolGenerationFault::PrePool(fault),
            )),
        )
        .await
    }
}
