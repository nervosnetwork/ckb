use super::component::entry::TxEntrySnapshot;
use crate::error::Reject;
use std::cell::Cell;

thread_local! {
    /// A committed-effect endpoint invokes callbacks on a blocking boundary.
    /// Mutating controller calls made directly by that callback can wait for
    /// effects whose FIFO publisher is awaiting the callback, so they must
    /// fail fast.
    ///
    /// This context is deliberately thread-local. A process-wide marker makes
    /// unrelated chain/RPC threads look re-entrant and can reject an
    /// authoritative reorg merely because a notification callback overlaps
    /// it. Callback ancestry cannot safely be inferred across arbitrary
    /// threads; callers must not join helpers which reenter the controller.
    /// Preview and template requests also need this context for reserved dispatch.
    static CALLBACK_THREAD: Cell<bool> = const { Cell::new(false) };
}

/// Execute one callback with re-entrant mutation detection scoped to the
/// current stack. Tokio may reuse a blocking-pool thread for unrelated work,
/// so the previous marker is restored even when callback code unwinds.
pub(crate) fn with_callback_context<T>(operation: impl FnOnce() -> T) -> T {
    struct CallbackContextGuard(bool);

    impl Drop for CallbackContextGuard {
        fn drop(&mut self) {
            CALLBACK_THREAD.with(|marked| marked.set(self.0));
        }
    }

    let previous = CALLBACK_THREAD.with(|marked| marked.replace(true));
    let _guard = CallbackContextGuard(previous);
    operation()
}

/// Detect direct callback reentry for mutation refusal and reserved read dispatch.
pub(crate) fn in_callback() -> bool {
    CALLBACK_THREAD.with(Cell::get)
}

/// Callback boxed fn pointer wrapper.
///
/// Callbacks receive a stable accounting snapshot rather than the full
/// resolved transaction, so deferred publication never pins resolved cell
/// metadata after pool ownership ends.
pub type PendingCallback = Box<dyn Fn(&TxEntrySnapshot) + Sync + Send>;
/// Proposed callback boxed fn pointer wrapper.
pub type ProposedCallback = Box<dyn Fn(&TxEntrySnapshot) + Sync + Send>;
/// Reject callback boxed fn pointer wrapper.
pub type RejectCallback = Box<dyn Fn(&TxEntrySnapshot, Reject) + Sync + Send>;

/// Synchronous notifications of committed pool changes.
///
/// Callbacks must return promptly. They execute without pool owner guards, but
/// ordered publication and service shutdown wait for their return. Direct
/// read-only controller calls are supported; mutating calls are not. Do not join
/// helper threads that reenter the controller: the callback's reserved-dispatch
/// context does not follow them. Panics disable callbacks for the generation;
/// asynchronous cancellation cannot interrupt an entered callback.
pub struct Callbacks {
    pub(crate) pending: Option<PendingCallback>,
    pub(crate) proposed: Option<ProposedCallback>,
    pub(crate) reject: Option<RejectCallback>,
}

#[derive(Clone)]
pub(crate) enum CallbackEvent {
    Pending(TxEntrySnapshot),
    Proposed(TxEntrySnapshot),
    Reject(TxEntrySnapshot, Reject),
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

    /// Dispatch one committed effect to its registered callback.
    /// The callback runs inline and must return before its batch can settle.
    pub(crate) fn publish(&self, event: &CallbackEvent) {
        match event {
            CallbackEvent::Pending(entry) => {
                if let Some(call) = &self.pending {
                    call(entry);
                }
            }
            CallbackEvent::Proposed(entry) => {
                if let Some(call) = &self.proposed {
                    call(entry);
                }
            }
            CallbackEvent::Reject(entry, reject) => {
                let reject = reject.clone();
                if let Some(call) = &self.reject {
                    call(entry, reject);
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/callback.rs"]
mod tests;
