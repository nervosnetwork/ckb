use ckb_error::Error;
use ckb_types::packed::Byte32;
use failure::{Backtrace, Context, Fail};
use std::fmt::{self, Display};

#[derive(Fail, Debug, PartialEq, Eq, Clone, Display)]
pub enum TransactionError {
    /// output.occupied_capacity() > output.capacity()
    InsufficientCellCapacity,

    /// SUM([o.capacity for o in outputs]) > SUM([i.capacity for i in inputs])
    OutputsSumOverflow,

    /// inputs.is_empty() || outputs.is_empty()
    Empty,

    /// Duplicated dep-out-points within the same one transaction
    DuplicateDeps,

    /// outputs.len() != outputs_data.len()
    OutputsDataLengthMismatch,

    /// ANY([o.data_hash != d.data_hash() for (o, d) in ZIP(outputs, outputs_data)])
    OutputDataHashMismatch,

    /// The format of `transaction.since` is invalid
    InvalidSince,

    /// The transaction is not mature which is required by `transaction.since`
    Immature,

    /// The transaction is not mature which is required by cellbase maturity rule
    CellbaseImmaturity,

    /// The transaction version is mismatched with the system can hold
    MismatchedVersion,

    /// The transaction size is too large
    ExceededMaximumBlockBytes,
}

#[derive(Debug, PartialEq, Eq, Clone, Display)]
pub enum HeaderErrorKind {
    InvalidParent,
    Pow,
    Timestamp,
    Number,
    Epoch,
    Version,
}

#[derive(Debug)]
pub struct HeaderError {
    kind: Context<HeaderErrorKind>,
}

#[derive(Debug)]
pub struct BlockError {
    kind: Context<BlockErrorKind>,
}

#[derive(Debug, PartialEq, Eq, Clone, Display)]
pub enum BlockErrorKind {
    ProposalTransactionDuplicate,

    /// There are duplicate committed transactions.
    CommitTransactionDuplicate,

    /// The merkle tree hash of proposed transactions does not match the one in header.
    ProposalTransactionsHash,

    /// The merkle tree hash of committed transactions does not match the one in header.
    TransactionsRoot,

    /// Invalid data in DAO header field is invalid
    InvalidDAO,

    /// Committed transactions verification error. It contains error for the first transaction that
    /// fails the verification. The errors are stored as a tuple, where the first item is the
    /// transaction index in the block and the second item is the transaction verification error.
    BlockTransactions,

    UnknownParent,

    Uncles,

    Cellbase,

    /// This error is returned when the committed transactions does not meet the 2-phases
    /// propose-then-commit consensus rule.
    Commit,

    ExceededMaximumProposalsLimit,

    ExceededMaximumCycles,

    ExceededMaximumBlockBytes,
}

#[derive(Fail, Debug)]
#[fail(display = "BlockTransactionsError(index: {}, error: {})", index, error)]
pub struct BlockTransactionsError {
    pub index: u32,
    pub error: Error,
}

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
#[fail(display = "UnknownParentError(parent_hash: {})", parent_hash)]
pub struct UnknownParentError {
    pub parent_hash: Byte32,
}

#[derive(Fail, Debug, PartialEq, Eq, Clone, Display)]
pub enum CommitError {
    AncestorNotFound,
    Invalid,
}

#[derive(Fail, Debug, PartialEq, Eq, Clone, Display)]
pub enum CellbaseError {
    InvalidInput,
    InvalidRewardAmount,
    InvalidRewardTarget,
    InvalidWitness,
    InvalidTypeScript,
    InvalidOutputQuantity,
    InvalidQuantity,
    InvalidPosition,
    InvalidOutputData,
}

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum UnclesError {
    #[fail(display = "OverCount(max: {}, actual: {})", max, actual)]
    OverCount { max: u32, actual: u32 },

    #[fail(
        display = "InvalidDepth(min: {}, max: {}, actual: {})",
        min, max, actual
    )]
    InvalidDepth { max: u64, min: u64, actual: u64 },

    #[fail(display = "InvalidHash(expected: {}, actual: {})", expected, actual)]
    InvalidHash { expected: Byte32, actual: Byte32 },

    #[fail(display = "InvalidNumber")]
    InvalidNumber,

    #[fail(display = "InvalidTarget")]
    InvalidTarget,

    #[fail(display = "InvalidDifficultyEpoch")]
    InvalidDifficultyEpoch,

    #[fail(display = "ProposalsHash")]
    ProposalsHash,

    #[fail(display = "ProposalDuplicate")]
    ProposalDuplicate,

    #[fail(display = "Duplicate({})", _0)]
    Duplicate(Byte32),

    #[fail(display = "DoubleInclusion({})", _0)]
    DoubleInclusion(Byte32),

    #[fail(display = "DescendantLimit")]
    DescendantLimit,

    #[fail(display = "ExceededMaximumProposalsLimit")]
    ExceededMaximumProposalsLimit,
}

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
#[fail(display = "InvalidParentError(parent_hash: {})gg '", parent_hash)]
pub struct InvalidParentError {
    pub parent_hash: Byte32,
}

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum PowError {
    #[fail(display = "Boundary(expected: {}, actual: {})", expected, actual)]
    Boundary { expected: Byte32, actual: Byte32 },

    #[fail(display = "InvalidNonce")]
    InvalidNonce,
}

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum TimestampError {
    #[fail(display = "BlockTimeTooOld(min: {}, actual: {})", min, actual)]
    BlockTimeTooOld { min: u64, actual: u64 },

    #[fail(display = "BlockTimeTooNew(max: {}, actual: {})", max, actual)]
    BlockTimeTooNew { max: u64, actual: u64 },
}

