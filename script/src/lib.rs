//! TODO(doc): @doitian
pub mod cost_model;
mod error;
mod ill_transaction_checker;
mod syscalls;
mod type_id;
mod types;
mod verify;

pub use crate::error::{ScriptError, TransactionScriptError};
pub use crate::ill_transaction_checker::IllTransactionChecker;
pub use crate::types::{ScriptGroup, ScriptGroupType};
pub use crate::verify::TransactionScriptsVerifier;
