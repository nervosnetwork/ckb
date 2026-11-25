//! Bounded request admission, startup readiness synchronization, and graceful
//! shutdown drain barrier for direct `Pool` execution.
//!
//! Replaces the legacy `Message` enum, dual `mpsc` queues, `Mutex<Receiver>`,
//! and native `RequestWorker` OS threads while preserving fail-fast backpressure,
//! callback read isolation, pre-start waiting, and graceful shutdown drain.

use ckb_error::{AnyError, OtherError};
use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicUsize, Ordering},
};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, TryAcquireError};

const STATE_STARTING: u8 = 0;
const STATE_OPEN: u8 = 1;
const STATE_CLOSED: u8 = 2;

/// Capacity lane enforced by `RequestGate`.
///
/// External reads and mutations are bounded independently; nested publication
/// and replay callbacks use their own bounded read lane so saturated external
/// pre-start readers can never starve a callback running on the publisher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AdmissionLane {
    Ordinary,
    Read,
    CallbackRead,
}

impl AdmissionLane {
    pub(crate) fn for_read(in_callback: bool) -> Self {
        if in_callback {
            Self::CallbackRead
        } else {
            Self::Read
        }
    }

    pub(crate) fn for_ordinary(in_callback: bool) -> Self {
        if in_callback {
            Self::CallbackRead
        } else {
            Self::Ordinary
        }
    }
}

/// Shared lifecycle, backpressure, and drain boundary between
/// `TxPoolServiceBuilder` and every clone of `TxPoolController`.
pub(crate) struct RequestGate {
    state: AtomicU8,
    state_changed: Notify,
    ordinary: Arc<Semaphore>,
    reads: Arc<Semaphore>,
    callback_reads: Arc<Semaphore>,
    inflight: AtomicUsize,
    drained: Notify,
}

/// RAII slot proving a direct request holds bounded admission capacity and is
/// tracked by the shutdown drain barrier.
pub(crate) struct RequestPermit {
    gate: Arc<RequestGate>,
    permit: Option<OwnedSemaphorePermit>,
}

impl Drop for RequestPermit {
    fn drop(&mut self) {
        // Release the lane semaphore permit before decrementing `inflight` so
        // LIFO acquire/release ordering holds: any observer seeing `inflight`
        // reach zero (including `wait_drained()` and callers observing normal
        // completion) is guaranteed to see all lane permits already reclaimed.
        drop(self.permit.take());
        let remaining = self
            .gate
            .inflight
            .fetch_sub(1, Ordering::SeqCst)
            .saturating_sub(1);
        if remaining == 0 && self.gate.is_closed() {
            self.gate.drained.notify_waiters();
        }
    }
}

pub(crate) fn closed_gate_error() -> AnyError {
    OtherError::new("TrySendError channel closed".to_owned()).into()
}

pub(crate) fn full_gate_error() -> AnyError {
    OtherError::new("TrySendError no available capacity".to_owned()).into()
}

