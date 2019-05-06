use serde_derive::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize, Hash)]
pub enum Action {
    AddPending,
    Proposed,
    Staged,
    Expired,
    AddOrphan,
    Committed,
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize, Hash)]
pub struct TxTrace {
    pub(crate) action: Action,
    pub(crate) info: String,
    pub(crate) time: u64,
}

impl TxTrace {
    pub fn new(action: Action, info: String, time: u64) -> TxTrace {
        TxTrace { action, info, time }
    }

    pub fn action(&self) -> &Action {
        &self.action
    }
}

impl fmt::Debug for TxTrace {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{{ action: {:?}, info: {}, time: {} }}",
            self.action, self.info, self.time
        )
    }
}

impl fmt::Display for TxTrace {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&self, f)
    }
}
