use super::*;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

pub(in crate::authority) async fn run_maintenance_driver_for_foundation(
    runtime: AuthorityRuntime,
    cancel: CancellationToken,
    rounds: Arc<AtomicUsize>,
) -> Result<(), AuthorityWorkerFault> {
    run_maintenance_driver_loop(runtime, cancel, move || {
        rounds.fetch_add(1, AtomicOrdering::Relaxed);
    })
    .await
}
