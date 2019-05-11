mod cost_model;
mod syscalls;
mod verify;

use ckb_vm::Error as VMInternalError;
use serde_derive::{Deserialize, Serialize};

pub use crate::syscalls::build_tx;
pub use crate::verify::TransactionScriptsVerifier;

#[derive(Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Debug)]
pub enum Runner {
    Assembly,
    Rust,
}

impl Default for Runner {
    fn default() -> Runner {
        Runner::Assembly
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
    InvalidIssuingDaoInput,
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
}
