//! Dedicated bounded OS compute workers for synchronous transaction resolution,
//! script verification, and local fast-path admission without Tokio
//! `block_in_place` thread churn.

use super::Error;
use ckb_util::parking_lot::{Condvar, Mutex};
use std::{
    collections::VecDeque,
    panic::AssertUnwindSafe,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

/// Brief single-worker adaptive spin before sleeping in the kernel after a
/// local RPC task finishes. Cuts cross-thread wake latency on serial RPC
/// workloads while keeping idle CPU at zero and bounding spinning to one core.
const LOCAL_IDLE_SPIN: Duration = Duration::from_micros(100);

pub(super) type ComputeTask = Box<dyn FnOnce() + Send + 'static>;

/// Result of one attempt to execute a queued direct local submission on a
/// compute worker.
pub(crate) enum LocalStep {
    /// Finished (or handed off to async fallback); do not requeue.
    Finished,
    /// Compute or active-job capacity was momentarily contended; requeue at the
    /// front of `local` and wait for a capacity release or a permitted task.
    RetryOnCapacity,
}

pub(crate) type LocalTask = Box<dyn FnMut() -> LocalStep + Send + 'static>;

/// Why a task could not be accepted into [`ComputePool`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PushErrorKind {
    /// The pool has already closed for shutdown.
    Closed,
    /// Starting the first required OS worker thread failed; the caller's exact
    /// task is returned without modifying queue state so later calls can retry.
    WorkerSpawnFailed,
}

struct Queue {
    /// Callers that already hold a `ComputePermit` (background workers and
    /// async local fallback retries). Always drained before `local` so
    /// permitted work can never starve or deadlock behind unpermitted jobs.
    permitted: VecDeque<ComputeTask>,
    /// Direct local RPC fast-path jobs waiting in strict FIFO order.
    local: VecDeque<LocalTask>,
    work_idle: usize,
    capacity_idle: usize,
    /// At most one idle worker is briefly spinning before kernel park.
    spinning: bool,
    /// Number of successfully created OS workers currently alive in `threads`.
    live_workers: usize,
    /// Number of extra OS workers (`>= 2`) currently being created off-lock.
    spawning: usize,
    next_worker_id: usize,
    closed: bool,
}

struct Shared {
    max_threads: usize,
    has_worker: AtomicBool,
    closed_flag: AtomicBool,
    /// True only when no concurrent worker activity is detected (serial local
    /// traffic). Shared with `TxPoolPublisher` so neither thread ever burns
    /// spin cycles under multi-worker saturation.
    serial_spin: Arc<AtomicBool>,
    local_queued: AtomicUsize,
    permitted_queued: AtomicUsize,
    capacity_epoch: AtomicU64,
    waiting_capacity: AtomicUsize,
    queue: Mutex<Queue>,
    work_ready: Condvar,
    capacity_ready: Condvar,
}

/// Lock-free wake handle attached to [`super::ComputePermit`] and
/// [`super::ActivePermit`] so capacity-waiting `ComputePool` workers resume
/// immediately after capacity is released, without snowballing into async
/// fallback tasks.
#[derive(Clone)]
pub(in crate::authority) struct CapacitySignal {
    shared: Arc<Shared>,
}

impl CapacitySignal {
    pub(in crate::authority) fn notify(&self) {
        self.shared.capacity_epoch.fetch_add(1, Ordering::SeqCst);
        if self.shared.waiting_capacity.load(Ordering::SeqCst) > 0 {
            let _guard = self.shared.queue.lock();
            self.shared.capacity_ready.notify_all();
        }
    }
}

#[cfg(test)]
type SpawnHook = Arc<dyn Fn(usize) -> Option<std::io::Result<()>> + Send + Sync>;

