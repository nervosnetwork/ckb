//! CKB verification
//!
//! This crate implements CKB non-contextual verification by newtypes abstraction struct
mod block_verifier;
pub mod cache;
mod convert;
mod error;
mod genesis_verifier;
mod header_verifier;
mod transaction_verifier;

#[cfg(test)]
mod tests;

pub use crate::block_verifier::{BlockVerifier, NonContextualBlockTxsVerifier};
pub use crate::error::{
    BlockError, BlockErrorKind, BlockTransactionsError, BlockVersionError, CellbaseError,
    CommitError, EpochError, HeaderError, HeaderErrorKind, InvalidParentError, NumberError,
    PowError, TimestampError, TransactionError, UnclesError, UnknownParentError,
};
pub use crate::genesis_verifier::GenesisVerifier;
pub use crate::header_verifier::HeaderVerifier;
pub use crate::transaction_verifier::{
    CapacityVerifier, ContextualTransactionVerifier, ContextualWithoutScriptTransactionVerifier,
    DaoScriptSizeVerifier, NonContextualTransactionVerifier, ScriptVerifier, Since, SinceMetric,
    TimeRelativeTransactionVerifier,
};
pub use ckb_script::{
    ScriptError, ScriptGroupType, TransactionSnapshot, TransactionState as ScriptVerifyState,
    TxVerifyEnv, VerifyResult as ScriptVerifyResult,
};

/// Maximum amount of time that a block timestamp is allowed to exceed the
/// current time before the block will be accepted.
pub const ALLOWED_FUTURE_BLOCKTIME: u64 = 15 * 1000; // 15 Second
