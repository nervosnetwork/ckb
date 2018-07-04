use bigint::{H256, U256};

#[derive(Debug, PartialEq, Clone, Eq)]
pub enum Error {
    Pow(PowError),
    Timestamp(TimestampError),
    Height(HeightError),
    Difficulty(DifficultyError),
    Transaction(Vec<(usize, TransactionError)>),
    EmptyTransactions,
    DuplicateTransactions,
    TransactionsRoot,
    MultipleCellbase,
    CellbaseNotAtFirst,
}

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
pub enum PowError {
    Boundary { expected: U256, actual: U256 },
    MixMismatch { expected: H256, actual: H256 },
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
pub struct DifficultyError {
    pub expected: U256,
    pub actual: U256,
}

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
pub enum TransactionError {
    NullNonCellbase,
    OutofBound,
    DuplicateInputs,
    Empty,
    InvalidCellbase,
}
