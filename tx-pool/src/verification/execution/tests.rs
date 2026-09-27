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

fn consume_cpu(duration: Duration) {
    let start = CpuTime::now().unwrap();
    while start.elapsed().unwrap() < duration {
        std::hint::spin_loop();
    }
}

#[test]
fn failed_measurement_cannot_accept_or_resume_but_preserves_vm_failures() {
    for result in [Ok(success()), Err(Error::Pause)] {
        assert_eq!(
            unmeasured_result(result, io::Error::other("clock failed")).unwrap_err(),
            Error::External("stopped".into())
        );
    }
    for error in [
        Error::CyclesExceeded,
        Error::Unexpected("ordinary VM error".into()),
    ] {
        assert_eq!(
            unmeasured_result(Err(error.clone()), io::Error::other("clock failed")).unwrap_err(),
            error
        );
    }
    // Nonzero exit is also a determined failure, attributed by the canonical
    // verifier after SchedulerRunner returns this otherwise successful result.
    let failed = TerminatedResult {
        exit_code: 1,
        consumed_cycles: 42,
    };
    let result = unmeasured_result(Ok(failed), io::Error::other("clock failed")).unwrap();
    assert_eq!(result.exit_code, 1);
    assert_eq!(result.consumed_cycles, 42);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completed_pause_wins_a_late_monitor_failure() {
    let (_commands, mut receiver) = watch::channel(ChunkCommand::Resume);
    let (release, released) = mpsc::channel();
    let child = VmTask {
        pause: Pause::new(),
        handle: tokio::spawn(async move {
            released.recv_timeout(TEST_TIMEOUT).unwrap();
            (
                Err::<TerminatedResult, _>(Error::Pause),
                Duration::from_millis(1),
            )
        }),
    };
    let completed = child.handle.abort_handle();
    let monitor = async move {
        // This single poll runs after join first reported Pending. Complete the
        // child on another worker before returning the old clock's read failure.
        release.send(()).unwrap();
        let watchdog = std::time::Instant::now();
        while !completed.is_finished() {
            assert!(watchdog.elapsed() < TEST_TIMEOUT, "child did not finish");
            std::thread::yield_now();
        }
        Err(io::Error::other("the old execution thread exited"))
    };
    let mut desired = ChunkCommand::Resume;
    let (result, receipt) = child
        .join(&mut receiver, &mut desired, monitor)
        .await
        .unwrap();
    assert_eq!(result.unwrap_err(), Error::Pause);
    assert_eq!(receipt, Duration::from_millis(1));
    assert_eq!(
        desired,
        ChunkCommand::Resume,
        "the next slice must be allowed"
    );
}

#[tokio::test]
async fn running_monitor_failure_stops_and_joins_before_returning() {
    let (commands, mut receiver) = watch::channel(ChunkCommand::Resume);
    let (release, released) = oneshot::channel();
    let pause = Pause::new();
    let child = VmTask {
        pause: pause.clone(),
        handle: tokio::spawn(async move {
            released.await.unwrap();
            Err::<TerminatedResult, _>(Error::Pause)
        }),
    };
    let monitor = async { Err(io::Error::other("clock unavailable")) };
    let mut desired = ChunkCommand::Resume;
    let mut joined = Box::pin(child.join(&mut receiver, &mut desired, monitor));
    assert!(poll!(&mut joined).is_pending());
    assert!(pause.has_interrupted());
    commands.send(ChunkCommand::Resume).unwrap();
    assert!(poll!(&mut joined).is_pending());
    release.send(()).unwrap();
    assert_eq!(joined.await.unwrap().unwrap_err(), Error::Pause);
    assert_eq!(desired, ChunkCommand::Stop);
}

#[tokio::test(start_paused = true)]
async fn cpu_monitor_requires_progress_despite_repeated_wall_checks() {
    use std::cell::Cell;

    let limit = Duration::from_millis(100);
    let cpu = Cell::new(Duration::ZERO);
    let samples = Cell::new(0);
    let wait = wait_for_cpu_budget(limit, || {
        samples.set(samples.get() + 1);
        Ok(cpu.get())
    });
    tokio::pin!(wait);
    assert!(poll!(&mut wait).is_pending());
    for count in 2..=4 {
        tokio::time::advance(limit * 2).await;
        assert!(poll!(&mut wait).is_pending());
        assert_eq!(samples.get(), count);
    }
    cpu.set(limit);
    tokio::time::advance(limit * 2).await;
    assert!(matches!(poll!(&mut wait), std::task::Poll::Ready(Ok(()))));
}

#[tokio::test(start_paused = true)]
async fn cpu_monitor_bounds_polling_near_an_exhausted_budget() {
    use std::cell::Cell;

    let limit = Duration::from_millis(100);
    let cpu = Cell::new(limit - Duration::from_nanos(1));
    let samples = Cell::new(0);
    let wait = wait_for_cpu_budget(limit, || {
        samples.set(samples.get() + 1);
        Ok(cpu.get())
    });
    tokio::pin!(wait);
    assert!(poll!(&mut wait).is_pending());
    for step in 1..=20 {
        tokio::time::advance(Duration::from_micros(100)).await;
        assert!(poll!(&mut wait).is_pending());
        assert!(samples.get() <= 1 + step / 10);
    }
    assert!(samples.get() > 1, "the monitor must still check progress");
    cpu.set(limit);
    tokio::time::advance(Duration::from_millis(2)).await;
    assert!(matches!(poll!(&mut wait), std::task::Poll::Ready(Ok(()))));
}

#[tokio::test(start_paused = true)]
async fn cpu_monitor_read_failure_is_distinct_from_exhaustion() {
    for fail_on_first_read in [true, false] {
        let limit = Duration::from_millis(100);
        let mut fail = fail_on_first_read;
        let wait = wait_for_cpu_budget(limit, || {
            if fail {
                Err(std::io::Error::other("clock unavailable"))
            } else {
                fail = true;
                Ok(Duration::ZERO)
            }
        });
        tokio::pin!(wait);
        if !fail_on_first_read {
            assert!(poll!(&mut wait).is_pending());
            tokio::time::advance(limit * 2).await;
        }
        let error = wait.await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Other);
        assert_eq!(error.to_string(), "clock unavailable");
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
async fn off_cpu_wait_does_not_interrupt_or_exhaust_execution() {
    for suspend_before_completion in [false, true] {
        let (commands, mut receiver) = watch::channel(ChunkCommand::Resume);
        let (execute, mut invocations, dropped) = controlled_vm();
        let mut runner = VmRunner {
            command: &mut receiver,
            remaining: Duration::from_millis(100),
            mode: ComputeMode::YieldRuntimeWorker,
        };
        let mut verification = Box::pin(runner.run_vm(execute));
        assert!(poll!(&mut verification).is_pending());
        let running = invocations.recv().await.unwrap();
        // The channel holds the child off CPU. Poll the parent after each
        // budget-length wall interval: it must not interrupt this execution.
        let mut interrupted_while_waiting = false;
        for _ in 0..3 {
            tokio::select! {
                result = &mut verification => panic!("the child has not returned: {result:?}"),
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
            assert!(poll!(&mut verification).is_pending());
            interrupted_while_waiting |= running.pause.has_interrupted();
            assert!(invocations.try_recv().is_err());
        }
        if suspend_before_completion {
            commands.send(ChunkCommand::Suspend).unwrap();
            assert!(poll!(&mut verification).is_pending());
            assert!(running.pause.has_interrupted());
            commands.send(ChunkCommand::Resume).unwrap();
            assert!(poll!(&mut verification).is_pending());
            running.finish.send(Err(Error::Pause)).unwrap();
            let resumed = tokio::select! {
                result = &mut verification => panic!("off-CPU time exhausted the budget: {result:?}"),
                invocation = invocations.recv() => invocation.unwrap(),
            };
            assert!(!resumed.pause.has_interrupted());
            resumed.finish.send(Ok(success())).unwrap();
        } else {
            running.finish.send(Ok(success())).unwrap();
        }
        let result = verification.await;
        assert!(
            !interrupted_while_waiting,
            "wall time alone must not interrupt the scheduler"
        );
        assert_eq!(result.unwrap().unwrap().consumed_cycles, 42);
        assert!(!runner.remaining.is_zero());
        dropped.await.unwrap();
    }
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
async fn cpu_limit_survives_resume_flood_and_joins_vm_on_a_single_worker() {
    cpu_limit_under_resume_flood(ComputeMode::YieldRuntimeWorker).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inline_cpu_monitor_keeps_progress_from_a_local_set() {
    tokio::task::LocalSet::new()
        .run_until(cpu_limit_under_resume_flood(ComputeMode::Inline))
        .await;
}

async fn cpu_limit_under_resume_flood(mode: ComputeMode) {
    let (commands, mut receiver) = watch::channel(ChunkCommand::Resume);
    let (started, start) = mpsc::channel::<Pause>();
    let (stop_watchdog, stopped) = mpsc::channel();
    // An independent watchdog makes a runtime-starvation regression fail instead
    // of hanging the test process. It does not implement the CPU limit.
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
            std::hint::spin_loop();
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
        "the runtime failed to poll its CPU monitor"
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
    assert!(
        runner
            .run_vm(|_| {
                consume_cpu(Duration::from_millis(2));
                Ok(success())
            })
            .await
            .unwrap()
            .is_some()
    );
    assert!(runner.remaining < TEST_TIMEOUT);
    // Even a normally completed slice is refused when its receipt exceeds the
    // remaining transaction budget. The next group must never start.
    runner.remaining = Duration::from_nanos(1);
    assert!(
        runner
            .run_vm(|_| {
                consume_cpu(Duration::from_millis(2));
                Ok(success())
            })
            .await
            .unwrap()
            .is_none()
    );
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
async fn resumed_slices_spend_one_cpu_budget() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    let (_commands, mut receiver) = watch::channel(ChunkCommand::Resume);
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&attempts);
    let mut runner = VmRunner {
        command: &mut receiver,
        remaining: Duration::from_millis(120),
        mode: ComputeMode::YieldRuntimeWorker,
    };
    let result = runner
        .run_vm(move |_| {
            let attempt = observed.fetch_add(1, Ordering::Relaxed);
            consume_cpu(Duration::from_millis(50));
            if attempt < 2 {
                Err(Error::Pause)
            } else {
                Ok(success())
            }
        })
        .await
        .unwrap();
    assert!(
        result.is_none(),
        "resuming must not replenish the CPU budget"
    );
    assert!((2..=3).contains(&attempts.load(Ordering::Relaxed)));
    assert_eq!(runner.remaining, Duration::ZERO);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn consuming_the_cpu_interrupt_does_not_rearm_the_monitor() {
    let (_commands, mut receiver) = watch::channel(ChunkCommand::Resume);
    let (mut execute, mut invocations, dropped) = controlled_vm();
    let execute = move |pause: Pause| {
        while !pause.has_interrupted() {
            std::hint::spin_loop();
        }
        execute(pause)
    };
    let mut runner = VmRunner {
        command: &mut receiver,
        remaining: Duration::from_millis(10),
        mode: ComputeMode::YieldRuntimeWorker,
    };
    let mut verification = Box::pin(runner.run_vm(execute));
    assert!(poll!(&mut verification).is_pending());
    let mut running = tokio::select! {
        result = &mut verification => panic!("the slice has not returned: {result:?}"),
        invocation = invocations.recv() => invocation.unwrap(),
    };
    assert!(running.pause.has_interrupted());
    // CKB-VM clears the interrupt when it observes Pause, before the scheduler
    // returns. A completed monitor must remain inert during that interval.
    running.pause.free();
    assert!(poll!(&mut verification).is_pending());
    assert!(!running.pause.has_interrupted());
    running.finish.send(Err(Error::Pause)).unwrap();
    assert!(verification.await.unwrap().is_none());
    dropped.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn joined_receipt_excludes_delay_after_task_completion() {
    let (_commands, mut receiver) = watch::channel(ChunkCommand::Resume);
    let child = VmTask {
        pause: Pause::new(),
        handle: tokio::spawn(async {
            let start = CpuTime::now().unwrap();
            consume_cpu(Duration::from_millis(2));
            start.elapsed().unwrap()
        }),
    };
    let completed = child.handle.abort_handle();
    tokio::time::timeout(TEST_TIMEOUT, async {
        while !completed.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // Completion has been published. Delay joining by more than the budget;
    // even a failing monitor must not replace the already final CPU receipt.
    let limit = Duration::from_millis(250);
    tokio::time::sleep(limit + Duration::from_millis(100)).await;
    let mut desired = ChunkCommand::Resume;
    let elapsed = child
        .join(&mut receiver, &mut desired, async {
            Err(io::Error::other("old thread unavailable"))
        })
        .await
        .unwrap();
    assert!(elapsed >= Duration::from_millis(2));
    assert!(elapsed < limit);
    assert_eq!(desired, ChunkCommand::Resume);
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

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn scheduler_resume_cannot_regrant_consumed_cycles() {
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
    for version in [ScriptVersion::V0, ScriptVersion::V2] {
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
        assert!(expected.consumed_cycles > 0);
        for max_cycles in [expected.consumed_cycles - 1, expected.consumed_cycles] {
            let mut scheduler = verifier.create_scheduler(group).unwrap();
            scheduler.iterate().unwrap();
            let consumed = scheduler.consumed_cycles();
            assert!(consumed > 0);
            if version == ScriptVersion::V2 {
                // The root has yielded to its spawned VM: resumption must
                // spend only the part of the total cap not already consumed.
                assert!(!scheduler.terminated());
                assert!(consumed < max_cycles);
            } else {
                // Keep the completed-scheduler regression, including the exact
                // cap where zero remaining cycles must still allow success.
                assert!(scheduler.terminated());
                assert_eq!(consumed, expected.consumed_cycles);
            }
            let (_commands, mut receiver) = watch::channel(ChunkCommand::Resume);
            let mut runner = VmRunner {
                command: &mut receiver,
                remaining: TEST_TIMEOUT,
                mode: ComputeMode::YieldRuntimeWorker,
            };
            let result = runner.run(scheduler, max_cycles).await;
            if max_cycles < expected.consumed_cycles {
                assert!(matches!(result, Err(Error::CyclesExceeded)), "{result:?}");
            } else {
                let actual = result.unwrap().unwrap();
                assert_eq!(actual.exit_code, expected.exit_code);
                assert_eq!(actual.consumed_cycles, expected.consumed_cycles);
            }
        }
    }
}

#[tokio::test]
async fn failed_clock_preserves_canonical_nonzero_exit_attribution() {
    use super::super::tests::{program_transaction, snapshot};
    use ckb_chain_spec::consensus::ConsensusBuilder;
    use ckb_script::{ScriptVersion, TransactionScriptsVerifier};
    use ckb_store::data_loader_wrapper::AsDataLoader;
    use ckb_verification::TxVerifyEnv;
    use std::sync::Arc;

    struct FailedReceipt;
    impl SchedulerRunner<Scheduler<DataLoaderWrapper<Snapshot>, DebugPrinter, Machine>>
        for FailedReceipt
    {
        async fn run(
            &mut self,
            mut scheduler: Scheduler<DataLoaderWrapper<Snapshot>, DebugPrinter, Machine>,
            max_cycles: Cycle,
        ) -> Result<Option<TerminatedResult>, Error> {
            let result = scheduler.run(RunMode::Pause(Pause::new(), max_cycles));
            assert_ne!(result.as_ref().unwrap().exit_code, 0);
            unmeasured_result(result, io::Error::other("CPU receipt failed")).map(Some)
        }
    }
    let snapshot = snapshot(Arc::new(ConsensusBuilder::default().build()));
    let env = Arc::new(TxVerifyEnv::new_submit(snapshot.tip_header()));
    let verifier = TransactionScriptsVerifier::new(
        program_transaction(
            ScriptVersion::V0,
            &[include_bytes!("../../../../script/testdata/always_failure")],
        ),
        snapshot.as_data_loader(),
        snapshot.cloned_consensus(),
        env,
    );
    let expected = verifier.verify(u64::MAX).unwrap_err();
    let actual = verifier
        .verify_with_runner(u64::MAX, &mut FailedReceipt)
        .await
        .unwrap_err();
    ckb_error::assert_error_eq!(actual, expected);
}
