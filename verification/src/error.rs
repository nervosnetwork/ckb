use bigint::{H256, U256};

#[derive(Debug, PartialEq, Clone, Eq)]
pub enum Error {
    Pow(InvalidPow),
    Timestamp(InvalidTimestamp),
    Height(InvalidHeight),
    Difficulty(InvalidDifficulty),
    Transaction(Vec<(usize, TransactionError)>),
    EmptyTransactions,
    DuplicateTransactions,
    TransactionsRoot,
}

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
pub enum InvalidPow {
    Boundary { expected: U256, actual: U256 },
    MixMismatch { expected: H256, actual: H256 },
}

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
pub struct InvalidTimestamp {
    pub min: Option<u64>,
    pub max: Option<u64>,
    pub found: u64,
}

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
pub struct InvalidHeight {
    pub expected: u64,
    pub actual: u64,
}

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
pub struct InvalidDifficulty {
    pub expected: U256,
    pub actual: U256,
}

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
pub enum TransactionError {
    NullNonCellbase,
    OutofBound,
    DuplicateInputs,
    Empty,
}
