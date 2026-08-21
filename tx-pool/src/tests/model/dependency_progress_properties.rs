use super::dependency_progress::{
    DependencyEventDisposition, DependencyMaintenanceState, DependencyMaintenanceStep,
    DependencyOwnerProgress, DependencyProgressError, ModelDependencyCut, ModelDependencyEdges,
    ModelDependencyKey, ModelDependencyOwner, ModelDirtyDependencyEpoch, ModelDirtyScope,
    ModelPendingDependencyEpoch,
};
use std::collections::{BTreeMap, BTreeSet};

fn owners(values: &[u8]) -> BTreeSet<ModelDependencyOwner> {
    values.iter().copied().map(ModelDependencyOwner).collect()
}

fn edges(values: &[(u8, &[u8])]) -> ModelDependencyEdges {
    values
        .iter()
        .map(|(key, values)| (ModelDependencyKey::cell(*key), owners(values)))
        .collect()
}

#[test]
fn model_dependency_maintenance_consumes_one_finite_obligation_per_apply() {
    let key = ModelDependencyKey::cell(1);
    let consumers = edges(&[(1, &[1, 2, 3])]);
    let waiters = edges(&[(1, &[1, 3])]);
    for scope in [
        ModelDirtyScope::ExistingWaiters,
        ModelDirtyScope::AllConsumers,
    ] {
        for cursor in [
            None,
            Some(ModelDependencyOwner(0)),
            Some(ModelDependencyOwner(1)),
            Some(ModelDependencyOwner(2)),
            Some(ModelDependencyOwner(3)),
        ] {
            for pending_scope in [
                None,
                Some(ModelDirtyScope::ExistingWaiters),
                Some(ModelDirtyScope::AllConsumers),
            ] {
                let pending = pending_scope.map(|pending_scope| ModelPendingDependencyEpoch {
                    target: ModelDependencyCut(2),
                    scope: pending_scope,
                });
                let epoch =
                    ModelDirtyDependencyEpoch::new(ModelDependencyCut(1), scope, cursor, pending)
                        .expect("the finite epoch has a strictly newer pending cut");
                let state = DependencyMaintenanceState::new(
                    consumers.clone(),
                    waiters.clone(),
                    BTreeMap::from([(key, epoch)]),
                    None,
                )
                .expect("the finite frontier is legal");
                let transition = state
                    .apply_next()
                    .expect("the finite rank is representable")
                    .expect("one dirty key has one bounded step");
                assert!(transition.after_rank < transition.before_rank);
            }
        }
    }
}

#[test]
fn model_dependency_rank_is_the_exact_static_edge_drain_bound() {
    let consumers = edges(&[(1, &[1, 2, 3]), (2, &[4, 5])]);
    let waiters = edges(&[(1, &[1, 3]), (2, &[4])]);
    let dirty = BTreeMap::from([
        (
            ModelDependencyKey::cell(1),
            ModelDirtyDependencyEpoch::new(
                ModelDependencyCut(3),
                ModelDirtyScope::ExistingWaiters,
                None,
                Some(ModelPendingDependencyEpoch {
                    target: ModelDependencyCut(5),
                    scope: ModelDirtyScope::AllConsumers,
                }),
            )
            .expect("the pending epoch is newer"),
        ),
        (
            ModelDependencyKey::cell(2),
            ModelDirtyDependencyEpoch::new(
                ModelDependencyCut(4),
                ModelDirtyScope::AllConsumers,
                None,
                None,
            )
            .expect("the second epoch is legal"),
        ),
    ]);
    let mut state = DependencyMaintenanceState::new(consumers, waiters, dirty, None)
        .expect("the two-key frontier is legal");
    let initial_rank = state.rank().expect("the finite rank is representable");
    let mut steps = 0usize;
    while let Some(transition) = state
        .apply_next()
        .expect("every stable maintenance step is total")
    {
        assert_eq!(transition.after_rank + 1, transition.before_rank);
        state = transition.after;
        steps += 1;
    }
    assert_eq!(steps, initial_rank);
    assert_eq!(state.rank(), Ok(0));
}

