use super::component::TxEntry;
use crate::error::Reject;

/// Callback boxed fn pointer wrapper
pub type Callback = Box<dyn Fn(TxEntry) + Sync + Send>;
/// Reject Callback boxed fn pointer wrapper
pub type RejectCallback = Box<dyn Fn(TxEntry, &Reject) + Sync + Send>;

/// Struct hold callbacks
pub struct Callbacks {
    pub(crate) pending: Vec<Callback>,
    pub(crate) proposed: Vec<Callback>,
    pub(crate) reject: Vec<RejectCallback>,
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
            pending: Vec::new(),
            proposed: Vec::new(),
            reject: Vec::new(),
        }
    }

    /// Register a new pending callback
    pub fn register_pending(&mut self, callback: Callback) {
        self.pending.push(callback);
    }

    /// Register a new proposed callback
    pub fn register_proposed(&mut self, callback: Callback) {
        self.proposed.push(callback);
    }

    /// Register a new abandon callback
    pub fn register_reject(&mut self, callback: RejectCallback) {
        self.reject.push(callback);
    }

    /// Call on after pending
    pub fn call_pending(&self, entry: TxEntry) {
        self.pending.iter().for_each(|call| call(entry.clone()))
    }

    /// Call on after proposed
    pub fn call_proposed(&self, entry: TxEntry) {
        self.proposed.iter().for_each(|call| call(entry.clone()))
    }

    /// Call on after reject
    pub fn call_reject(&self, entry: TxEntry, reject: &Reject) {
        self.reject
            .iter()
            .for_each(|call| call(entry.clone(), reject))
    }
}
