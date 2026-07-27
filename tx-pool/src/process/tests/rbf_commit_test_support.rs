use super::*;

pub(crate) struct TestAdmissionOutcome {
    pub(crate) result: Result<(), Reject>,
    pub(crate) assembler_statuses: HashSet<Status>,
}

impl TxPoolService {
    pub(crate) fn try_submit_entry(
        &self,
        tx_pool: &mut TxPool,
        snapshot: Arc<Snapshot>,
        pre_resolve_tip: Byte32,
        entry: TxEntry,
        _status: Status,
        _entry_id: ProposalShortId,
    ) -> TestAdmissionOutcome {
        self.pipeline.kernel.mutate_authoritative(|kernel| {
            match self.plan_external_admission(
                tx_pool,
                kernel,
                snapshot,
                pre_resolve_tip,
                entry,
                TxSource::Local,
                None,
                self.current_pipeline_epoch()
                    .expect("test admission has a live pipeline epoch"),
            ) {
                Err(AdmissionPlanningError::Policy(reject)) => TestAdmissionOutcome {
                    result: Err(reject),
                    assembler_statuses: HashSet::new(),
                },
                Err(AdmissionPlanningError::Kernel(error)) => TestAdmissionOutcome {
                    result: Err(Reject::Internal(format!(
                        "pre-pool planning fault in test seam: {error:?}"
                    ))),
                    assembler_statuses: HashSet::new(),
                },
                Err(AdmissionPlanningError::Pool(error)) => TestAdmissionOutcome {
                    result: Err(Reject::Internal(format!(
                        "accepted-pool planning fault: {error:?}"
                    ))),
                    assembler_statuses: HashSet::new(),
                },
                Err(AdmissionPlanningError::Effect(error)) => TestAdmissionOutcome {
                    result: Err(Reject::Internal(format!(
                        "effect batch planning fault: {error:?}"
                    ))),
                    assembler_statuses: HashSet::new(),
                },
                Ok(plan) => {
                    let assembler_statuses = plan.block_assembler_statuses.clone();
                    self.apply_admission_plan(plan).unwrap_or_else(|error| {
                        panic!("test admission journal unexpectedly unavailable: {error:?}")
                    });
                    TestAdmissionOutcome {
                        result: Ok(()),
                        assembler_statuses,
                    }
                }
            }
        })
    }
}
