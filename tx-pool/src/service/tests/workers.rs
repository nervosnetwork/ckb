use super::*;

/// Consume an ordered channel without acknowledging the head item until its
/// handler succeeds. Later messages stay in the bounded channel, providing
/// backpressure and preserving transition order.
async fn run_retained_receiver<T, F, Fut>(
    worker_name: &'static str,
    mut receiver: mpsc::Receiver<T>,
    cancel: &CancellationToken,
    mut handler: F,
) where
    T: Clone,
    F: FnMut(T) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    loop {
        let item = tokio::select! {
            item = receiver.recv() => item,
            _ = cancel.cancelled() => None,
        };
        let Some(item) = item else {
            break;
        };
        if !retry_retained_message(worker_name, item, cancel, &mut handler).await {
            break;
        }
    }
}

#[test]
fn respawn_backoff_progresses_caps_and_resets() {
    let mut backoff = RespawnBackoff::new();
    let crash = Duration::from_millis(5);

    // Consecutive crashes: 100ms, 200ms, 400ms, ...
    assert_eq!(backoff.delay_for(crash), Duration::from_millis(100));
    assert_eq!(backoff.delay_for(crash), Duration::from_millis(200));
    assert_eq!(backoff.delay_for(crash), Duration::from_millis(400));

    // Capped at 30s under a persistent failure.
    for _ in 0..20 {
        backoff.delay_for(crash);
    }
    assert_eq!(backoff.delay_for(crash), Duration::from_secs(30));

    // A healthy run (>= HEALTHY_RUN) resets the backoff to the base.
    assert_eq!(
        backoff.delay_for(Duration::from_secs(120)),
        Duration::from_millis(100)
    );
}

/// A received reorg delta must survive more than the historical single
/// retry. The same head item remains selected until one attempt succeeds.
#[tokio::test]
async fn retained_message_retries_until_success() {
    let cancel = CancellationToken::new();
    let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let attempts_for_handler = Arc::clone(&attempts);
    let completed = retry_retained_message("test retained worker", 7usize, &cancel, move |item| {
        let attempts = Arc::clone(&attempts_for_handler);
        async move {
            assert_eq!(item, 7);
            if attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 3 {
                panic!("injected deterministic transition panic");
            }
        }
    })
    .await;

    assert!(completed);
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 4);
}

#[tokio::test]
async fn second_phase_retry_never_replays_completed_first_phase() {
    let cancel = CancellationToken::new();
    let first_attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let second_attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let first_counter = Arc::clone(&first_attempts);
    let second_counter = Arc::clone(&second_attempts);
    let completed = retry_retained_two_phase(
        "test authoritative phase",
        "test derived refresh phase",
        9usize,
        &cancel,
        move |item| {
            let attempts = Arc::clone(&first_counter);
            async move {
                assert_eq!(item, 9);
                attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        },
        move |item| {
            let attempts = Arc::clone(&second_counter);
            async move {
                assert_eq!(item, 9);
                if attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    panic!("injected derived refresh failure");
                }
            }
        },
    )
    .await;

    assert!(completed);
    assert_eq!(first_attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(second_attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
}

/// Persistent failure must be cancel-aware while retaining the item; it
/// must neither hot-spin nor report success/drop.
#[tokio::test]
async fn retained_message_cancellation_interrupts_backoff() {
    let cancel = CancellationToken::new();
    let cancel_for_task = cancel.clone();
    let task = tokio::spawn(async move {
        retry_retained_message(
            "test retained worker",
            1usize,
            &cancel_for_task,
            |_| async { panic!("persistent injected panic") },
        )
        .await
    });

    tokio::time::sleep(Duration::from_millis(20)).await;
    cancel.cancel();
    let completed = tokio::time::timeout(Duration::from_millis(200), task)
        .await
        .expect("cancellation must interrupt retry backoff")
        .expect("retry task joins");
    assert!(!completed, "cancelled retained work is not acknowledged");
}

/// Later chain deltas must never overtake a retained/panicking head delta.
#[tokio::test]
async fn retained_receiver_preserves_fifo_across_panics() {
    let (sender, receiver) = mpsc::channel(2);
    sender.send(1usize).await.unwrap();
    sender.send(2usize).await.unwrap();
    drop(sender);

    let cancel = CancellationToken::new();
    let first_attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let processed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let attempts_for_handler = Arc::clone(&first_attempts);
    let processed_for_handler = Arc::clone(&processed);
    run_retained_receiver("test retained receiver", receiver, &cancel, move |item| {
        let attempts = Arc::clone(&attempts_for_handler);
        let processed = Arc::clone(&processed_for_handler);
        async move {
            if item == 1 && attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 2 {
                panic!("injected head transition panic");
            }
            processed.lock().unwrap().push(item);
        }
    })
    .await;

    assert_eq!(processed.lock().unwrap().as_slice(), &[1, 2]);
    assert_eq!(first_attempts.load(std::sync::atomic::Ordering::SeqCst), 3);
}

/// Cancel during backoff sleep must exit immediately (bug #40):
/// the `select!` wrapping the sleep must observe cancellation
/// instead of waiting out the full backoff duration.
#[tokio::test]
async fn cancel_during_backoff_exits_immediately() {
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    // Spawn a task that simulates a worker in backoff sleep.
    let handle = tokio::spawn(async move {
        let mut backoff = RespawnBackoff::new();
        // Simulate a crash to get a non-trivial backoff delay.
        let _ = backoff.delay_for(Duration::from_millis(1));
        let _ = backoff.delay_for(Duration::from_millis(1));
        // Now the next delay would be 400ms.
        let delay = backoff.delay_for(Duration::from_millis(1));
        assert!(delay >= Duration::from_millis(400));

        let started = std::time::Instant::now();
        tokio::select! {
            _ = tokio::time::sleep(delay) => {
                // Should not reach here if cancel fires first.
                started.elapsed()
            }
            _ = cancel_clone.cancelled() => {
                // Cancel fired: exit immediately.
                started.elapsed()
            }
        }
    });

    // Give the task time to enter the select!.
    tokio::time::sleep(Duration::from_millis(50)).await;
    // Cancel: the task should exit well before the 400ms backoff.
    cancel.cancel();

    let elapsed = tokio::time::timeout(Duration::from_millis(200), handle)
        .await
        .expect("task must exit within 200ms of cancel, not wait out the backoff")
        .expect("task joins");

    assert!(
        elapsed < Duration::from_millis(150),
        "cancel must interrupt backoff sleep immediately, took {:?}",
        elapsed
    );
}
