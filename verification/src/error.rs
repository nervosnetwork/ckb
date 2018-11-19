use bigint::{H256, U256};
use chain::error::Error as ChainError;
use core::BlockNumber;

#[derive(Debug, PartialEq, Clone, Eq)]
pub enum Error {
    Pow(PowError),
    Timestamp(TimestampError),
    Height(HeightError),
    Difficulty(DifficultyError),
    Transaction(Vec<(usize, TransactionError)>),
    Chain(ChainError),
    EmptyTransactions,
    DuplicateTransactions,
    TransactionsRoot,
    DuplicateHeader,
    InvalidInput,
    InvalidOutput,
    UnknownParent(H256),
    Uncles(UnclesError),
    Cellbase(CellbaseError),
    Commit(CommitError),
}

#[derive(Debug, PartialEq, Clone, Eq)]
pub enum CommitError {
    AncestorNotFound,
    Confilct,
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
pub struct HeightError {
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
    InvalidSignature,
    DoubleSpent,
    UnknownInput,
}

impl From<ChainError> for Error {
    fn from(e: ChainError) -> Self {
        Error::Chain(e)
    }
}
