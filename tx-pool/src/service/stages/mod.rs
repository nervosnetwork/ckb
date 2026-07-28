//! Asynchronous resolve and verify pipeline stages.

pub(crate) mod resolve;
mod runner;
mod verify;

pub(crate) use resolve::spawn_ordered_resolver;
pub(crate) use verify::spawn_verify_workers;

fn finish_continuation<T>(
    service: &crate::service::TxPoolService,
    applied: crate::component::pre_pool::AppliedContinuation<T>,
    fault_context: &'static str,
) -> Option<T> {
    match applied.into_checkout() {
        Ok(next) => next,
        Err(error) => {
            service.fail_tx_pool_generation(
                fault_context,
                &crate::process::TxPoolGenerationFault::PrePool(error.into_unexpected_fault()),
            );
            None
        }
    }
}
