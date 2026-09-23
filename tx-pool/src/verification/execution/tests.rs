use super::*;
use futures_util::poll;
use std::sync::mpsc;
use tokio::sync::{mpsc as async_mpsc, oneshot};

const TEST_TIMEOUT: Duration = Duration::from_secs(3);

struct OnDrop(Option<oneshot::Sender<()>>);

impl Drop for OnDrop {
    fn drop(&mut self) {
        let _ = self.0.take().unwrap().send(());
    }
}

struct Invocation {
    pause: Pause,
    finish: mpsc::Sender<Result<TerminatedResult, Error>>,
}

// Each invocation returns only when the test acknowledges the VM's interrupt.
// This makes the Suspend/Resume race observable without hooks in production code.
fn controlled_vm() -> (
    impl FnMut(Pause) -> Result<TerminatedResult, Error> + Send,
    async_mpsc::UnboundedReceiver<Invocation>,
    oneshot::Receiver<()>,
) {
    let (started, invocations) = async_mpsc::unbounded_channel();
    let (dropped, finished) = oneshot::channel();
    let lifetime = OnDrop(Some(dropped));
    let execute = move |pause| {
        let _ = &lifetime;
        let (finish, result) = mpsc::channel();
        started.send(Invocation { pause, finish }).unwrap();
        result
            .recv_timeout(TEST_TIMEOUT)
            .expect("the test must release the VM")
    };
    (execute, invocations, finished)
}

fn success() -> TerminatedResult {
    TerminatedResult {
        exit_code: 0,
        consumed_cycles: 42,
    }
}

