use super::{
    dependency::seed_runtime_dependency_maintenance,
    foundation::{accepted_parent_child_at, admit_remote_until, genesis_snapshot, runtime_config},
};
use crate::authority::{
    runtime::AuthorityRuntime,
    state::{AcceptedAtMillis, OwnedTx, PreAcceptedPhase, QueuedWork},
    worker::{run_maintenance_driver, test_support::run_maintenance_driver_for_foundation},
};
use ckb_stop_handler::CancellationToken;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_maintenance_driver_fairly_drains_every_preexisting_level() {
    let snapshot = genesis_snapshot();
    let mut config = runtime_config();
    config.expiry_hours = 0;
    let runtime = AuthorityRuntime::new(&config, snapshot.consensus(), Arc::clone(&snapshot))
        .expect("the authority runtime fixture is valid");
    let (remote, parent, child) = runtime.with_authority_for_foundation(|authority| {
        let remote = admit_remote_until(authority, 1_735, 735, 0);
        let (parent, child) =
            accepted_parent_child_at(authority, 92, AcceptedAtMillis(0), AcceptedAtMillis(1));
        (remote, parent, child)
    });
    let dependency = seed_runtime_dependency_maintenance(&runtime);

    let cancel = CancellationToken::new();
    let task = tokio::spawn(run_maintenance_driver(runtime.clone(), cancel.clone()));
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let converged = runtime.with_authority_for_foundation(|authority| {
                authority.entry(&remote).is_none()
                    && authority.entry(&parent).is_none()
                    && authority.entry(&child).is_none()
                    && matches!(
                        authority.entry(&dependency),
                        Some(OwnedTx::PreAccepted(entry))
                            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
                    )
                    && authority
                        .dependency_maintenance_observation_for_foundation()
                        .expect("the dependency projection remains readable")
                        .is_none()
                    && authority.primary_projection_consistent()
            });
            if converged {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("every maintenance lane progresses despite the other two being non-empty");

    cancel.cancel();
    task.await
        .expect("the maintenance task remains healthy")
        .expect("cancellation is a clean driver outcome");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_idle_maintenance_driver_waits_instead_of_spinning() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the authority runtime fixture is valid");
    let cancel = CancellationToken::new();
    let rounds = Arc::new(AtomicUsize::new(0));
    let task = tokio::spawn(run_maintenance_driver_for_foundation(
        runtime,
        cancel.clone(),
        Arc::clone(&rounds),
    ));

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while rounds.load(Ordering::Relaxed) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the driver performs one initial level read");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(
        rounds.load(Ordering::Relaxed),
        1,
        "an idle authority must suspend until maintenance work or a wall-clock tick"
    );

    cancel.cancel();
    task.await
        .expect("the maintenance task remains healthy")
        .expect("idle cancellation is a clean driver outcome");
}