/// Bounded lazy-spawning OS worker pool owned by one [`super::Pool`] generation.
///
/// Worker threads never retain `Arc<Pool>` across idle waits, so dropping the
/// pool cleanly closes the queue and joins every spawned OS thread with no
/// reference cycle.
///
/// Lock order: `first_worker` -> `threads` -> `queue`.
/// The first worker is established and committed in the same `queue` transition
/// that accepts the caller's task, so a cold-start thread creation failure can
/// only return that caller's own task and never strands acknowledged work.
pub(super) struct ComputePool {
    shared: Arc<Shared>,
    first_worker: Mutex<()>,
    threads: Mutex<Vec<std::thread::JoinHandle<()>>>,
    #[cfg(test)]
    spawn_hook: Mutex<Option<SpawnHook>>,
}

impl ComputePool {
    pub(super) fn new(max_threads: usize) -> Self {
        Self {
            shared: Arc::new(Shared {
                max_threads: max_threads.max(1),
                has_worker: AtomicBool::new(false),
                closed_flag: AtomicBool::new(false),
                serial_spin: Arc::new(AtomicBool::new(false)),
                local_queued: AtomicUsize::new(0),
                permitted_queued: AtomicUsize::new(0),
                capacity_epoch: AtomicU64::new(0),
                waiting_capacity: AtomicUsize::new(0),
                queue: Mutex::new(Queue {
                    permitted: VecDeque::new(),
                    local: VecDeque::new(),
                    work_idle: 0,
                    capacity_idle: 0,
                    spinning: false,
                    live_workers: 0,
                    spawning: 0,
                    next_worker_id: 0,
                    closed: false,
                }),
                work_ready: Condvar::new(),
                capacity_ready: Condvar::new(),
            }),
            first_worker: Mutex::new(()),
            threads: Mutex::new(Vec::new()),
            #[cfg(test)]
            spawn_hook: Mutex::new(None),
        }
    }

    pub(super) fn signal(&self) -> CapacitySignal {
        CapacitySignal {
            shared: Arc::clone(&self.shared),
        }
    }

    pub(super) fn notify_capacity(&self) {
        self.signal().notify();
    }

