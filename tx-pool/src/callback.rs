use super::component::TxEntry;
use crate::error::Reject;
use crate::pool::TxPool;

/// Callback boxed fn pointer wrapper
pub type PendingCallback = Box<dyn Fn(&TxEntry) + Sync + Send>;
/// Proposed Callback boxed fn pointer wrapper
pub type ProposedCallback = Box<dyn Fn(&TxEntry) + Sync + Send>;
/// Reject Callback boxed fn pointer wrapper
pub type RejectCallback = Box<dyn Fn(&mut TxPool, &TxEntry, Reject) + Sync + Send>;

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
            call(entry)
        }
    }

    /// Call on after proposed
    pub fn call_proposed(&self, entry: &TxEntry) {
        if let Some(call) = &self.proposed {
            call(entry)
        }
    }

    /// Call on after reject
    pub fn call_reject(&self, tx_pool: &mut TxPool, entry: &TxEntry, reject: Reject) {
        if let Some(call) = &self.reject {
            call(tx_pool, entry, reject)
        }
    }
}
