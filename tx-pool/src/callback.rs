use super::component::TxEntry;
use crate::error::Reject;
use ckb_logger::error;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

/// User-supplied callbacks are side effects, not part of the authoritative
/// pool transition. Contain their panics so a notifier cannot unwind a
/// completed pool mutation, make a reorg delta retry forever, or kill a
/// pipeline worker.
fn call_guarded(name: &'static str, call: impl FnOnce()) {
    if let Err(payload) = catch_unwind(AssertUnwindSafe(call)) {
        error!(
            "tx-pool {} callback panicked: {}",
            name,
            crate::util::panic_payload_to_string(payload.as_ref())
        );
    }
}

/// Callback boxed fn pointer wrapper
pub type PendingCallback = Box<dyn Fn(&TxEntry) + Sync + Send>;
/// Proposed Callback boxed fn pointer wrapper
pub type ProposedCallback = Box<dyn Fn(&TxEntry) + Sync + Send>;
/// Reject Callback boxed fn pointer wrapper
pub type RejectCallback = Box<dyn Fn(&TxEntry, Reject) + Sync + Send>;

/// Struct hold callbacks
pub struct Callbacks {
    pub(crate) pending: Option<PendingCallback>,
    pub(crate) proposed: Option<ProposedCallback>,
    pub(crate) reject: Option<RejectCallback>,
    deferred: Mutex<DeferredCallbacks>,
}

#[derive(Default)]
struct DeferredCallbacks {
    depth: usize,
    events: Vec<CallbackEvent>,
}

enum CallbackEvent {
    Pending(TxEntry),
    Proposed(TxEntry),
    Reject(TxEntry, Reject),
}

/// RAII effect barrier. The guard must be declared before the authoritative
/// lock it protects; reverse drop order then releases that lock before queued
/// user callbacks are published, including during unwinding/cancellation.
pub(crate) struct CallbackDeferral {
    callbacks: Arc<Callbacks>,
}

impl Drop for CallbackDeferral {
    fn drop(&mut self) {
        self.callbacks.finish_deferral();
    }
}

impl Default for Callbacks {
    fn default() -> Self {
        Self::new()
    }
}

impl Callbacks {
    /// Construct new Callbacks
    pub fn new() -> Self {
        Callbacks {
            pending: None,
            proposed: None,
            reject: None,
            deferred: Mutex::new(DeferredCallbacks::default()),
        }
    }

    /// Defer every callback until the outermost returned guard is dropped.
    /// This is process-wide for the callback set: concurrent submissions may
    /// be delayed briefly by a reorg, but no callback can re-enter partially
    /// reconciled state.
    pub(crate) fn defer(self: &Arc<Self>) -> CallbackDeferral {
        let mut state = self.deferred.lock().unwrap_or_else(|e| e.into_inner());
        state.depth = state
            .depth
            .checked_add(1)
            .expect("callback deferral nesting exhausted");
        drop(state);
        CallbackDeferral {
            callbacks: Arc::clone(self),
        }
    }

    /// Register a new pending callback
    pub fn register_pending(&mut self, callback: PendingCallback) {
        self.pending = Some(callback);
    }

    /// Register a new proposed callback
    pub fn register_proposed(&mut self, callback: ProposedCallback) {
        self.proposed = Some(callback);
    }

    /// Register a new abandon callback
    pub fn register_reject(&mut self, callback: RejectCallback) {
        self.reject = Some(callback);
    }

    /// Call on after pending
    pub fn call_pending(&self, entry: &TxEntry) {
        if self.pending.is_none() {
            return;
        }
        if let Err(CallbackEvent::Pending(entry)) =
            self.enqueue_if_deferred(CallbackEvent::Pending(entry.clone()))
        {
            self.call_pending_now(&entry);
        }
    }

    /// Call on after proposed
    pub fn call_proposed(&self, entry: &TxEntry) {
        if self.proposed.is_none() {
            return;
        }
        if let Err(CallbackEvent::Proposed(entry)) =
            self.enqueue_if_deferred(CallbackEvent::Proposed(entry.clone()))
        {
            self.call_proposed_now(&entry);
        }
    }

    /// Call on after reject
    pub fn call_reject(&self, entry: &TxEntry, reject: Reject) {
        if self.reject.is_none() {
            return;
        }
        if let Err(CallbackEvent::Reject(entry, reject)) =
            self.enqueue_if_deferred(CallbackEvent::Reject(entry.clone(), reject))
        {
            self.call_reject_now(&entry, reject);
        }
    }

    fn enqueue_if_deferred(&self, event: CallbackEvent) -> Result<(), CallbackEvent> {
        let mut state = self.deferred.lock().unwrap_or_else(|e| e.into_inner());
        if state.depth == 0 {
            return Err(event);
        }
        state.events.push(event);
        Ok(())
    }

    fn finish_deferral(&self) {
        let events = {
            let mut state = self.deferred.lock().unwrap_or_else(|e| e.into_inner());
            debug_assert!(state.depth > 0, "unbalanced callback deferral guard");
            state.depth = state.depth.saturating_sub(1);
            if state.depth == 0 {
                std::mem::take(&mut state.events)
            } else {
                Vec::new()
            }
        };
        for event in events {
            match event {
                CallbackEvent::Pending(entry) => self.call_pending_now(&entry),
                CallbackEvent::Proposed(entry) => self.call_proposed_now(&entry),
                CallbackEvent::Reject(entry, reject) => self.call_reject_now(&entry, reject),
            }
        }
    }

    fn call_pending_now(&self, entry: &TxEntry) {
        if let Some(call) = &self.pending {
            call_guarded("pending", || call(entry));
        }
    }

    fn call_proposed_now(&self, entry: &TxEntry) {
        if let Some(call) = &self.proposed {
            call_guarded("proposed", || call(entry));
        }
    }

    fn call_reject_now(&self, entry: &TxEntry, reject: Reject) {
        if let Some(call) = &self.reject {
            call_guarded("reject", || call(entry, reject));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Callbacks, call_guarded};
    use crate::component::entry::TxEntry;
    use ckb_types::core::{Capacity, TransactionBuilder};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn guarded_callback_panic_cannot_escape_or_block_later_callbacks() {
        let calls = AtomicUsize::new(0);

        call_guarded("test", || {
            calls.fetch_add(1, Ordering::SeqCst);
            panic!("injected callback panic");
        });
        call_guarded("test", || {
            calls.fetch_add(1, Ordering::SeqCst);
        });

        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn deferral_publishes_only_after_outer_guard_drops() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut callbacks = Callbacks::new();
        let observed = Arc::clone(&calls);
        callbacks.register_pending(Box::new(move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
        }));
        let callbacks = Arc::new(callbacks);
        let entry = TxEntry::dummy_resolve(
            TransactionBuilder::default().build(),
            0,
            Capacity::zero(),
            0,
        );

        let outer = callbacks.defer();
        let inner = callbacks.defer();
        callbacks.call_pending(&entry);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        drop(inner);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        drop(outer);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
