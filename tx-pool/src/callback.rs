use super::component::TxEntry;
use crate::error::Reject;
use ckb_logger::error;
use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};

thread_local! {
    /// Callbacks are synchronous, so a thread-local depth guard accurately
    /// marks their complete execution interval even when the Tokio task may
    /// migrate at later await points. Controller mutations use this to fail
    /// fast instead of forming publisher/recovery/effect-capacity cycles.
    static CALLBACK_DEPTH: Cell<usize> = const { Cell::new(0) };
}

struct CallbackContextGuard;

impl CallbackContextGuard {
    fn enter() -> Self {
        CALLBACK_DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
        Self
    }
}

impl Drop for CallbackContextGuard {
    fn drop(&mut self) {
        CALLBACK_DEPTH.with(|depth| {
            depth.set(
                depth
                    .get()
                    .checked_sub(1)
                    .expect("callback context depth is balanced"),
            )
        });
    }
}

/// Read-only controller calls are safe from callbacks. Synchronous mutations
/// are not: they can wait for the same publisher that is executing the
/// callback, directly or through `recovery_lock`/effect capacity.
pub(crate) fn in_callback() -> bool {
    CALLBACK_DEPTH.with(|depth| depth.get() != 0)
}

/// User-supplied callbacks are side effects, not part of the authoritative
/// pool transition. Contain their panics so a notifier cannot unwind a
/// completed pool mutation, make a reorg delta retry forever, or kill a
/// pipeline worker.
fn call_guarded(name: &'static str, call: impl FnOnce()) {
    let _context = CallbackContextGuard::enter();
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
}

pub(crate) enum CallbackEvent {
    Pending(TxEntry),
    Proposed(TxEntry),
    Reject(TxEntry, Reject),
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

    /// Publish one already-journaled callback effect.
    ///
    /// The effect publisher is itself the stable-state barrier, so this path
    /// deliberately bypasses the legacy in-task deferral queue. It is the only
    /// callback entry point used by the production effect outbox.
    pub(crate) fn publish(&self, event: &CallbackEvent) {
        match event {
            CallbackEvent::Pending(entry) => self.call_pending_now(entry),
            CallbackEvent::Proposed(entry) => self.call_proposed_now(entry),
            CallbackEvent::Reject(entry, reject) => self.call_reject_now(entry, reject.clone()),
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
    use super::{Callbacks, call_guarded, in_callback};
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
    fn publish_dispatches_the_typed_event() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut callbacks = Callbacks::new();
        let observed = Arc::clone(&calls);
        callbacks.register_pending(Box::new(move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
        }));
        let entry = TxEntry::dummy_resolve(
            TransactionBuilder::default().build(),
            0,
            Capacity::zero(),
            0,
        );

        callbacks.publish(&super::CallbackEvent::Pending(entry));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn callback_context_is_scoped_across_panics_and_nested_calls() {
        assert!(!in_callback());
        call_guarded("outer", || {
            assert!(in_callback());
            call_guarded("inner", || assert!(in_callback()));
            assert!(in_callback());
            panic!("injected callback panic");
        });
        assert!(!in_callback());
    }
}
