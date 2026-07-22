use crate::component::{
    dependency_scheduler::{
        DependencyError, DependencyLimits, DependencyScheduler, DependencyState,
    },
    lifecycle_store::PipelineStage,
};
use ckb_types::packed::Byte32;
use std::collections::{HashMap, HashSet};

fn hash(seed: u8) -> Byte32 {
    Byte32::new([seed; 32])
}

fn set<const N: usize>(items: [Byte32; N]) -> HashSet<Byte32> {
    HashSet::from(items)
}

fn roomy_scheduler() -> DependencyScheduler {
    DependencyScheduler::new(DependencyLimits::new(100, 1_000, 100))
}

#[test]
fn final_parent_event_wakes_child_exactly_once() {
    let mut scheduler = roomy_scheduler();
    let parent_a = hash(1);
    let parent_b = hash(2);
    let child = hash(3);
    scheduler
        .park(
            child.clone(),
            set([parent_a.clone(), parent_b.clone()]),
            set([parent_a.clone(), parent_b.clone()]),
        )
        .unwrap();

    assert!(scheduler.parent_available(&parent_a).is_empty());
    assert!(scheduler.pop_ready().is_none());
    let woken = scheduler.parent_available(&parent_b);
    assert_eq!(woken.len(), 1);
    assert!(scheduler.parent_available(&parent_b).is_empty());

    let ticket = scheduler.pop_ready().unwrap();
    assert_eq!(ticket.hash, child);
    assert!(scheduler.pop_ready().is_none());
    scheduler.complete(&ticket).unwrap();
    assert_eq!(scheduler.len(), 0);
    scheduler.audit().unwrap();
}

/// Historical bug #15: once all parents are ready, downstream queue
/// saturation must create an event-driven capacity wait, not an orphan that
/// can only be revisited by polling or expiry.
#[test]
fn queue_full_wait_is_woken_by_capacity_event() {
    let mut scheduler = roomy_scheduler();
    let parent = hash(10);
    let child = hash(11);
    scheduler
        .park(child.clone(), set([parent.clone()]), set([parent.clone()]))
        .unwrap();
    scheduler.parent_available(&parent);
    let ticket = scheduler.pop_ready().unwrap();

    scheduler
        .block_on_capacity(&ticket, PipelineStage::Verify)
        .unwrap();
    assert_eq!(
        scheduler.view(&child).unwrap().state,
        DependencyState::CapacityBlocked(PipelineStage::Verify)
    );
    assert!(scheduler.pop_ready().is_none());
    assert!(scheduler.parent_available(&parent).is_empty());

    assert!(
        scheduler
            .capacity_available(PipelineStage::Resolve, 1)
            .is_empty()
    );
    let woken = scheduler.capacity_available(PipelineStage::Verify, 1);
    assert_eq!(woken, vec![ticket.clone()]);
    let retried = scheduler.pop_ready().unwrap();
    assert_eq!(retried, ticket);
    scheduler.complete(&retried).unwrap();
    scheduler.audit().unwrap();
}

#[test]
fn parent_unavailable_invalidates_dispatched_ticket() {
    let mut scheduler = roomy_scheduler();
    let parent = hash(20);
    let child = hash(21);
    scheduler
        .park(child.clone(), set([parent.clone()]), HashSet::new())
        .unwrap();
    let stale = scheduler.pop_ready().unwrap();

    assert_eq!(
        scheduler.parent_unavailable(&parent).unwrap(),
        vec![child.clone()]
    );
    assert!(matches!(
        scheduler.complete(&stale),
        Err(DependencyError::StaleTicket { .. })
    ));
    assert!(matches!(
        scheduler.return_ready(&stale),
        Err(DependencyError::StaleTicket { .. })
    ));
    assert!(matches!(
        scheduler.view(&child).unwrap().state,
        DependencyState::Waiting { .. }
    ));

    let woken = scheduler.parent_available(&parent);
    assert_eq!(woken.len(), 1);
    assert_ne!(woken[0].generation, stale.generation);
    let fresh = scheduler.pop_ready().unwrap();
    scheduler.complete(&fresh).unwrap();
    scheduler.audit().unwrap();
}

#[test]
fn failed_dispatch_can_be_returned_without_duplication() {
    let mut scheduler = roomy_scheduler();
    let child = hash(30);
    scheduler
        .park(child.clone(), HashSet::new(), HashSet::new())
        .unwrap();
    let ticket = scheduler.pop_ready().unwrap();
    scheduler.return_ready(&ticket).unwrap();
    assert_eq!(
        scheduler.return_ready(&ticket),
        Err(DependencyError::StateMismatch {
            hash: child,
            expected: "dispatched",
            actual: DependencyState::Ready,
        })
    );
    assert_eq!(scheduler.pop_ready().unwrap(), ticket);
    assert!(scheduler.pop_ready().is_none());
    scheduler.audit().unwrap();
}

