use super::{
    dependency::{seed_runtime_dependency_maintenance, seed_runtime_dependency_waiter},
    foundation::{
        accepted_parent_child_at, admit_remote_until, genesis_snapshot, runtime_config, tx,
        verify_remote_transaction,
    },
};
use crate::authority::{
    runtime::AuthorityRuntime,
    state::{AcceptedAtMillis, OwnedTx, PreAcceptedPhase, QueuedWork},
    worker::test_support::AuthorityTestWorkerOwner,
};
use crate::constants::MAX_READY_BATCH;
use ckb_async_runtime::Handle;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_first_dirty_dependency_apply_notifies_an_already_subscribed_consumer() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the authority runtime fixture is valid");
    let (_hash, key) = seed_runtime_dependency_waiter(&runtime);

    let notified = runtime.maintenance_signal().notified();
    tokio::pin!(notified);
    let _ = notified.as_mut().enable();
    assert!(
        runtime
            .publish_dependency_availability_for_foundation(vec![key])
            .expect("the real post-commit dependency event is valid")
    );
    tokio::time::timeout(Duration::from_millis(100), notified.as_mut())
        .await
        .expect("the empty-to-nonempty dirty edge publishes one maintenance prompt");
}

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

    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let workers = AuthorityTestWorkerOwner::spawn_maintenance(runtime.clone(), &handle)
        .expect("the test owns the maintenance worker generation");
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

    workers
        .shutdown()
        .await
        .expect("the maintenance worker generation closes cleanly");
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
    let rounds = Arc::new(AtomicUsize::new(0));
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let workers =
        AuthorityTestWorkerOwner::spawn_observed_maintenance(runtime, &handle, Arc::clone(&rounds))
            .expect("the test owns the observed maintenance worker generation");

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

    workers
        .shutdown()
        .await
        .expect("the observed maintenance worker generation closes cleanly");
}

#[tokio::test(flavor = "current_thread")]
async fn uak_ready_driver_yields_after_one_bounded_progress_attempt() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the authority runtime fixture is valid");
    let keys = runtime.with_authority_for_foundation(|authority| {
        (0..MAX_READY_BATCH + 2)
            .map(|index| {
                verify_remote_transaction(
                    authority,
                    tx(20_000 + u64::try_from(index).expect("the fixture index fits")),
                    800 + index,
                    Vec::new(),
                )
            })
            .collect::<Vec<_>>()
    });
    let attempts = Arc::new(AtomicUsize::new(0));
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let workers = AuthorityTestWorkerOwner::spawn_observed_ready(
        runtime.clone(),
        &handle,
        Arc::clone(&attempts),
    )
    .expect("the test owns the observed Ready worker");
    let cancel = workers.cancellation_for_foundation();
    let observed = Arc::clone(&attempts);
    tokio::time::timeout(Duration::from_secs(2), async move {
        while observed.load(Ordering::Relaxed) == 0 {
            tokio::task::yield_now().await;
        }
        cancel.cancel();
    })
    .await
    .expect("cancellation runs at the first cooperative Ready handoff");

    workers
        .shutdown()
        .await
        .expect("the Ready worker observes cancellation and joins");
    assert_eq!(
        attempts.load(Ordering::Relaxed),
        1,
        "one bounded Apply must yield before another Ready observation"
    );
    let remaining = runtime.with_authority_for_foundation(|authority| {
        keys.iter()
            .filter(|key| {
                matches!(
                    authority.entry(key),
                    Some(OwnedTx::PreAccepted(entry))
                        if matches!(entry.phase, PreAcceptedPhase::Ready(_))
                )
            })
            .count()
    });
    assert_eq!(
        remaining, 2,
        "one bounded Ready plan commits the complete MAX_READY_BATCH prefix before yielding"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_test_worker_owner_aborts_every_task_on_unwind() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the authority runtime fixture is valid");
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let workers = AuthorityTestWorkerOwner::spawn_maintenance(runtime, &handle)
        .expect("the test owns the maintenance worker generation");
    let aborts = workers
        .abort_handles()
        .expect("the bounded worker generation exposes its abort observations");

    let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _workers = workers;
        panic!("exercise structured worker-owner unwind");
    }));
    assert!(unwind.is_err());
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while aborts.iter().any(|handle| !handle.is_finished()) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("unwind aborts every still-owned worker task");
}
