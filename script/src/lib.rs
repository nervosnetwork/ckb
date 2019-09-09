mod cost_model;
mod error;
mod syscalls;
mod type_id;
mod verify;

use serde_derive::{Deserialize, Serialize};
use std::fmt;

pub use crate::error::ScriptError;
pub use crate::verify::{ScriptGroup, ScriptGroupType, TransactionScriptsVerifier};

/// re-export DataLoader
pub use ckb_script_data_loader::DataLoader;

#[derive(Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Debug)]
pub enum Runner {
    #[cfg(all(unix, target_pointer_width = "64"))]
    Assembly,
    Rust,
}

impl fmt::Display for Runner {
    #[cfg(all(unix, target_pointer_width = "64"))]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Runner::Assembly => write!(f, "Assembly"),
            Runner::Rust => write!(f, "Rust"),
        }
    }

    #[cfg(not(all(unix, target_pointer_width = "64")))]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Runner::Rust => write!(f, "Rust"),
        }
    }
}

impl Default for Runner {
    #[cfg(all(unix, target_pointer_width = "64"))]
    fn default() -> Runner {
        Runner::Assembly
    }

    #[cfg(not(all(unix, target_pointer_width = "64")))]
    fn default() -> Runner {
        Runner::Rust
    }
}

#[derive(Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Debug, Default)]
pub struct ScriptConfig {
    pub runner: Runner,
}
