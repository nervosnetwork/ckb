//! CKB component to run the type/lock scripts.
pub mod cost_model;
mod error;
mod ill_transaction_checker;
mod syscalls;
mod type_id;
mod types;
mod verify;
mod verify_env;

pub use crate::error::{ScriptError, TransactionScriptError};
pub use crate::ill_transaction_checker::IllTransactionChecker;
pub use crate::types::{ScriptGroup, ScriptGroupType};
pub use crate::verify::TransactionScriptsVerifier;
pub use crate::verify_env::TxVerifyEnv;
