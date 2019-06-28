mod cost_model;
mod syscalls;
mod verify;

use ckb_vm::Error as VMInternalError;
use serde_derive::{Deserialize, Serialize};
use std::fmt;

pub use crate::verify::TransactionScriptsVerifier;

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

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
pub enum ScriptError {
    NoScript,
    InvalidReferenceIndex,
    ArgumentError,
    ValidationFailure(i8),
    VMError(VMInternalError),
    ExceededMaximumCycles,
    IOError,
    InvalidDaoDepositHeader,
    InvalidDaoWithdrawHeader,
    CapacityOverflow,
    InterestCalculation,
    InvalidSince,
    InvalidInterest,
    InvalidPubkeyHash,
    Secp,
    ArgumentNumber,
    NoWitness,
}