impl RequestGate {
    pub(crate) fn new(ordinary_capacity: usize, read_capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            state: AtomicU8::new(STATE_STARTING),
            state_changed: Notify::new(),
            ordinary: Arc::new(Semaphore::new(ordinary_capacity)),
            reads: Arc::new(Semaphore::new(read_capacity)),
            callback_reads: Arc::new(Semaphore::new(read_capacity)),
            inflight: AtomicUsize::new(0),
            drained: Notify::new(),
        })
    }

    pub(crate) fn is_open(&self) -> bool {
        self.state.load(Ordering::SeqCst) == STATE_OPEN
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.state.load(Ordering::SeqCst) == STATE_CLOSED
    }

    /// Transition `Starting -> Open` after persistence replay finishes, waking
    /// all pre-start callers. A previously closed gate never reopens.
    pub(crate) fn open(&self) {
        if self
            .state
            .compare_exchange(
                STATE_STARTING,
                STATE_OPEN,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
        {
            self.state_changed.notify_waiters();
        }
    }

    /// Permanently close request intake, release pre-start waiters, and notify
    /// the drain barrier immediately if no requests are currently in flight.
    pub(crate) fn close(&self) {
        self.state.store(STATE_CLOSED, Ordering::SeqCst);
        self.ordinary.close();
        self.reads.close();
        self.callback_reads.close();
        self.state_changed.notify_waiters();
        if self.inflight.load(Ordering::SeqCst) == 0 {
            self.drained.notify_waiters();
        }
    }

    /// Synchronously admit one direct request on its bounded lane.
    pub(crate) fn try_admit(
        self: &Arc<Self>,
        lane: AdmissionLane,
    ) -> Result<RequestPermit, AnyError> {
        if self.is_closed() {
            return Err(closed_gate_error());
        }
        let semaphore = match lane {
            AdmissionLane::Ordinary => &self.ordinary,
            AdmissionLane::Read => &self.reads,
            AdmissionLane::CallbackRead => &self.callback_reads,
        };
        let permit = match Arc::clone(semaphore).try_acquire_owned() {
            Ok(permit) => permit,
            Err(TryAcquireError::Closed) => return Err(closed_gate_error()),
            Err(TryAcquireError::NoPermits) => return Err(full_gate_error()),
        };
        self.inflight.fetch_add(1, Ordering::SeqCst);
        let guard = RequestPermit {
            gate: Arc::clone(self),
            permit: Some(permit),
        };
        if self.is_closed() {
            drop(guard);
            return Err(closed_gate_error());
        }
        Ok(guard)
    }

    /// Wait until the service has finished startup replay (or immediately
    /// proceed when `allow_prestart` is true for nested replay callbacks).
    pub(crate) async fn wait_ready(&self, allow_prestart: bool) -> Result<(), AnyError> {
        match self.state.load(Ordering::SeqCst) {
            STATE_OPEN => return Ok(()),
            STATE_CLOSED => return Err(closed_gate_error()),
            _ if allow_prestart => return Ok(()),
            _ => {}
        }
        loop {
            let notified = self.state_changed.notified();
            match self.state.load(Ordering::SeqCst) {
                STATE_OPEN => return Ok(()),
                STATE_CLOSED => return Err(closed_gate_error()),
                _ => notified.await,
            }
        }
    }

    /// Wait until every admitted direct request has exited and dropped its
    /// `RequestPermit`. Must be called after `close()`.
    pub(crate) async fn wait_drained(&self) {
        loop {
            let notified = self.drained.notified();
            if self.inflight.load(Ordering::SeqCst) == 0 {
                return;
            }
            notified.await;
        }
    }

    #[cfg(test)]
    pub(crate) fn ordinary_available(&self) -> usize {
        self.ordinary.available_permits()
    }

    #[cfg(test)]
    pub(crate) fn read_available(&self) -> usize {
        self.reads.available_permits()
    }

    #[cfg(any(test, feature = "internal"))]
    pub(crate) fn inflight(&self) -> usize {
        self.inflight.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    pub(crate) fn try_reserve_ordinary(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.ordinary).try_acquire_owned().ok()
    }

    #[cfg(test)]
    pub(crate) fn try_reserve_read(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.reads).try_acquire_owned().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;

    #[test]
    fn inflight_zero_observes_reclaimed_lane_permits() {
        const ITERATIONS: usize = 512;
        let gate = RequestGate::new(1, 1);
        gate.open();

        let start = Arc::new(Barrier::new(2));
        let done = Arc::new(Barrier::new(2));
        let observer_gate = Arc::clone(&gate);
        let observer_start = Arc::clone(&start);
        let observer_done = Arc::clone(&done);

        let observer = std::thread::spawn(move || {
            for _ in 0..ITERATIONS {
                for lane in [
                    AdmissionLane::Ordinary,
                    AdmissionLane::Read,
                    AdmissionLane::CallbackRead,
                ] {
                    observer_start.wait();
                    while observer_gate.inflight() != 0 {
                        std::hint::spin_loop();
                    }
                    match lane {
                        AdmissionLane::Ordinary => assert_eq!(
                            observer_gate.ordinary_available(),
                            1,
                            "ordinary lane permit must be reclaimed before inflight drops to zero"
                        ),
                        AdmissionLane::Read => assert_eq!(
                            observer_gate.read_available(),
                            1,
                            "read lane permit must be reclaimed before inflight drops to zero"
                        ),
                        AdmissionLane::CallbackRead => assert_eq!(
                            observer_gate.callback_reads.available_permits(),
                            1,
                            "callback read permit must be reclaimed before inflight drops to zero"
                        ),
                    }
                    observer_done.wait();
                }
            }
        });

        for _ in 0..ITERATIONS {
            for lane in [
                AdmissionLane::Ordinary,
                AdmissionLane::Read,
                AdmissionLane::CallbackRead,
            ] {
                let permit = gate.try_admit(lane).expect("lane permit is available");
                assert_eq!(gate.inflight(), 1);
                start.wait();
                drop(permit);
                done.wait();
            }
        }

        observer.join().expect("observer thread completed cleanly");
    }
}