#[test]
fn model_dependency_rank_bounds_requeue_pruning_under_a_stable_epoch() {
    let key = ModelDependencyKey::cell(1);
    let dirty = BTreeMap::from([(
        key,
        ModelDirtyDependencyEpoch::new(
            ModelDependencyCut(3),
            ModelDirtyScope::ExistingWaiters,
            None,
            Some(ModelPendingDependencyEpoch {
                target: ModelDependencyCut(5),
                scope: ModelDirtyScope::ExistingWaiters,
            }),
        )
        .expect("the pending waiter epoch is newer"),
    )]);
    let mut state =
        DependencyMaintenanceState::new(edges(&[(1, &[1])]), edges(&[(1, &[1])]), dirty, None)
            .expect("the coalesced waiter frontier is legal");
    let initial_rank = state.rank().expect("the finite rank is representable");
    assert_eq!(initial_rank, 4);

    let first = state
        .apply_next_with_owner_progress(DependencyOwnerProgress::Requeued)
        .expect("one owner requeue is a legal maintenance successor")
        .expect("the current epoch has one waiter edge");
    assert_eq!(first.before_rank, 4);
    assert_eq!(first.after_rank, 2);
    assert!(first.after_rank < first.before_rank);
    state = first.after;

    let mut steps = 1usize;
    while let Some(transition) = state
        .apply_next()
        .expect("the remaining completion markers are total")
    {
        assert!(transition.after_rank < transition.before_rank);
        state = transition.after;
        steps += 1;
    }
    assert_eq!(steps, 3);
    assert!(steps <= initial_rank);
    assert_eq!(state.rank(), Ok(0));
}

#[test]
fn model_dependency_completion_cannot_claim_owner_progress() {
    let key = ModelDependencyKey::cell(1);
    let state = DependencyMaintenanceState::new(
        edges(&[(1, &[1])]),
        ModelDependencyEdges::new(),
        BTreeMap::from([(
            key,
            ModelDirtyDependencyEpoch::new(
                ModelDependencyCut(1),
                ModelDirtyScope::ExistingWaiters,
                None,
                None,
            )
            .expect("the completion-only epoch is legal"),
        )]),
        None,
    )
    .expect("the completion-only frontier is legal");
    assert!(matches!(
        state.apply_next_with_owner_progress(DependencyOwnerProgress::Requeued),
        Err(DependencyProgressError::OwnerProgressWithoutOwner)
    ));
}

#[test]
fn model_dependency_cursor_gives_key_fairness_without_a_second_queue() {
    let consumers = edges(&[(1, &[1, 2]), (2, &[1, 2]), (3, &[1, 2])]);
    let dirty = [1, 2, 3]
        .into_iter()
        .map(|key| {
            (
                ModelDependencyKey::cell(key),
                ModelDirtyDependencyEpoch::new(
                    ModelDependencyCut(1),
                    ModelDirtyScope::AllConsumers,
                    None,
                    None,
                )
                .expect("the finite epoch is legal"),
            )
        })
        .collect();
    let mut state =
        DependencyMaintenanceState::new(consumers, ModelDependencyEdges::new(), dirty, None)
            .expect("the three-key frontier is legal");
    let mut selected = Vec::new();
    for _ in 0..6 {
        let transition = state
            .apply_next()
            .expect("the finite rank is representable")
            .expect("each key still has an edge");
        let key = match transition.step {
            DependencyMaintenanceStep::Advance { key, .. }
            | DependencyMaintenanceStep::Complete { key } => key,
        };
        selected.push(key.ordinal());
        state = transition.after;
    }
    assert_eq!(selected, vec![1, 2, 3, 1, 2, 3]);
    assert_eq!(state.dirty_cursor(), Some(ModelDependencyKey::cell(3)));
}

#[test]
fn model_dependency_event_only_supersedes_with_a_newer_cut() {
    let key = ModelDependencyKey::cell(1);
    let mut state = DependencyMaintenanceState::new(
        edges(&[(1, &[1, 2])]),
        edges(&[(1, &[1])]),
        BTreeMap::from([(
            key,
            ModelDirtyDependencyEpoch::new(
                ModelDependencyCut(4),
                ModelDirtyScope::ExistingWaiters,
                None,
                None,
            )
            .expect("the initial epoch is legal"),
        )]),
        None,
    )
    .expect("the frontier is legal");

    assert!(
        state
            .publish_event(key, ModelDependencyCut(4), ModelDirtyScope::AllConsumers,)
            .is_err()
    );
    assert_eq!(
        state
            .publish_event(key, ModelDependencyCut(5), ModelDirtyScope::AllConsumers,)
            .expect("a newer dependency cut may publish a new epoch"),
        DependencyEventDisposition::Superseded
    );
    let pending = state
        .dirty()
        .get(&key)
        .and_then(|epoch| epoch.pending)
        .expect("the newer epoch is retained behind the current epoch");
    assert_eq!(pending.target, ModelDependencyCut(5));
    assert_eq!(pending.scope, ModelDirtyScope::AllConsumers);
}

#[test]
fn model_dependency_event_without_an_affected_edge_creates_no_drain_work() {
    let mut state = DependencyMaintenanceState::new(
        edges(&[(1, &[1])]),
        ModelDependencyEdges::new(),
        BTreeMap::new(),
        None,
    )
    .expect("the clean frontier is legal");
    assert_eq!(
        state
            .publish_event(
                ModelDependencyKey::cell(1),
                ModelDependencyCut(1),
                ModelDirtyScope::ExistingWaiters,
            )
            .expect("an unaffected level update is total"),
        DependencyEventDisposition::NoMaintenance
    );
    assert_eq!(state.rank(), Ok(0));
}
