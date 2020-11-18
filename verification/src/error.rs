use ckb_error::Error;
use ckb_types::{
    core::{Capacity, Version},
    packed::{Byte32, OutPoint},
};
use failure::{Backtrace, Context, Fail};
use std::fmt::{self, Display};

#[derive(Clone, Debug, Display, Eq, PartialEq)]
pub enum TransactionErrorSource {
    CellDeps,
    HeaderDeps,
    Inputs,
    Outputs,
    OutputsData,
    Witnesses,
}

/// TODO(doc): @keroro520
#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum TransactionError {
    /// output.occupied_capacity() > output.capacity()
    #[fail(
        display = "InsufficientCellCapacity({}[{}]): expected occupied capacity ({:#x}) <= capacity ({:#x})",
        source, index, occupied_capacity, capacity
    )]
    InsufficientCellCapacity {
        /// TODO(doc): @keroro520
        source: TransactionErrorSource,
        /// TODO(doc): @keroro520
        index: usize,
        /// TODO(doc): @keroro520
        occupied_capacity: Capacity,
        /// TODO(doc): @keroro520
        capacity: Capacity,
    },

    /// SUM([o.capacity for o in outputs]) > SUM([i.capacity for i in inputs])
    #[fail(
        display = "OutputsSumOverflow: expected outputs capacity ({:#x}) <= inputs capacity ({:#x})",
        outputs_sum, inputs_sum
    )]
    OutputsSumOverflow {
        /// TODO(doc): @keroro520
        inputs_sum: Capacity,
        /// TODO(doc): @keroro520
        outputs_sum: Capacity,
    },

    /// inputs.is_empty() || outputs.is_empty()
    #[fail(display = "Empty({})", source)]
    Empty {
        /// TODO(doc): @keroro520
        source: TransactionErrorSource,
    },

    /// Duplicated dep-out-points within the same transaction
    #[fail(display = "DuplicateCellDeps({})", out_point)]
    DuplicateCellDeps {
        /// TODO(doc): @keroro520
        out_point: OutPoint,
    },

    /// Duplicated headers deps without within the same transaction
    #[fail(display = "DuplicateHeaderDeps({})", hash)]
    DuplicateHeaderDeps {
        /// TODO(doc): @keroro520
        hash: Byte32,
    },

    /// outputs.len() != outputs_data.len()
    #[fail(
        display = "OutputsDataLengthMismatch: expected outputs data length ({}) = outputs length ({})",
        outputs_data_len, outputs_len
    )]
    OutputsDataLengthMismatch {
        /// TODO(doc): @keroro520
        outputs_len: usize,
        /// TODO(doc): @keroro520
        outputs_data_len: usize,
    },

    /// The format of `transaction.since` is invalid
    #[fail(
        display = "InvalidSince(Inputs[{}]): the field since is invalid",
        index
    )]
    InvalidSince {
        /// TODO(doc): @keroro520
        index: usize,
    },

    /// The transaction is not mature which is required by `transaction.since`
    #[fail(
        display = "Immature(Inputs[{}]): the transaction is immature because of the since requirement",
        index
    )]
    Immature {
        /// TODO(doc): @keroro520
        index: usize,
    },

    /// The transaction is not mature which is required by cellbase maturity rule
    #[fail(display = "CellbaseImmaturity({}[{}])", source, index)]
    CellbaseImmaturity {
        /// TODO(doc): @keroro520
        source: TransactionErrorSource,
        /// TODO(doc): @keroro520
        index: usize,
    },

    /// The transaction version is mismatched with the system can hold
    #[fail(display = "MismatchedVersion: expected {}, got {}", expected, actual)]
    MismatchedVersion {
        /// TODO(doc): @keroro520
        expected: Version,
        /// TODO(doc): @keroro520
        actual: Version,
    },

    /// The transaction size is too large
    #[fail(
        display = "ExceededMaximumBlockBytes: expected transaction serialized size ({}) < block size limit ({})",
        actual, limit
    )]
    ExceededMaximumBlockBytes {
        /// TODO(doc): @keroro520
        limit: u64,
        /// TODO(doc): @keroro520
        actual: u64,
    },
}

/// TODO(doc): @keroro520
#[derive(Debug, PartialEq, Eq, Clone, Display)]
pub enum HeaderErrorKind {
    /// TODO(doc): @keroro520
    InvalidParent,
    /// TODO(doc): @keroro520
    Pow,
    /// TODO(doc): @keroro520
    Timestamp,
    /// TODO(doc): @keroro520
    Number,
    /// TODO(doc): @keroro520
    Epoch,
    /// TODO(doc): @keroro520
    Version,
}

/// TODO(doc): @keroro520
#[derive(Debug)]
pub struct HeaderError {
    kind: Context<HeaderErrorKind>,
}

/// TODO(doc): @keroro520
#[derive(Debug)]
pub struct BlockError {
    kind: Context<BlockErrorKind>,
}