    pub(super) fn serial_spin_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.shared.serial_spin)
    }

    fn spawn_worker_thread(&self, id: usize) -> std::io::Result<std::thread::JoinHandle<()>> {
        #[cfg(test)]
        {
            let hook = self.spawn_hook.lock().clone();
            if let Some(hook) = hook
                && let Some(Err(error)) = hook(id)
            {
                return Err(error);
            }
        }
        let shared = Arc::clone(&self.shared);
        std::thread::Builder::new()
            .name(format!("TxPoolCpu-{id}"))
            .spawn(move || worker_loop(shared))
    }

    fn spawn_extra_worker(&self, id: usize) {
        match self.spawn_worker_thread(id) {
            Ok(handle) => {
                self.threads.lock().push(handle);
                let mut q = self.shared.queue.lock();
                q.spawning = q.spawning.saturating_sub(1);
                q.live_workers = q.live_workers.saturating_add(1);
                if q.closed {
                    self.shared.work_ready.notify_all();
                }
            }
            Err(_) => {
                let mut q = self.shared.queue.lock();
                q.spawning = q.spawning.saturating_sub(1);
            }
        }
    }

    /// Cold-start path: establish the first live OS worker and commit `task`
    /// inside one `queue` transition so no caller is acknowledged before a live
    /// worker exists, and spawn failure returns only this caller's exact task.
    fn push_cold<T>(
        &self,
        task: T,
        enqueue: impl FnOnce(&mut Queue, &Shared, T),
        push_warm: impl FnOnce(&Self, T) -> Result<(), (PushErrorKind, T)>,
    ) -> Result<(), (PushErrorKind, T)> {
        let _startup = self.first_worker.lock();
        if self.shared.has_worker.load(Ordering::SeqCst) {
            drop(_startup);
            return push_warm(self, task);
        }
        let id = {
            let mut q = self.shared.queue.lock();
            if q.closed {
                return Err((PushErrorKind::Closed, task));
            }
            debug_assert_eq!(q.live_workers, 0);
            q.next_worker_id = q.next_worker_id.saturating_add(1);
            q.next_worker_id
        };
        let handle = match self.spawn_worker_thread(id) {
            Ok(handle) => handle,
            Err(_) => return Err((PushErrorKind::WorkerSpawnFailed, task)),
        };
        self.threads.lock().push(handle);
        let mut q = self.shared.queue.lock();
        if q.closed {
            self.shared.work_ready.notify_all();
            return Err((PushErrorKind::Closed, task));
        }
        q.live_workers = q.live_workers.saturating_add(1);
        self.shared.has_worker.store(true, Ordering::SeqCst);
        enqueue(&mut q, &self.shared, task);
        if q.work_idle > 0 {
            self.shared.work_ready.notify_one();
        }
        Ok(())
    }

    fn push_permitted(&self, task: ComputeTask) -> Result<(), (PushErrorKind, ComputeTask)> {
        if !self.shared.has_worker.load(Ordering::SeqCst) {
            return self.push_cold(
                task,
                |q, shared, task| {
                    q.permitted.push_back(task);
                    shared.permitted_queued.fetch_add(1, Ordering::SeqCst);
                },
                Self::push_permitted_warm,
            );
        }
        self.push_permitted_warm(task)
    }

    fn push_permitted_warm(&self, task: ComputeTask) -> Result<(), (PushErrorKind, ComputeTask)> {
        let need_spawn = {
            let mut q = self.shared.queue.lock();
            if q.closed {
                return Err((PushErrorKind::Closed, task));
            }
            q.permitted.push_back(task);
            self.shared.permitted_queued.fetch_add(1, Ordering::SeqCst);
            self.shared.serial_spin.store(false, Ordering::SeqCst);
            let total_idle = q
                .work_idle
                .saturating_add(q.capacity_idle)
                .saturating_add(usize::from(q.spinning));
            let pending = q.permitted.len().saturating_add(q.local.len());
            if q.work_idle > 0 {
                self.shared.work_ready.notify_one();
                None
            } else if q.capacity_idle > 0 {
                self.shared.capacity_ready.notify_one();
                None
            } else if q.live_workers.saturating_add(q.spawning) < self.shared.max_threads
                && pending > total_idle
            {
                q.spawning = q.spawning.saturating_add(1);
                q.next_worker_id = q.next_worker_id.saturating_add(1);
                Some(q.next_worker_id)
            } else {
                None
            }
        };
        if let Some(id) = need_spawn {
            self.spawn_extra_worker(id);
        }
        Ok(())
    }

    pub(super) fn push_local(&self, task: LocalTask) -> Result<(), (PushErrorKind, LocalTask)> {
        if !self.shared.has_worker.load(Ordering::SeqCst) {
            return self.push_cold(
                task,
                |q, shared, task| {
                    q.local.push_back(task);
                    shared.local_queued.fetch_add(1, Ordering::SeqCst);
                },
                Self::push_local_warm,
            );
        }
        self.push_local_warm(task)
    }

    fn push_local_warm(&self, task: LocalTask) -> Result<(), (PushErrorKind, LocalTask)> {
        let need_spawn = {
            let mut q = self.shared.queue.lock();
            if q.closed {
                return Err((PushErrorKind::Closed, task));
            }
            q.local.push_back(task);
            self.shared.local_queued.fetch_add(1, Ordering::SeqCst);
            let pending = q.permitted.len().saturating_add(q.local.len());
            if pending > 1 || (q.work_idle == 0 && !q.spinning) {
                self.shared.serial_spin.store(false, Ordering::SeqCst);
            }
            if q.work_idle > 0 {
                self.shared.work_ready.notify_one();
                None
            } else {
                let active_idle = q.capacity_idle.saturating_add(usize::from(q.spinning));
                if q.live_workers.saturating_add(q.spawning) < self.shared.max_threads
                    && pending > active_idle
                {
                    q.spawning = q.spawning.saturating_add(1);
                    q.next_worker_id = q.next_worker_id.saturating_add(1);
                    Some(q.next_worker_id)
                } else {
                    None
                }
            }
        };
        if let Some(id) = need_spawn {
            self.spawn_extra_worker(id);
        }
        Ok(())
    }

    /// Execute a synchronous closure on a dedicated worker for a caller that
    /// already holds a [`super::ComputePermit`], propagating panics to the caller.
    pub(super) async fn run_permitted<T: Send + 'static>(
        &self,
        operation: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, Error> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let task: ComputeTask = Box::new(move || {
            let _ = tx.send(std::panic::catch_unwind(AssertUnwindSafe(operation)));
        });
        if let Err((kind, task)) = self.push_permitted(task) {
            drop(task);
            return Err(match kind {
                PushErrorKind::Closed => Error::Closed,
                PushErrorKind::WorkerSpawnFailed => {
                    Error::Full("compute worker unavailable".into())
                }
            });
        }
        match rx.await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(payload)) => std::panic::resume_unwind(payload),
            Err(_) => Err(Error::Closed),
        }
    }
}

