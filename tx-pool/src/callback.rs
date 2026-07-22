use super::component::TxEntry;
use crate::error::Reject;
use ckb_logger::error;
use std::panic::{AssertUnwindSafe, catch_unwind};

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

    /// Call on after pending
    pub fn call_pending(&self, entry: &TxEntry) {
        if let Some(call) = &self.pending {
            call_guarded("pending", || call(entry));
        }
    }

    /// Call on after proposed
    pub fn call_proposed(&self, entry: &TxEntry) {
        if let Some(call) = &self.proposed {
            call_guarded("proposed", || call(entry));
        }
    }

    /// Call on after reject
    pub fn call_reject(&self, entry: &TxEntry, reject: Reject) {
        if let Some(call) = &self.reject {
            call_guarded("reject", || call(entry, reject));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::call_guarded;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
}