/// TODO(doc): @keroro520
#[derive(Debug, PartialEq, Eq, Clone, Display)]
pub enum BlockErrorKind {
    /// TODO(doc): @keroro520
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

    /// TODO(doc): @keroro520
    UnknownParent,

    /// TODO(doc): @keroro520
    Uncles,

    /// TODO(doc): @keroro520
    Cellbase,

    /// This error is returned when the committed transactions does not meet the 2-phases
    /// propose-then-commit consensus rule.
    Commit,

    /// TODO(doc): @keroro520
    ExceededMaximumProposalsLimit,

    /// TODO(doc): @keroro520
    ExceededMaximumCycles,

    /// TODO(doc): @keroro520
    ExceededMaximumBlockBytes,
}

/// TODO(doc): @keroro520
#[derive(Fail, Debug)]
#[fail(display = "BlockTransactionsError(index: {}, error: {})", index, error)]
pub struct BlockTransactionsError {
    /// TODO(doc): @keroro520
    pub index: u32,
    /// TODO(doc): @keroro520
    pub error: Error,
}

/// TODO(doc): @keroro520
#[derive(Fail, Debug, PartialEq, Eq, Clone)]
#[fail(display = "UnknownParentError(parent_hash: {})", parent_hash)]
pub struct UnknownParentError {
    /// TODO(doc): @keroro520
    pub parent_hash: Byte32,
}

/// TODO(doc): @keroro520
#[derive(Fail, Debug, PartialEq, Eq, Clone, Display)]
pub enum CommitError {
    /// TODO(doc): @keroro520
    AncestorNotFound,
    /// TODO(doc): @keroro520
    Invalid,
}

/// TODO(doc): @keroro520
#[derive(Fail, Debug, PartialEq, Eq, Clone, Display)]
pub enum CellbaseError {
    /// TODO(doc): @keroro520
    InvalidInput,
    /// TODO(doc): @keroro520
    InvalidRewardAmount,
    /// TODO(doc): @keroro520
    InvalidRewardTarget,
    /// TODO(doc): @keroro520
    InvalidWitness,
    /// TODO(doc): @keroro520
    InvalidTypeScript,
    /// TODO(doc): @keroro520
    InvalidOutputQuantity,
    /// TODO(doc): @keroro520
    InvalidQuantity,
    /// TODO(doc): @keroro520
    InvalidPosition,
    /// TODO(doc): @keroro520
    InvalidOutputData,
}

/// TODO(doc): @keroro520
#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum UnclesError {
    /// TODO(doc): @keroro520
    #[fail(display = "OverCount(max: {}, actual: {})", max, actual)]
    OverCount {
        /// TODO(doc): @keroro520
        max: u32,
        /// TODO(doc): @keroro520
        actual: u32,
    },

    /// TODO(doc): @keroro520
    #[fail(
        display = "InvalidDepth(min: {}, max: {}, actual: {})",
        min, max, actual
    )]
    InvalidDepth {
        /// TODO(doc): @keroro520
        max: u64,
        /// TODO(doc): @keroro520
        min: u64,
        /// TODO(doc): @keroro520
        actual: u64,
    },

    /// TODO(doc): @keroro520
    #[fail(display = "InvalidHash(expected: {}, actual: {})", expected, actual)]
    InvalidHash {
        /// TODO(doc): @keroro520
        expected: Byte32,
        /// TODO(doc): @keroro520
        actual: Byte32,
    },

    /// TODO(doc): @keroro520
    #[fail(display = "InvalidNumber")]
    InvalidNumber,

    /// TODO(doc): @keroro520
    #[fail(display = "InvalidTarget")]
    InvalidTarget,

    /// TODO(doc): @keroro520
    #[fail(display = "InvalidDifficultyEpoch")]
    InvalidDifficultyEpoch,

    /// TODO(doc): @keroro520
    #[fail(display = "ProposalsHash")]
    ProposalsHash,

    /// TODO(doc): @keroro520
    #[fail(display = "ProposalDuplicate")]
    ProposalDuplicate,

    /// TODO(doc): @keroro520
    #[fail(display = "Duplicate({})", _0)]
    Duplicate(Byte32),

    /// TODO(doc): @keroro520
    #[fail(display = "DoubleInclusion({})", _0)]
    DoubleInclusion(Byte32),

    /// TODO(doc): @keroro520
    #[fail(display = "DescendantLimit")]
    DescendantLimit,

    /// TODO(doc): @keroro520
    #[fail(display = "ExceededMaximumProposalsLimit")]
    ExceededMaximumProposalsLimit,
}

/// TODO(doc): @keroro520
#[derive(Fail, Debug, PartialEq, Eq, Clone)]
#[fail(
    display = "BlockVersionError(expected: {}, actual: {})",
    expected, actual
)]
pub struct BlockVersionError {
    /// TODO(doc): @keroro520
    pub expected: Version,
    /// TODO(doc): @keroro520
    pub actual: Version,
}