impl Drop for ComputePool {
    fn drop(&mut self) {
        self.shared.serial_spin.store(false, Ordering::SeqCst);
        self.shared.closed_flag.store(true, Ordering::SeqCst);
        let (leftover_permitted, leftover_local): (Vec<ComputeTask>, Vec<LocalTask>) = {
            let mut q = self.shared.queue.lock();
            q.closed = true;
            let permitted_len = q.permitted.len();
            let permitted = if permitted_len > 0 {
                self.shared
                    .permitted_queued
                    .fetch_sub(permitted_len, Ordering::SeqCst);
                q.permitted.drain(..).collect()
            } else {
                Vec::new()
            };
            let local_len = q.local.len();
            let local = if local_len > 0 {
                self.shared
                    .local_queued
                    .fetch_sub(local_len, Ordering::SeqCst);
                q.local.drain(..).collect()
            } else {
                Vec::new()
            };
            self.shared.work_ready.notify_all();
            self.shared.capacity_ready.notify_all();
            (permitted, local)
        };
        drop(leftover_permitted);
        drop(leftover_local);
        let current = std::thread::current().id();
        let handles: Vec<_> = self.threads.lock().drain(..).collect();
        for handle in handles {
            if handle.thread().id() != current {
                let _ = handle.join();
            }
        }
    }
}

