use bigint::{H256, U256};
use chain::error::Error as ChainError;
use core::BlockNumber;
use script::Error as ScriptError;

/// Block verification error
#[derive(Debug, PartialEq, Clone, Eq)]
pub enum Error {
    /// PoW proof is corrupt or does not meet the difficulty target.
    Pow(PowError),
    /// The field timestamp in block header is invalid.
    Timestamp(TimestampError),
    /// The field number in block header is invalid.
    Number(NumberError),
    /// The field difficulty in block header is invalid.
    Difficulty(DifficultyError),
    /// Committed transactions verification error. It contains errors for all the transactions that
    /// fail the verification. The errors are stored as a Vec of tuple, where the first item is the
    /// transaction index in the block and the second item is the transaction verification error.
    Transactions(Vec<(usize, TransactionError)>),
    /// This is a wrapper of error encountered when invoking chain API.
    Chain(ChainError),
    /// The committed transactions list is empty.
    CommitTransactionsEmpty,
    /// There are duplicate proposed transactions.
    ProposalTransactionDuplicate,
    /// There are duplicate committed transactions.
    CommitTransactionDuplicate,
    /// The merkle tree hash of proposed transactions does not match the one in header.
    ProposalTransactionsRoot,
    /// The merkle tree hash of committed transactions does not match the one in header.
    CommitTransactionsRoot,
    /// The parent of the block is unknown.
    UnknownParent(H256),
    /// Uncles does not meet the consensus requirements.
    Uncles(UnclesError),
    /// Cellbase transaction is invalid.
    Cellbase(CellbaseError),
    /// This error is returned when the committed transactions does not meet the 2-phases
    /// propose-then-commit consensus rule.
    Commit(CommitError),
}

#[derive(Debug, PartialEq, Clone, Eq)]
pub enum CommitError {
    /// Ancestor not found, should not happen, we check header first and check ancestor.
    AncestorNotFound,
    /// Break propose-then-commit consensus rule.
    Invalid,
}

#[derive(Debug, PartialEq, Clone, Eq)]
pub enum CellbaseError {
    InvalidInput,
    InvalidReward,
    InvalidQuantity,
    InvalidPosition,
}

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
pub enum UnclesError {
    OverLength {
        max: usize,
        actual: usize,
    },
    InvalidDepth {
        max: BlockNumber,
        min: BlockNumber,
        actual: BlockNumber,
    },
    InvalidHash {
        expected: H256,
        actual: H256,
    },
    Duplicate(H256),
    InvalidInclude(H256),
    InvalidCellbase,
}

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
pub enum PowError {
    Boundary { expected: U256, actual: U256 },
    InvalidProof,
}

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
pub enum TimestampError {
    ZeroBlockTime { min: u64, found: u64 },
    FutureBlockTime { max: u64, found: u64 },
}

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
pub struct NumberError {
    pub expected: u64,
    pub actual: u64,
}

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
pub enum DifficultyError {
    MixMismatch { expected: U256, actual: U256 },
    AncestorNotFound,
}

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
pub enum TransactionError {
    NullInput,
    OutofBound,
    DuplicateInputs,
    Empty,
    InvalidCapacity,
    InvalidScript,
    ScriptFailure(ScriptError),
    InvalidSignature,
    DoubleSpent,
    UnknownInput,
}

impl From<ChainError> for Error {
    fn from(e: ChainError) -> Self {
        Error::Chain(e)
    }
}