/// TODO(doc): @keroro520
#[derive(Fail, Debug, PartialEq, Eq, Clone)]
#[fail(display = "InvalidParentError(parent_hash: {})", parent_hash)]
pub struct InvalidParentError {
    /// TODO(doc): @keroro520
    pub parent_hash: Byte32,
}

/// TODO(doc): @keroro520
#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum PowError {
    /// TODO(doc): @keroro520
    #[fail(display = "Boundary(expected: {}, actual: {})", expected, actual)]
    Boundary {
        /// TODO(doc): @keroro520
        expected: Byte32,
        /// TODO(doc): @keroro520
        actual: Byte32,
    },

    /// TODO(doc): @keroro520
    #[fail(
        display = "InvalidNonce: please set logger.filter to \"info,ckb-pow=debug\" to see detailed PoW verification information in the log"
    )]
    InvalidNonce,
}

/// TODO(doc): @keroro520
#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum TimestampError {
    /// TODO(doc): @keroro520
    #[fail(display = "BlockTimeTooOld(min: {}, actual: {})", min, actual)]
    BlockTimeTooOld {
        /// TODO(doc): @keroro520
        min: u64,
        /// TODO(doc): @keroro520
        actual: u64,
    },

    /// TODO(doc): @keroro520
    #[fail(display = "BlockTimeTooNew(max: {}, actual: {})", max, actual)]
    BlockTimeTooNew {
        /// TODO(doc): @keroro520
        max: u64,
        /// TODO(doc): @keroro520
        actual: u64,
    },
}

impl TimestampError {
    /// TODO(doc): @keroro520
    pub fn is_too_new(&self) -> bool {
        match self {
            Self::BlockTimeTooOld { .. } => false,
            Self::BlockTimeTooNew { .. } => true,
        }
    }
}

/// TODO(doc): @keroro520
#[derive(Fail, Debug, PartialEq, Eq, Clone)]
#[fail(display = "NumberError(expected: {}, actual: {})", expected, actual)]
pub struct NumberError {
    /// TODO(doc): @keroro520
    pub expected: u64,
    /// TODO(doc): @keroro520
    pub actual: u64,
}

/// TODO(doc): @keroro520
#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum EpochError {
    /// TODO(doc): @keroro520
    #[fail(
        display = "TargetMismatch(expected: {:x}, actual: {:x})",
        expected, actual
    )]
    TargetMismatch {
        /// TODO(doc): @keroro520
        expected: u32,
        /// TODO(doc): @keroro520
        actual: u32,
    },

    /// TODO(doc): @keroro520
    #[fail(display = "NumberMismatch(expected: {}, actual: {})", expected, actual)]
    NumberMismatch {
        /// TODO(doc): @keroro520
        expected: u64,
        /// TODO(doc): @keroro520
        actual: u64,
    },
}

impl TransactionError {
    /// TODO(doc): @keroro520
    pub fn is_malformed_tx(&self) -> bool {
        match self {
            TransactionError::OutputsSumOverflow { .. }
            | TransactionError::DuplicateCellDeps { .. }
            | TransactionError::DuplicateHeaderDeps { .. }
            | TransactionError::Empty { .. }
            | TransactionError::InsufficientCellCapacity { .. }
            | TransactionError::InvalidSince { .. }
            | TransactionError::ExceededMaximumBlockBytes { .. }
            | TransactionError::OutputsDataLengthMismatch { .. } => true,

            TransactionError::Immature { .. }
            | TransactionError::CellbaseImmaturity { .. }
            | TransactionError::MismatchedVersion { .. } => false,
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
    /// TODO(doc): @keroro520
    pub fn kind(&self) -> &HeaderErrorKind {
        self.kind.get_context()
    }

    /// TODO(doc): @keroro520
    pub fn downcast_ref<T: Fail>(&self) -> Option<&T> {
        self.cause().and_then(|cause| cause.downcast_ref::<T>())
    }

    /// TODO(doc): @keroro520
    pub fn inner(&self) -> &Context<HeaderErrorKind> {
        &self.kind
    }

    /// TODO(doc): @keroro520
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
    /// TODO(doc): @keroro520
    pub fn kind(&self) -> &BlockErrorKind {
        self.kind.get_context()
    }

    /// TODO(doc): @keroro520
    pub fn downcast_ref<T: Fail>(&self) -> Option<&T> {
        self.cause().and_then(|cause| cause.downcast_ref::<T>())
    }

    /// TODO(doc): @keroro520
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

    #[test]
    fn test_version_error_display() {
        let e: Error = BlockVersionError {
            expected: 0,
            actual: 1,
        }
        .into();

        assert_eq!(
            "Header(Version(BlockVersionError(expected: 0, actual: 1)))",
            format!("{}", e)
        );
    }
}