#[tokio::test]
async fn initial_suspend_and_stop_do_not_enter_the_scheduler() {
    for mode in [ComputeMode::Inline, ComputeMode::YieldRuntimeWorker] {
        let (commands, mut receiver) = watch::channel(ChunkCommand::Suspend);
        let mut runner = VmRunner {
            command: &mut receiver,
            remaining: TEST_TIMEOUT,
            mode,
        };
        let mut execution =
            Box::pin(runner.run_vm(|_| panic!("the scheduler must wait for Resume")));
        assert!(poll!(&mut execution).is_pending());
        commands.send(ChunkCommand::Stop).unwrap();
        assert_eq!(
            execution.await.unwrap_err(),
            Error::External("stopped".into())
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn resume_waits_for_the_interrupted_slice_to_return() {
    let (commands, mut receiver) = watch::channel(ChunkCommand::Resume);
    let (execute, mut invocations, dropped) = controlled_vm();
    let mut runner = VmRunner {
        command: &mut receiver,
        remaining: TEST_TIMEOUT,
        mode: ComputeMode::YieldRuntimeWorker,
    };
    let mut verification = Box::pin(runner.run_vm(execute));
    assert!(poll!(&mut verification).is_pending());
    let first = invocations.recv().await.unwrap();
    commands.send(ChunkCommand::Suspend).unwrap();
    assert!(poll!(&mut verification).is_pending());
    assert!(first.pause.has_interrupted());
    commands.send(ChunkCommand::Resume).unwrap();
    assert!(poll!(&mut verification).is_pending());
    assert!(
        first.pause.has_interrupted(),
        "Resume must wait for the previous slice to return"
    );
    assert!(invocations.try_recv().is_err());
    first.finish.send(Err(Error::Pause)).unwrap();
    let next = tokio::select! {
        result = &mut verification => panic!("verification ended before resuming: {result:?}"),
        invocation = invocations.recv() => invocation.unwrap(),
    };
    assert!(!next.pause.has_interrupted());
    next.finish.send(Ok(success())).unwrap();
    assert_eq!(verification.await.unwrap().unwrap().consumed_cycles, 42);
    dropped.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn suspension_can_outlast_the_remaining_budget() {
    let (commands, mut receiver) = watch::channel(ChunkCommand::Resume);
    let (execute, mut invocations, dropped) = controlled_vm();
    let mut runner = VmRunner {
        command: &mut receiver,
        remaining: Duration::from_millis(250),
        mode: ComputeMode::YieldRuntimeWorker,
    };
    let mut verification = Box::pin(runner.run_vm(execute));
    assert!(poll!(&mut verification).is_pending());
    let first = invocations.recv().await.unwrap();
    commands.send(ChunkCommand::Suspend).unwrap();
    assert!(poll!(&mut verification).is_pending());
    assert!(first.pause.has_interrupted());
    first.finish.send(Err(Error::Pause)).unwrap();
    tokio::select! {
        result = &mut verification => panic!("suspension spent the budget: {result:?}"),
        _ = tokio::time::sleep(Duration::from_millis(350)) => {}
    }
    commands.send(ChunkCommand::Resume).unwrap();
    let resumed = tokio::select! {
        result = &mut verification => panic!("verification ended before resuming: {result:?}"),
        invocation = invocations.recv() => invocation.unwrap(),
    };
    resumed.finish.send(Ok(success())).unwrap();
    assert_eq!(verification.await.unwrap().unwrap().consumed_cycles, 42);
    dropped.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn cancellation_interrupts_and_releases_running_vm_work() {
    let (_commands, mut receiver) = watch::channel(ChunkCommand::Resume);
    let (execute, mut invocations, dropped) = controlled_vm();
    let mut runner = VmRunner {
        command: &mut receiver,
        remaining: TEST_TIMEOUT,
        mode: ComputeMode::YieldRuntimeWorker,
    };
    let mut verification = Box::pin(runner.run_vm(execute));
    assert!(poll!(&mut verification).is_pending());
    let running = invocations.recv().await.unwrap();
    drop(verification);
    assert!(running.pause.has_interrupted());
    running.finish.send(Err(Error::Pause)).unwrap();
    tokio::time::timeout(TEST_TIMEOUT, dropped)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn stop_and_closed_control_join_running_vm_work_before_returning() {
    for close in [false, true] {
        let (commands, mut receiver) = watch::channel(ChunkCommand::Resume);
        let (execute, mut invocations, dropped) = controlled_vm();
        let mut runner = VmRunner {
            command: &mut receiver,
            remaining: TEST_TIMEOUT,
            mode: ComputeMode::YieldRuntimeWorker,
        };
        let mut verification = Box::pin(runner.run_vm(execute));
        assert!(poll!(&mut verification).is_pending());
        let running = invocations.recv().await.unwrap();
        if close {
            drop(commands);
        } else {
            // Observe Stop before testing that later Resume cannot reverse it.
            commands.send(ChunkCommand::Stop).unwrap();
            assert!(poll!(&mut verification).is_pending());
            commands.send(ChunkCommand::Resume).unwrap();
        }
        assert!(poll!(&mut verification).is_pending());
        assert!(running.pause.has_interrupted());
        running.finish.send(Err(Error::Pause)).unwrap();
        assert_eq!(
            verification.await.unwrap_err(),
            Error::External("stopped".into())
        );
        dropped.await.unwrap();
        assert!(invocations.recv().await.is_none());
    }
}

#[tokio::test]
async fn cancellation_and_closed_control_release_suspended_vm_work() {
    for cancel in [false, true] {
        let (commands, mut receiver) = watch::channel(ChunkCommand::Suspend);
        let (execute, mut invocations, dropped) = controlled_vm();
        let mut runner = VmRunner {
            command: &mut receiver,
            remaining: TEST_TIMEOUT,
            mode: ComputeMode::YieldRuntimeWorker,
        };
        let mut verification = Box::pin(runner.run_vm(execute));
        assert!(poll!(&mut verification).is_pending());
        if cancel {
            drop(verification);
        } else {
            drop(commands);
            assert_eq!(
                verification.await.unwrap_err(),
                Error::External("stopped".into())
            );
        }
        tokio::time::timeout(TEST_TIMEOUT, dropped)
            .await
            .unwrap()
            .unwrap();
        assert!(invocations.recv().await.is_none());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn deadline_survives_resume_flood_and_joins_vm_on_a_single_worker() {
    deadline_under_resume_flood(ComputeMode::YieldRuntimeWorker).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inline_deadline_keeps_progress_from_a_local_set() {
    tokio::task::LocalSet::new()
        .run_until(deadline_under_resume_flood(ComputeMode::Inline))
        .await;
}

async fn deadline_under_resume_flood(mode: ComputeMode) {
    let (commands, mut receiver) = watch::channel(ChunkCommand::Resume);
    let (started, start) = mpsc::channel::<Pause>();
    let (stop_watchdog, stopped) = mpsc::channel();
    // An independent watchdog makes a runtime-starvation regression fail instead
    // of hanging the test process. It does not implement the verification deadline.
    let watchdog = std::thread::spawn(move || {
        if let Ok(pause) = start.recv_timeout(TEST_TIMEOUT)
            && stopped.recv_timeout(TEST_TIMEOUT).is_err()
        {
            pause.interrupt();
            return false;
        }
        true
    });
    let (dropped, finished) = oneshot::channel();
    let lifetime = OnDrop(Some(dropped));
    let execute = move |pause: Pause| {
        let _ = &lifetime;
        let _ = started.send(pause.clone());
        while !pause.has_interrupted() {
            std::thread::yield_now();
        }
        Err(Error::Pause)
    };
    let mut runner = VmRunner {
        command: &mut receiver,
        remaining: Duration::from_millis(10),
        mode,
    };
    let verification = runner.run_vm(execute);
    tokio::pin!(verification);
    let result = loop {
        tokio::select! {
            biased;
            result = &mut verification => break result,
            _ = tokio::task::yield_now() => { commands.send(ChunkCommand::Resume).unwrap(); }
        }
    };
    let _ = stop_watchdog.send(());
    assert!(
        watchdog.join().unwrap(),
        "the runtime failed to poll its deadline"
    );
    assert!(result.unwrap().is_none());
    assert!(
        finished.await.is_ok(),
        "a budget refusal must join its VM task"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn groups_share_the_budget_and_exhaustion_precedes_completion() {
    let (_commands, mut receiver) = watch::channel(ChunkCommand::Resume);
    let mut runner = VmRunner {
        command: &mut receiver,
        remaining: TEST_TIMEOUT,
        mode: ComputeMode::YieldRuntimeWorker,
    };
    assert!(runner.run_vm(|_| Ok(success())).await.unwrap().is_some());
    assert!(runner.remaining < TEST_TIMEOUT);
    // Even a normally completed slice is refused when its receipt exceeds the
    // remaining transaction budget. The next group must never start.
    runner.remaining = Duration::from_nanos(1);
    assert!(runner.run_vm(|_| Ok(success())).await.unwrap().is_none());
    assert_eq!(runner.remaining, Duration::ZERO);
    assert!(
        runner
            .run_vm(|_| panic!("the shared budget is exhausted"))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn consuming_the_timer_interrupt_does_not_rearm_the_deadline() {
    let (_commands, mut receiver) = watch::channel(ChunkCommand::Resume);
    let (execute, mut invocations, dropped) = controlled_vm();
    let mut runner = VmRunner {
        command: &mut receiver,
        remaining: Duration::from_millis(10),
        mode: ComputeMode::YieldRuntimeWorker,
    };
    let mut verification = Box::pin(runner.run_vm(execute));
    assert!(poll!(&mut verification).is_pending());
    let mut running = invocations.recv().await.unwrap();
    tokio::select! {
        result = &mut verification => panic!("the slice has not returned: {result:?}"),
        _ = tokio::time::sleep(Duration::from_millis(30)) => {}
    }
    assert!(running.pause.has_interrupted());
    // CKB-VM clears the interrupt when it observes Pause, before the scheduler
    // returns. A completed deadline must remain inert during that interval.
    running.pause.free();
    assert!(poll!(&mut verification).is_pending());
    assert!(!running.pause.has_interrupted());
    running.finish.send(Err(Error::Pause)).unwrap();
    assert!(verification.await.unwrap().is_none());
    dropped.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn waiting_to_poll_a_completed_slice_does_not_spend_vm_time() {
    let (_commands, mut receiver) = watch::channel(ChunkCommand::Resume);
    let (finished, finish) = oneshot::channel();
    let mut runner = VmRunner {
        command: &mut receiver,
        remaining: Duration::from_millis(250),
        mode: ComputeMode::YieldRuntimeWorker,
    };
    let mut finished = Some(finished);
    let mut verification = Box::pin(runner.run_vm(move |_| {
        finished.take().unwrap().send(()).unwrap();
        Ok(success())
    }));
    assert!(poll!(&mut verification).is_pending());
    finish.await.unwrap();
    // The parent remains unpolled while its wall-clock deadline passes.
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert_eq!(verification.await.unwrap().unwrap().consumed_cycles, 42);
}

#[test]
fn waiting_for_a_runtime_worker_does_not_spend_vm_time() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let (occupied, worker_occupied) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let blocker = runtime.spawn(async move {
        occupied.send(()).unwrap();
        released.recv_timeout(TEST_TIMEOUT).unwrap();
    });
    worker_occupied.recv_timeout(TEST_TIMEOUT).unwrap();
    runtime.block_on(async {
        let (_commands, mut receiver) = watch::channel(ChunkCommand::Resume);
        let mut runner = VmRunner {
            command: &mut receiver,
            remaining: Duration::from_millis(250),
            mode: ComputeMode::YieldRuntimeWorker,
        };
        let mut verification = Box::pin(runner.run_vm(|_| Ok(success())));
        assert!(poll!(&mut verification).is_pending());
        // Keep the only worker occupied past the budget before releasing it.
        std::thread::sleep(Duration::from_millis(350));
        release.send(()).unwrap();
        blocker.await.unwrap();
        assert_eq!(verification.await.unwrap().unwrap().consumed_cycles, 42);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn scheduler_state_and_cycles_survive_joined_pauses() {
    use super::super::tests::{program_transaction, snapshot};
    use ckb_chain_spec::consensus::ConsensusBuilder;
    use ckb_script::{ScriptVersion, TransactionScriptsVerifier};
    use ckb_store::data_loader_wrapper::AsDataLoader;
    use ckb_types::core::hardfork::HardForks;
    use ckb_verification::TxVerifyEnv;
    use std::sync::Arc;

    let snapshot = snapshot(Arc::new(
        ConsensusBuilder::default()
            .hardfork_switch(HardForks::new_dev())
            .build(),
    ));
    let env = Arc::new(TxVerifyEnv::new_submit(snapshot.tip_header()));
    for version in [ScriptVersion::V0, ScriptVersion::V1, ScriptVersion::V2] {
        let programs: &[&[u8]] = if version == ScriptVersion::V2 {
            &[
                include_bytes!("../../../../script/testdata/spawn_caller_exec"),
                include_bytes!("../../../../script/testdata/current_cycles"),
            ]
        } else {
            &[include_bytes!("../../../../script/testdata/debugger")]
        };
        let verifier = TransactionScriptsVerifier::new(
            program_transaction(version, programs),
            snapshot.as_data_loader(),
            snapshot.cloned_consensus(),
            Arc::clone(&env),
        );
        let (_, group) = verifier.groups().next().unwrap();
        let expected = verifier.detailed_run(group, u64::MAX).unwrap();
        let mut scheduler = verifier.create_scheduler(group).unwrap();
        let (_commands, mut receiver) = watch::channel(ChunkCommand::Resume);
        let mut runner = VmRunner {
            command: &mut receiver,
            remaining: TEST_TIMEOUT,
            mode: ComputeMode::YieldRuntimeWorker,
        };
        let mut attempts = 0;
        let actual = runner
            .run_vm(move |pause| {
                if attempts < 3 {
                    // On V2 advance into spawned VM work before pausing, using
                    // the scheduler's normal stepping interface.
                    if version == ScriptVersion::V2 && attempts == 0 {
                        scheduler.iterate()?;
                    }
                    pause.interrupt();
                    attempts += 1;
                    let result = scheduler.run(RunMode::Pause(pause, u64::MAX));
                    assert!(
                        matches!(result, Err(Error::Pause)),
                        "{version:?}, attempt {attempts}: {result:?}"
                    );
                    result
                } else {
                    scheduler.run(RunMode::Pause(pause, u64::MAX))
                }
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(actual.exit_code, expected.exit_code);
        assert_eq!(actual.consumed_cycles, expected.consumed_cycles);
    }
}