fn worker_loop(shared: Arc<Shared>) {
    let mut q = shared.queue.lock();
    let mut should_spin = false;
    loop {
        if let Some(task) = q.permitted.pop_front() {
            shared.permitted_queued.fetch_sub(1, Ordering::SeqCst);
            drop(q);
            let _ = std::panic::catch_unwind(AssertUnwindSafe(task));
            should_spin = false;
            q = shared.queue.lock();
            continue;
        }
        if let Some(mut task) = q.local.pop_front() {
            shared.local_queued.fetch_sub(1, Ordering::SeqCst);
            let epoch = shared.capacity_epoch.load(Ordering::SeqCst);
            drop(q);
            let retry = matches!(
                std::panic::catch_unwind(AssertUnwindSafe(&mut task)),
                Ok(LocalStep::RetryOnCapacity)
            );
            q = shared.queue.lock();
            if retry {
                if q.closed {
                    drop(q);
                    drop(task);
                    return;
                }
                q.local.push_front(task);
                shared.local_queued.fetch_add(1, Ordering::SeqCst);
                if !q.permitted.is_empty() {
                    continue;
                }
                shared.waiting_capacity.fetch_add(1, Ordering::SeqCst);
                while !q.closed
                    && q.permitted.is_empty()
                    && shared.capacity_epoch.load(Ordering::SeqCst) == epoch
                {
                    q.capacity_idle = q.capacity_idle.saturating_add(1);
                    shared.capacity_ready.wait(&mut q);
                    q.capacity_idle = q.capacity_idle.saturating_sub(1);
                }
                shared.waiting_capacity.fetch_sub(1, Ordering::SeqCst);
                should_spin = false;
            } else {
                should_spin = true;
            }
            continue;
        }
        if q.closed {
            return;
        }
        let other_busy = q
            .live_workers
            .saturating_sub(1)
            .saturating_sub(q.work_idle)
            .saturating_sub(q.capacity_idle);
        if should_spin && !q.spinning && other_busy == 0 && q.capacity_idle == 0 {
            shared.serial_spin.store(true, Ordering::SeqCst);
            q.spinning = true;
            should_spin = false;
            drop(q);
            if let Some(deadline) = Instant::now().checked_add(LOCAL_IDLE_SPIN) {
                while !shared.closed_flag.load(Ordering::SeqCst)
                    && shared.local_queued.load(Ordering::SeqCst) == 0
                    && shared.permitted_queued.load(Ordering::SeqCst) == 0
                    && shared.serial_spin.load(Ordering::SeqCst)
                    && Instant::now() < deadline
                {
                    std::hint::spin_loop();
                }
            }
            q = shared.queue.lock();
            q.spinning = false;
            if q.local.is_empty() && q.permitted.is_empty() {
                shared.serial_spin.store(false, Ordering::SeqCst);
            }
            continue;
        }
        if other_busy > 0 {
            shared.serial_spin.store(false, Ordering::SeqCst);
        }
        q.work_idle = q.work_idle.saturating_add(1);
        shared.work_ready.wait(&mut q);
        q.work_idle = q.work_idle.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use tokio::sync::Semaphore;

    struct DropWitness {
        id: usize,
        dropped: mpsc::Sender<usize>,
    }

    impl Drop for DropWitness {
        fn drop(&mut self) {
            let _ = self.dropped.send(self.id);
        }
    }

    #[test]
    fn cold_start_failure_on_local_queue_preserves_caller_identity_permits_and_recovers() {
        let pool = Arc::new(ComputePool::new(1));
        let permits = Arc::new(Semaphore::new(2));
        let attempts = Arc::new(AtomicUsize::new(0));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Arc::new(Mutex::new(release_rx));

        let hook_attempts = Arc::clone(&attempts);
        *pool.spawn_hook.lock() = Some(Arc::new(move |_id| {
            let attempt = hook_attempts.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                let _ = entered_tx.send(());
                let _ = release_rx.lock().recv_timeout(Duration::from_secs(10));
                Some(Err(std::io::Error::other(
                    "simulated cold-start local worker failure",
                )))
            } else {
                None
            }
        }));

        let ran_a = Arc::new(AtomicUsize::new(0));
        let ran_b = Arc::new(AtomicUsize::new(0));
        let (dropped_tx, dropped_rx) = mpsc::channel();
        let (reply_a_tx, reply_a_rx) = mpsc::channel();
        let (reply_b_tx, reply_b_rx) = mpsc::channel();
        let make_task = |id: usize,
                         ran: Arc<AtomicUsize>,
                         permit: tokio::sync::OwnedSemaphorePermit,
                         reply_tx: mpsc::Sender<(usize, String)>|
         -> LocalTask {
            let witness = DropWitness {
                id,
                dropped: dropped_tx.clone(),
            };
            let mut state = Some((permit, reply_tx, witness));
            Box::new(move || {
                let Some((permit, reply_tx, witness)) = state.take() else {
                    return LocalStep::Finished;
                };
                ran.fetch_add(1, Ordering::SeqCst);
                let worker = std::thread::current().name().unwrap_or("").to_owned();
                let _ = reply_tx.send((id, worker));
                drop(permit);
                drop(witness);
                LocalStep::Finished
            })
        };

        let p1 = Arc::clone(&permits).try_acquire_owned().unwrap();
        let p2 = Arc::clone(&permits).try_acquire_owned().unwrap();
        assert_eq!(permits.available_permits(), 0);

        let pool_a = Arc::clone(&pool);
        let task_a = make_task(1, Arc::clone(&ran_a), p1, reply_a_tx);
        let thread_a = std::thread::spawn(move || pool_a.push_local(task_a));

        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("caller A entered cold-start spawn");

        let (b_started_tx, b_started_rx) = mpsc::channel();
        let (b_ack_tx, b_ack_rx) = mpsc::channel();
        let pool_b = Arc::clone(&pool);
        let task_b = make_task(2, Arc::clone(&ran_b), p2, reply_b_tx);
        let thread_b = std::thread::spawn(move || {
            let _ = b_started_tx.send(());
            let res = pool_b.push_local(task_b);
            let _ = b_ack_tx.send(res.is_ok());
            res
        });

        b_started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("caller B started");
        // Caller B must not be acknowledged before a live worker exists.
        assert!(
            b_ack_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "concurrent submission must wait until first worker startup settles"
        );

        release_tx.send(()).unwrap();

        let (kind, returned_a) = thread_a
            .join()
            .unwrap()
            .expect_err("caller A must fail when first worker creation fails");
        assert_eq!(kind, PushErrorKind::WorkerSpawnFailed);

        // Caller B must recover on the next spawn attempt and execute task 2 once.
        assert!(b_ack_rx.recv_timeout(Duration::from_secs(5)).unwrap());
        assert!(thread_b.join().unwrap().is_ok());
        let (ran_id, worker_name) = reply_b_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(ran_id, 2);
        assert!(
            worker_name.starts_with("TxPoolCpu-"),
            "unexpected worker thread: {worker_name}"
        );
        assert_eq!(dropped_rx.recv_timeout(Duration::from_secs(5)).unwrap(), 2);
        assert_eq!(ran_a.load(Ordering::SeqCst), 0);
        assert_eq!(ran_b.load(Ordering::SeqCst), 1);
        // Task 2 finished and released permit 2; permit 1 is still owned by returned_a.
        assert_eq!(permits.available_permits(), 1);
        assert!(reply_a_rx.try_recv().is_err());

        // Dropping returned_a proves it is caller A's exact task 1, never executed,
        // closes A's reply channel, and reclaims permit 1.
        drop(returned_a);
        assert_eq!(dropped_rx.recv_timeout(Duration::from_secs(5)).unwrap(), 1);
        assert_eq!(reply_a_rx.try_recv(), Err(mpsc::TryRecvError::Disconnected));
        assert_eq!(ran_a.load(Ordering::SeqCst), 0);
        assert_eq!(ran_b.load(Ordering::SeqCst), 1);
        assert_eq!(permits.available_permits(), 2);

        {
            let q = pool.shared.queue.lock();
            assert_eq!(q.live_workers, 1);
            assert!(q.local.is_empty());
            assert_eq!(pool.shared.local_queued.load(Ordering::SeqCst), 0);
        }
        drop(pool);
    }

    #[test]
    fn cold_start_failure_on_permitted_queue_preserves_caller_identity_permits_and_recovers() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(3)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let pool = Arc::new(ComputePool::new(1));
            let permits = Arc::new(Semaphore::new(2));
            let attempts = Arc::new(AtomicUsize::new(0));
            let (entered_tx, entered_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let release_rx = Arc::new(Mutex::new(release_rx));

            let hook_attempts = Arc::clone(&attempts);
            *pool.spawn_hook.lock() = Some(Arc::new(move |_id| {
                let attempt = hook_attempts.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    let _ = entered_tx.send(());
                    release_rx
                        .lock()
                        .recv_timeout(Duration::from_secs(5))
                        .expect("test must release caller A explicitly");
                    Some(Err(std::io::Error::other(
                        "simulated cold-start permitted worker failure",
                    )))
                } else {
                    None
                }
            }));

            let ran_a = Arc::new(AtomicUsize::new(0));
            let ran_b = Arc::new(AtomicUsize::new(0));
            let (dropped_tx, dropped_rx) = mpsc::channel();

            let p1 = Arc::clone(&permits).try_acquire_owned().unwrap();
            let p2 = Arc::clone(&permits).try_acquire_owned().unwrap();
            assert_eq!(permits.available_permits(), 0);

            let pool_a = Arc::clone(&pool);
            let ran_a_clone = Arc::clone(&ran_a);
            let witness_a = DropWitness {
                id: 1,
                dropped: dropped_tx.clone(),
            };
            let fut_a = tokio::spawn(async move {
                let _permit = p1;
                pool_a
                    .run_permitted(move || {
                        let _witness = witness_a;
                        ran_a_clone.fetch_add(1, Ordering::SeqCst);
                        1usize
                    })
                    .await
            });

            entered_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("caller A entered cold-start spawn");

            let (b_started_tx, b_started_rx) = mpsc::channel();
            let pool_b = Arc::clone(&pool);
            let ran_b_clone = Arc::clone(&ran_b);
            let witness_b = DropWitness {
                id: 2,
                dropped: dropped_tx.clone(),
            };
            let fut_b = tokio::spawn(async move {
                let _ = b_started_tx.send(());
                let _permit = p2;
                pool_b
                    .run_permitted(move || {
                        let _witness = witness_b;
                        ran_b_clone.fetch_add(1, Ordering::SeqCst);
                        2usize
                    })
                    .await
            });

            b_started_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("caller B started");
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert!(
                !fut_b.is_finished(),
                "permitted caller B must not complete before first worker exists"
            );

            release_tx.send(()).unwrap();

            let res_a = fut_a.await.unwrap();
            assert!(
                matches!(res_a, Err(Error::Full(_))),
                "caller A should get worker startup error, got {res_a:?}"
            );
            let res_b = fut_b.await.unwrap();
            assert_eq!(res_b.unwrap(), 2);

            let mut dropped_ids = vec![
                dropped_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
                dropped_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            ];
            dropped_ids.sort_unstable();
            assert_eq!(dropped_ids, vec![1, 2]);
            assert_eq!(ran_a.load(Ordering::SeqCst), 0);
            assert_eq!(ran_b.load(Ordering::SeqCst), 1);
            assert_eq!(permits.available_permits(), 2);
            {
                let q = pool.shared.queue.lock();
                assert_eq!(q.live_workers, 1);
                assert!(q.permitted.is_empty());
                assert_eq!(pool.shared.permitted_queued.load(Ordering::SeqCst), 0);
            }
            drop(pool);
        });
    }

    #[test]
    fn cold_start_failure_without_followup_shuts_down_cleanly() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        // 1. Local queue with no follow-up tasks.
        {
            let pool = ComputePool::new(2);
            *pool.spawn_hook.lock() = Some(Arc::new(|_| {
                Some(Err(std::io::Error::other("always fail")))
            }));
            let permits = Arc::new(Semaphore::new(1));
            let mut p = Some(Arc::clone(&permits).try_acquire_owned().unwrap());
            let ran = Arc::new(AtomicUsize::new(0));
            let ran_c = Arc::clone(&ran);
            let err = pool
                .push_local(Box::new(move || {
                    drop(p.take());
                    ran_c.fetch_add(1, Ordering::SeqCst);
                    LocalStep::Finished
                }))
                .expect_err("spawn must fail");
            assert_eq!(err.0, PushErrorKind::WorkerSpawnFailed);
            drop(err.1);
            assert_eq!(ran.load(Ordering::SeqCst), 0);
            assert_eq!(permits.available_permits(), 1);
            {
                let q = pool.shared.queue.lock();
                assert_eq!(q.live_workers, 0);
                assert!(q.local.is_empty());
                assert_eq!(pool.shared.local_queued.load(Ordering::SeqCst), 0);
            }
            drop(pool);
        }

        // 2. Permitted queue with no follow-up tasks.
        rt.block_on(async {
            let pool = ComputePool::new(2);
            *pool.spawn_hook.lock() = Some(Arc::new(|_| {
                Some(Err(std::io::Error::other("always fail")))
            }));
            let permits = Arc::new(Semaphore::new(1));
            let p = Arc::clone(&permits).try_acquire_owned().unwrap();
            let ran = Arc::new(AtomicUsize::new(0));
            let ran_c = Arc::clone(&ran);
            let res = pool
                .run_permitted(move || {
                    let _p = p;
                    ran_c.fetch_add(1, Ordering::SeqCst);
                })
                .await;
            assert!(matches!(res, Err(Error::Full(_))));
            assert_eq!(ran.load(Ordering::SeqCst), 0);
            assert_eq!(permits.available_permits(), 1);
            {
                let q = pool.shared.queue.lock();
                assert_eq!(q.live_workers, 0);
                assert!(q.permitted.is_empty());
                assert_eq!(pool.shared.permitted_queued.load(Ordering::SeqCst), 0);
            }
            drop(pool);
        });
    }
}
