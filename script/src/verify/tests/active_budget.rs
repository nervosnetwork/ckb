use super::super::{VmActiveTimeBudget, VmSlicePhase, VmSliceState};
use std::time::{Duration, Instant};

fn end_slice(
    state: VmSliceState,
    started_at: Instant,
    elapsed: Duration,
    finished: bool,
) -> VmSliceState {
    state
        .begin(started_at)
        .expect("the fixture slice starts from a non-running receipt")
        .end(started_at + elapsed, finished)
        .expect("the fixture ends its exact running receipt")
}

#[test]
fn cache_hit_and_pre_vm_delay_charge_no_active_time() {
    let budget = VmActiveTimeBudget::new(Duration::from_millis(10));
    let child_started = Instant::now() + Duration::from_secs(30);

    assert_eq!(
        budget.state.charged,
        Duration::ZERO,
        "a cache hit has no slice"
    );
    assert_eq!(budget.remaining(), Duration::from_millis(10));

    let running = budget
        .state
        .begin(child_started)
        .expect("the first VM preparation starts one slice");
    let mut running_budget = budget;
    running_budget.observe(running);
    assert_eq!(
        running_budget.timer_deadline(),
        Some((1, child_started + Duration::from_millis(10))),
        "queue, resolution, assignment, and Tokio delay before child start are excluded"
    );
}

#[test]
fn suspend_idle_gap_is_not_charged() {
    let started_at = Instant::now();
    let mut budget = VmActiveTimeBudget::new(Duration::from_millis(10));
    let first = end_slice(budget.state, started_at, Duration::from_millis(3), false);
    budget.observe(first);

    let resumed_at = started_at + Duration::from_secs(60);
    let second = end_slice(budget.state, resumed_at, Duration::from_millis(2), true);
    budget.observe(second);

    assert_eq!(budget.state.charged, Duration::from_millis(5));
    assert_eq!(budget.remaining(), Duration::from_millis(5));
}

#[test]
fn script_groups_share_one_cumulative_budget() {
    let started_at = Instant::now();
    let mut budget = VmActiveTimeBudget::new(Duration::from_millis(10));
    let first_group = end_slice(budget.state, started_at, Duration::from_millis(4), true);
    budget.observe(first_group);

    let second_group = end_slice(
        budget.state.next_group_idle(),
        started_at + Duration::from_secs(1),
        Duration::from_millis(6),
        true,
    );
    budget.observe(second_group);

    assert_eq!(budget.state.seq, 2);
    assert_eq!(budget.state.charged, Duration::from_millis(10));
    assert!(budget.exceeded());
}

#[test]
fn child_receipt_decides_completion_timer_race() {
    let started_at = Instant::now();
    let mut completed = VmActiveTimeBudget::new(Duration::from_millis(10));
    let running = completed
        .state
        .begin(started_at)
        .expect("the fixture starts one slice");
    completed.observe(running);
    assert!(completed.timer_still_applies(running.seq));

    let finished_before_timer = running
        .end(started_at + Duration::from_millis(9), true)
        .expect("the child publishes its completion timestamp");
    completed.observe(finished_before_timer);
    assert!(!completed.timer_still_applies(running.seq));
    assert!(!completed.exceeded());

    let mut exact = VmActiveTimeBudget::new(Duration::from_millis(10));
    let finished_at_timer = end_slice(exact.state, started_at, Duration::from_millis(10), true);
    exact.observe(finished_at_timer);
    assert!(exact.exceeded(), "the exact boundary is exhausted");

    let mut late = VmActiveTimeBudget::new(Duration::from_millis(10));
    let finished_after_timer = end_slice(late.state, started_at, Duration::from_millis(11), true);
    assert_eq!(finished_after_timer.phase, VmSlicePhase::Finished);
    late.observe(finished_after_timer);
    assert!(
        late.exceeded(),
        "a coalesced Finished receipt still preserves the charge"
    );
}
