use ckb_core::BlockNumber;
use ckb_occupied_capacity::Error as CapacityError;
use ckb_script::ScriptError;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::error::Error as StdError;
use std::fmt;

/// Block verification error

/// Should we use ErrorKind pattern?
/// Those error kind carry some data that provide additional information,
/// ErrorKind pattern should only carry stateless data. And, our ErrorKind can not be `Eq`.
/// If the Rust community has better patterns in the future, then look back here
#[derive(Debug, PartialEq)]
pub enum Error {
    /// PoW proof is corrupt or does not meet the difficulty target.
    Pow(PowError),
    /// The field timestamp in block header is invalid.
    Timestamp(TimestampError),
    /// The field number in block header is invalid.
    Number(NumberError),
    /// The field difficulty in block header is invalid.
    Epoch(EpochError),
    /// Committed transactions verification error. It contains error for the first transaction that
    /// fails the verification. The errors are stored as a tuple, where the first item is the
    /// transaction index in the block and the second item is the transaction verification error.
    Transactions((usize, TransactionError)),
    /// This is a wrapper of error encountered when invoking chain API.
    Chain(String),
    /// There are duplicate proposed transactions.
    ProposalTransactionDuplicate,
    /// There are duplicate committed transactions.
    CommitTransactionDuplicate,
    /// The merkle tree hash of proposed transactions does not match the one in header.
    ProposalTransactionsRoot,
    /// The merkle tree hash of committed transactions does not match the one in header.
    CommitTransactionsRoot,
    /// The merkle tree witness hash of committed transactions does not match the one in header.
    WitnessesMerkleRoot,
    /// The parent of the block is unknown.
    UnknownParent(H256),
    /// Uncles does not meet the consensus requirements.
    Uncles(UnclesError),
    /// Cellbase transaction is invalid.
    Cellbase(CellbaseError),
    /// This error is returned when the committed transactions does not meet the 2-phases
    /// propose-then-commit consensus rule.
    Commit(CommitError),
    /// Cycles consumed by all scripts in all commit transactions of the block exceed
    /// the maximum allowed cycles in consensus rules
    ExceededMaximumCycles,
    /// Number of proposals exceeded the limit.
    ExceededMaximumProposalsLimit,
    /// The size of the block exceeded the limit.
    ExceededMaximumBlockBytes,
    /// The field version in block header is not allowed.
    Version,
    /// Overflow when do computation for capacity.
    CapacityOverflow,
    /// Error fetching block reward,
    CannotFetchBlockReward,
    /// Fee calculation error
    FeeCalculation,
    /// Error generating DAO field
    DAOGeneration,
    /// Invalid data in DAO header field
    InvalidDAO,
}

impl StdError for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::UnknownParent(h) => write!(f, "UnknownParent({:#x})", h),
            _ => fmt::Debug::fmt(&self, f),
        }
    }
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
    InvalidRewardAmount,
    InvalidRewardTarget,
    InvalidWitness,
    InvalidTypeScript,
    InvalidQuantity,
    InvalidPosition,
}

#[derive(Debug, PartialEq, Clone, Eq)]
pub enum UnclesError {
    OverCount {
        max: u32,
        actual: u32,
    },
    MissMatchCount {
        expected: u32,
        actual: u32,
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
    InvalidNumber,
    InvalidDifficulty,
    InvalidDifficultyEpoch,
    InvalidProof,
    ProposalsHash,
    ProposalDuplicate,
    Duplicate(H256),
    DoubleInclusion(H256),
    InvalidCellbase,
    ExceededMaximumProposalsLimit,
}

#[derive(Debug, PartialEq, Clone, Eq)]
pub enum PowError {
    Boundary { expected: U256, actual: U256 },
    InvalidProof,
}

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
pub enum TimestampError {
    BlockTimeTooOld { min: u64, found: u64 },
    BlockTimeTooNew { max: u64, found: u64 },
}

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
pub struct NumberError {
    pub expected: u64,
    pub actual: u64,
}

#[derive(Debug, PartialEq, Clone, Eq)]
pub enum EpochError {
    DifficultyMismatch { expected: U256, actual: U256 },
    NumberMismatch { expected: u64, actual: u64 },
    AncestorNotFound,
}

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
pub enum TransactionError {
    /// Occur output's bytes_len exceed capacity
    CapacityOverflow,
    /// In a single output cell, the capacity is not enough to hold the cell serialized size
    InsufficientCellCapacity,
    DuplicateDeps,
    Empty,
    /// Sum of all outputs capacity exceed sum of all inputs in the transaction
    OutputsSumOverflow,
    InvalidScript,
    ScriptFailure(ScriptError),
    InvalidSignature,
    Version,
    /// Tx not satisfied since condition
    Immature,
    /// Invalid Since flags
    InvalidSince,
    CellbaseImmaturity,
    ExceededMaximumBlockBytes,
}

impl StdError for TransactionError {}

impl fmt::Display for TransactionError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&self, f)
    }
}

impl TransactionError {
    /// Transaction error may be caused by different tip between peers if this method return false,
    /// Otherwise we consider the Bad Tx is constructed intendedly.
    pub fn is_bad_tx(self) -> bool {
        use TransactionError::*;
        match self {
            CapacityOverflow | DuplicateDeps | Empty | OutputsSumOverflow | InvalidScript
            | ScriptFailure(_) | InvalidSignature | InvalidSince => true,
            _ => false,
        }
    }
}

impl From<CapacityError> for TransactionError {
    fn from(error: CapacityError) -> Self {
        match error {
            CapacityError::Overflow => TransactionError::CapacityOverflow,
        }
    }
}

impl From<CapacityError> for Error {
    fn from(error: CapacityError) -> Self {
        match error {
            CapacityError::Overflow => Error::CapacityOverflow,
        }
    }
}