#[test]
fn definitive_parent_failure_cascades_ready_and_blocked_descendants() {
    let mut scheduler = roomy_scheduler();
    let root = hash(40);
    let child = hash(41);
    let sibling = hash(42);
    let grandchild = hash(43);

    scheduler
        .park(child.clone(), set([root.clone()]), set([root.clone()]))
        .unwrap();
    scheduler
        .park(sibling.clone(), set([root.clone()]), set([root.clone()]))
        .unwrap();
    scheduler
        .park(
            grandchild.clone(),
            set([child.clone()]),
            set([child.clone()]),
        )
        .unwrap();
    scheduler.parent_available(&root);
    let dispatched = scheduler.pop_ready().unwrap();
    scheduler
        .block_on_capacity(&dispatched, PipelineStage::Verify)
        .unwrap();

    let failures = scheduler.parent_failed(&root);
    let by_hash: HashMap<_, _> = failures
        .into_iter()
        .map(|failure| (failure.hash, failure.failed_parent))
        .collect();
    assert_eq!(by_hash.get(&child), Some(&root));
    assert_eq!(by_hash.get(&sibling), Some(&root));
    assert_eq!(by_hash.get(&grandchild), Some(&child));
    assert_eq!(scheduler.len(), 0);
    assert_eq!(scheduler.edge_count(), 0);
    assert!(scheduler.pop_ready().is_none());
    assert!(
        scheduler
            .capacity_available(PipelineStage::Verify, usize::MAX)
            .is_empty()
    );
    scheduler.audit().unwrap();
}

#[test]
fn failed_reclassification_preserves_old_edges_and_ticket() {
    let mut scheduler = DependencyScheduler::new(DependencyLimits::new(2, 2, 2));
    let child = hash(50);
    let old_parent = hash(51);
    let old_ticket = scheduler
        .park(
            child.clone(),
            set([old_parent.clone()]),
            set([old_parent.clone()]),
        )
        .unwrap();
    let old_view = scheduler.view(&child).unwrap();

    assert_eq!(
        scheduler.park(
            child.clone(),
            set([hash(52), hash(53), hash(54)]),
            HashSet::new(),
        ),
        Err(DependencyError::PerEntryEdgeLimitExceeded)
    );
    assert_eq!(scheduler.view(&child).unwrap(), old_view);
    assert_eq!(scheduler.edge_count(), 1);
    let woken = scheduler.parent_available(&old_parent);
    assert_eq!(woken, vec![old_ticket]);
    scheduler.audit().unwrap();
}

#[test]
fn count_and_edge_limits_bound_id_only_state() {
    let mut scheduler = DependencyScheduler::new(DependencyLimits::new(2, 3, 2));
    scheduler
        .park(hash(60), set([hash(61), hash(62)]), HashSet::new())
        .unwrap();
    scheduler
        .park(hash(63), set([hash(64)]), HashSet::new())
        .unwrap();

    assert_eq!(
        scheduler.park(hash(65), HashSet::new(), HashSet::new()),
        Err(DependencyError::EntryLimitExceeded)
    );
    assert_eq!(
        scheduler.park(hash(63), set([hash(66), hash(67)]), HashSet::new()),
        Err(DependencyError::EdgeLimitExceeded)
    );
    assert_eq!(scheduler.len(), 2);
    assert_eq!(scheduler.edge_count(), 3);
    scheduler.audit().unwrap();
}

#[test]
fn stale_capacity_ticket_cannot_wake_reparked_child() {
    let mut scheduler = roomy_scheduler();
    let child = hash(70);
    scheduler
        .park(child.clone(), HashSet::new(), HashSet::new())
        .unwrap();
    let stale = scheduler.pop_ready().unwrap();
    scheduler
        .block_on_capacity(&stale, PipelineStage::Resolve)
        .unwrap();

    let fresh = scheduler
        .park(child.clone(), HashSet::new(), HashSet::new())
        .unwrap();
    let woken = scheduler.capacity_available(PipelineStage::Resolve, usize::MAX);
    assert!(woken.is_empty());
    assert_eq!(scheduler.pop_ready().unwrap(), fresh);
    assert_ne!(fresh.generation, stale.generation);
    scheduler.audit().unwrap();
}

#[test]
fn clear_removes_edges_ready_and_capacity_waiters() {
    let mut scheduler = roomy_scheduler();
    scheduler
        .park(hash(80), HashSet::new(), HashSet::new())
        .unwrap();
    scheduler
        .park(hash(81), HashSet::new(), HashSet::new())
        .unwrap();
    let ticket = scheduler.pop_ready().unwrap();
    scheduler
        .block_on_capacity(&ticket, PipelineStage::Resolve)
        .unwrap();
    scheduler.clear();

    assert_eq!(scheduler.len(), 0);
    assert_eq!(scheduler.edge_count(), 0);
    assert!(scheduler.pop_ready().is_none());
    assert!(
        scheduler
            .capacity_available(PipelineStage::Resolve, usize::MAX)
            .is_empty()
    );
    scheduler.audit().unwrap();
}