impl TimestampError {
    pub fn is_too_new(&self) -> bool {
        match self {
            Self::BlockTimeTooOld { .. } => false,
            Self::BlockTimeTooNew { .. } => true,
        }
    }
}

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
#[fail(display = "NumberError(expected: {}, actual: {})", expected, actual)]
pub struct NumberError {
    pub expected: u64,
    pub actual: u64,
}

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum EpochError {
    #[fail(
        display = "TargetMismatch(expected: {:x}, actual: {:x})",
        expected, actual
    )]
    TargetMismatch { expected: u32, actual: u32 },

    #[fail(display = "NumberMismatch(expected: {}, actual: {})", expected, actual)]
    NumberMismatch { expected: u64, actual: u64 },
}

impl TransactionError {
    pub fn is_malformed_tx(&self) -> bool {
        match self {
            TransactionError::OutputsSumOverflow
            | TransactionError::DuplicateDeps
            | TransactionError::Empty
            | TransactionError::InsufficientCellCapacity
            | TransactionError::InvalidSince
            | TransactionError::ExceededMaximumBlockBytes
            | TransactionError::OutputsDataLengthMismatch
            | TransactionError::OutputDataHashMismatch => true,

            TransactionError::Immature
            | TransactionError::CellbaseImmaturity
            | TransactionError::MismatchedVersion => false,
        }
    }
}

impl fmt::Display for HeaderError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(cause) = self.cause() {
            write!(f, "{}({})", self.kind(), cause)
        } else {
            write!(f, "{}", self.kind())
        }
    }
}

impl fmt::Display for BlockError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(cause) = self.cause() {
            write!(f, "{}({})", self.kind(), cause)
        } else {
            write!(f, "{}", self.kind())
        }
    }
}

impl From<Context<HeaderErrorKind>> for HeaderError {
    fn from(kind: Context<HeaderErrorKind>) -> Self {
        Self { kind }
    }
}

impl Fail for HeaderError {
    fn cause(&self) -> Option<&dyn Fail> {
        self.inner().cause()
    }

    fn backtrace(&self) -> Option<&Backtrace> {
        self.inner().backtrace()
    }
}

impl HeaderError {
    pub fn kind(&self) -> &HeaderErrorKind {
        self.kind.get_context()
    }

    pub fn downcast_ref<T: Fail>(&self) -> Option<&T> {
        self.cause().and_then(|cause| cause.downcast_ref::<T>())
    }

    pub fn inner(&self) -> &Context<HeaderErrorKind> {
        &self.kind
    }

    // Note: if the header is invalid, that may also be grounds for disconnecting the peer,
    // However, there is a circumstance where that does not hold:
    // if the header's timestamp is more than ALLOWED_FUTURE_BLOCKTIME ahead of our current time.
    // In that case, the header may become valid in the future,
    // and we don't want to disconnect a peer merely for serving us one too-far-ahead block header,
    // to prevent an attacker from splitting the network by mining a block right at the ALLOWED_FUTURE_BLOCKTIME boundary.
    pub fn is_too_new(&self) -> bool {
        self.downcast_ref::<TimestampError>()
            .map(|e| e.is_too_new())
            .unwrap_or(false)
    }
}

impl From<Context<BlockErrorKind>> for BlockError {
    fn from(kind: Context<BlockErrorKind>) -> Self {
        Self { kind }
    }
}

impl Fail for BlockError {
    fn cause(&self) -> Option<&dyn Fail> {
        self.inner().cause()
    }

    fn backtrace(&self) -> Option<&Backtrace> {
        self.inner().backtrace()
    }
}

impl BlockError {
    pub fn kind(&self) -> &BlockErrorKind {
        self.kind.get_context()
    }

    pub fn downcast_ref<T: Fail>(&self) -> Option<&T> {
        self.cause().and_then(|cause| cause.downcast_ref::<T>())
    }

    pub fn inner(&self) -> &Context<BlockErrorKind> {
        &self.kind
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_too_new() {
        let too_old = TimestampError::BlockTimeTooOld { min: 0, actual: 0 };
        let too_new = TimestampError::BlockTimeTooNew { max: 0, actual: 0 };

        let errors: Vec<HeaderError> = vec![
            HeaderErrorKind::InvalidParent.into(),
            HeaderErrorKind::Pow.into(),
            HeaderErrorKind::Version.into(),
            HeaderErrorKind::Epoch.into(),
            HeaderErrorKind::Version.into(),
            HeaderErrorKind::Timestamp.into(),
            too_old.into(),
            too_new.into(),
        ];

        let is_too_new: Vec<bool> = errors.iter().map(|e| e.is_too_new()).collect();
        assert_eq!(
            is_too_new,
            vec![false, false, false, false, false, false, false, true]
        );
    }
}
