use super::component::TxEntry;
use crate::error::Reject;
use crate::pool::TxPool;

/// Callback boxed fn pointer wrapper
pub type Callback = Box<dyn Fn(&mut TxPool, &TxEntry) + Sync + Send>;
/// Proposed Callback boxed fn pointer wrapper
pub type ProposedCallback = Box<dyn Fn(&mut TxPool, &TxEntry, bool) + Sync + Send>;
/// Reject Callback boxed fn pointer wrapper
pub type RejectCallback = Box<dyn Fn(&mut TxPool, &TxEntry, Reject) + Sync + Send>;

/// Struct hold callbacks
pub struct Callbacks {
    pub(crate) pending: Option<Callback>,
    pub(crate) proposed: Option<ProposedCallback>,
    pub(crate) committed: Option<Callback>,
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
            committed: None,
            reject: None,
        }
    }

    /// Register a new pending callback
    pub fn register_pending(&mut self, callback: Callback) {
        self.pending = Some(callback);
    }

    /// Register a new proposed callback
    pub fn register_proposed(&mut self, callback: ProposedCallback) {
        self.proposed = Some(callback);
    }

    /// Register a new committed callback
    pub fn register_committed(&mut self, callback: Callback) {
        self.committed = Some(callback);
    }

    /// Register a new abandon callback
    pub fn register_reject(&mut self, callback: RejectCallback) {
        self.reject = Some(callback);
    }

    /// Call on after pending
    pub fn call_pending(&self, tx_pool: &mut TxPool, entry: &TxEntry) {
        if let Some(call) = &self.pending {
            call(tx_pool, entry)
        }
    }

    /// Call on after proposed
    pub fn call_proposed(&self, tx_pool: &mut TxPool, entry: &TxEntry, new: bool) {
        if let Some(call) = &self.proposed {
            call(tx_pool, entry, new)
        }
    }

    /// Call on after proposed
    pub fn call_committed(&self, tx_pool: &mut TxPool, entry: &TxEntry) {
        if let Some(call) = &self.committed {
            call(tx_pool, entry)
        }
    }

    /// Call on after reject
    pub fn call_reject(&self, tx_pool: &mut TxPool, entry: &TxEntry, reject: Reject) {
        if let Some(call) = &self.reject {
            call(tx_pool, entry, reject)
        }
    }
}
