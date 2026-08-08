use super::*;
use std::convert::Infallible;

/// Test-only materialized observation of the production recovery visitor.
/// Production never constructs this large enum; the allocation-free visitor
/// remains the sole capability-routing implementation.
#[expect(
    clippy::large_enum_variant,
    reason = "test-only recovery observation preserves direct pattern assertions without changing the production failure path"
)]
pub(in crate::authority) enum ComputeExchangeRecovery {
    Settlement(ComputeExchangeCompletion),
    Obsolete(ComputeWorkerSlot),
    Grant(ComputeWorkerGrant),
}

struct RecoveryCollector {
    observed: Vec<ComputeExchangeRecovery>,
}

impl ComputeExchangeRecoverySink for RecoveryCollector {
    type Error = Infallible;

    fn recover_settlement(
        &mut self,
        completion: ComputeExchangeCompletion,
    ) -> Result<(), Self::Error> {
        self.observed
            .push(ComputeExchangeRecovery::Settlement(completion));
        Ok(())
    }

    fn recover_obsolete(&mut self, slot: ComputeWorkerSlot) -> Result<(), Self::Error> {
        self.observed.push(ComputeExchangeRecovery::Obsolete(slot));
        Ok(())
    }

    fn recover_grant(&mut self, grant: ComputeWorkerGrant) -> Result<(), Self::Error> {
        self.observed.push(ComputeExchangeRecovery::Grant(grant));
        Ok(())
    }
}

impl ComputeExchangePlanFailure {
    pub(in crate::authority) fn into_parts(
        self,
    ) -> (PlanError, std::vec::IntoIter<ComputeExchangeRecovery>) {
        let (error, recoveries) = self.into_recovery();
        let mut collector = RecoveryCollector {
            observed: Vec::new(),
        };
        match recoveries.recover_into(&mut collector) {
            Ok(()) => (error, collector.observed.into_iter()),
            Err(never) => match never {},
        }
    }
}

impl ComputeExchangeCompletion {
    pub(in crate::authority) fn new(
        slot: ComputeWorkerSlot,
        settlement: ComputeSettlement,
    ) -> Self {
        Self::from_finished(
            slot,
            AuthorityFinishedCompute::from_parts(
                settlement,
                AuthorityComputeAftermath::completed_without_cache(),
            ),
        )
    }
}

impl ComputeExchangeDeferred {
    pub(in crate::authority) fn version(&self) -> EntryVersion {
        self.completion.version()
    }
}
