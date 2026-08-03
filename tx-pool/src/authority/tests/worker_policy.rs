use super::{SettlementOrigin, WorkerStep, classify_compute_cancellation};

#[test]
fn uak_released_allocation_failure_backs_off_before_recompute() {
    assert!(matches!(
        classify_compute_cancellation(Ok(()), SettlementOrigin::Completion),
        Ok(WorkerStep::Backoff)
    ));
}
