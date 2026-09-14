use super::super::{TxPoolVmExecutionMode, VmSlicePhase, VmSliceState, VmVerificationBudget};
use std::time::{Duration, Instant};

const LIMIT: Duration = Duration::from_millis(10);

fn running(charged: Duration, started_at: Instant) -> VmSliceState {
    VmSliceState {
        charged,
        phase: VmSlicePhase::Running { started_at },
    }
}

#[test]
fn idle_time_does_not_charge_or_arm_a_deadline() {
    let idle = VmSliceState::default();
    let child_started = Instant::now() + Duration::from_secs(30);
    assert_eq!(idle.deadline(LIMIT), None);
    assert_eq!(idle.charged, Duration::ZERO);
    assert_eq!(
        running(Duration::ZERO, child_started).deadline(LIMIT),
        Some(child_started + LIMIT),
        "the timer starts with VM work, not queueing, resolution or task scheduling"
    );
}

#[test]
fn suspend_gap_and_coalesced_slices_preserve_only_running_time() {
    let started_at = Instant::now();
    let (sender, mut receiver) = tokio::sync::watch::channel(VmSliceState::default());
    sender.send_replace(running(Duration::ZERO, started_at));
    let paused = VmSliceState {
        charged: Duration::from_millis(3),
        phase: VmSlicePhase::Idle,
    };
    sender.send_replace(paused);
    let resumed_at = started_at + Duration::from_secs(60);
    assert_eq!(paused.deadline(LIMIT), None);
    sender.send_replace(running(paused.charged, resumed_at));
    sender.send_replace(VmSliceState {
        charged: Duration::from_millis(5),
        phase: VmSlicePhase::Finished,
    });
    let latest = *receiver.borrow_and_update();
    let mut budget = VmVerificationBudget::new(LIMIT, TxPoolVmExecutionMode::Inline);
    budget.charge(latest.charged);
    assert_eq!(budget.remaining, Duration::from_millis(5));
}

#[test]
fn script_groups_share_one_remaining_budget() {
    let started_at = Instant::now();
    let mut budget = VmVerificationBudget::new(LIMIT, TxPoolVmExecutionMode::Inline);
    budget.charge(Duration::from_millis(4));
    assert_eq!(budget.remaining, Duration::from_millis(6));
    let next_group = running(Duration::ZERO, started_at + Duration::from_secs(1));
    assert_eq!(
        next_group.deadline(budget.remaining),
        Some(started_at + Duration::from_secs(1) + Duration::from_millis(6))
    );
    budget.charge(Duration::from_millis(6));
    assert!(
        budget.remaining.is_zero(),
        "the exact shared boundary is exhausted"
    );
}

#[test]
fn finished_receipt_decides_completion_timer_race() {
    for (milliseconds, exceeded) in [(9, false), (10, true), (11, true)] {
        let finished = VmSliceState {
            charged: Duration::from_millis(milliseconds),
            phase: VmSlicePhase::Finished,
        };
        assert_eq!(finished.deadline(LIMIT), None);
        let mut budget = VmVerificationBudget::new(LIMIT, TxPoolVmExecutionMode::Inline);
        budget.charge(finished.charged);
        assert_eq!(
            budget.remaining.is_zero(),
            exceeded,
            "a delayed parent observes the child's completion time, not its own wake time"
        );
    }
}

#[test]
fn stale_timer_rechecks_the_current_slice_after_resume() {
    let started_at = Instant::now();
    let old_deadline = running(Duration::ZERO, started_at).deadline(LIMIT).unwrap();
    let resumed_at = started_at + Duration::from_secs(60);
    let resumed = running(Duration::from_millis(3), resumed_at);
    let delayed_wake = resumed_at + Duration::from_millis(1);
    assert!(delayed_wake > old_deadline);
    let current_deadline = resumed.deadline(LIMIT).unwrap();
    assert_eq!(current_deadline, resumed_at + Duration::from_millis(7));
    assert!(
        delayed_wake < current_deadline,
        "the old timer cannot stop a new slice early"
    );
}
