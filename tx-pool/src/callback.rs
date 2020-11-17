use super::component::TxEntry;

/// Callback boxed fn pointer wrapper
pub type Callback = Box<dyn Fn(TxEntry) + Sync + Send>;

/// Struct hold callbacks
pub struct Callbacks {
    pub(crate) pending: Vec<Callback>,
    pub(crate) proposed: Vec<Callback>,
    pub(crate) abandon: Vec<Callback>,
}

impl Callbacks {
    /// Construct new Callbacks
    pub fn new() -> Self {
        Callbacks {
            pending: Vec::new(),
            proposed: Vec::new(),
            abandon: Vec::new(),
        }
    }

    /// Register a new pending callback
    pub fn register_pending(&mut self, callback: Callback) {
        self.pending.push(callback);
    }

    /// Register a new proposed callback
    pub fn register_proposed(&mut self, callback: Callback) {
        self.pending.push(callback);
    }

    /// Register a new abandon callback
    pub fn register_abandon(&mut self, callback: Callback) {
        self.pending.push(callback);
    }

    /// Call on after pending
    pub fn call_pending(&self, entry: &TxEntry) {
        self.pending.iter().for_each(|call| call(entry.clone()))
    }

    /// Call on after proposed
    pub fn call_proposed(&self, entry: &TxEntry) {
        self.proposed.iter().for_each(|call| call(entry.clone()))
    }

    /// Call on after abandon
    pub fn call_abandon(&self, entry: &TxEntry) {
        self.abandon.iter().for_each(|call| call(entry.clone()))
    }
}
