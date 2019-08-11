use ckb_types::{core::BlockNumber, packed::Byte32, U256};
use ckb_error::Error;
use failure::{Backtrace, Context, Fail};
use std::fmt::{self, Display};

#[derive(Fail, Debug, PartialEq, Eq, Clone, Display)]
pub enum TransactionError {
    /// output.occupied_capacity() > output.capacity()
    // NOTE: the original name is InsufficientCellCapacity
    OccupiedOverflowCapacity,

    /// SUM([o.capacity for o in outputs]) > SUM([i.capacity for i in inputs])
    // NOTE: the original name is OutputsSumOverflow
    OutputOverflowCapacity,

    /// inputs.is_empty() || outputs.is_empty()
    // NOTE: the original name is Empty
    MissingInputsOrOutputs,

    /// Duplicated dep-out-points within the same one transaction
    // NOTE: the original name is DuplicateDeps
    DuplicatedDeps,

    /// outputs.len() != outputs_data.len()
    // NOTE: the original name is OutputsDataLengthMismatch
    UnmatchedOutputsDataLength,

    /// ANY([o.data_hash != d.data_hash() for (o, d) in ZIP(outputs, outputs_data)])
    // NOTE: the original name is OutputDataHashMismatch
    UnmatchedOutputsDataHashes,

    /// The format of `transaction.since` is invalid
    // NOTE: the original name is InvalidSince
    InvalidSinceFormat,

    /// The transaction is not mature which is required by `transaction.since`
    // NOTE: the original name is Immature
    ImmatureTransaction,

    /// The transaction is not mature which is required by cellbase maturity rule
    // NOTE: the original name is CellbaseImmaturity
    ImmatureCellbase,

    /// The transaction version is mismatched with the system can hold
    MismatchedVersion,
use std::fmt::{self, Display};
use failure::{Backtrace, Context, Fail};

    /// The transaction size is too large
    // NOTE: the original name is ExceededMaximumBlockBytes
    TooLargeSize,
}

#[derive(Fail, Debug, PartialEq, Eq, Clone, Display)]
pub enum HeaderErrorKind {
    /// The parent of this header was marked as invalid
    InvalidParent,

    /// The field pow in block header is invalid
    Pow,

    /// The field timestamp in block header is invalid.
    Timestamp,

    /// The field number in block header is invalid.
    Number,

    /// The field difficulty in block header is invalid.
    Epoch,
}

#[derive(Debug)]
pub struct HeaderError {
    kind: Context<HeaderErrorKind>,
}

#[derive(Debug)]
pub struct BlockError {
    kind: Context<BlockErrorKind>,
}

#[derive(Fail, Debug, PartialEq, Eq, Clone, Display)]
pub enum BlockErrorKind {
    /// There are duplicate proposed transactions.
    // NOTE: the original name is ProposalTransactionDuplicate
    DuplicatedProposalTransactions,

    /// There are duplicate committed transactions.
    // NOTE: the original name is CommitTransactionDuplicate
    DuplicatedCommittedTransactions,

    /// The merkle tree hash of proposed transactions does not match the one in header.
    // NOTE: the original name is ProposalTransactionsRoot
    UnmatchedProposalRoot,

    /// The merkle tree hash of committed transactions does not match the one in header.
    // NOTE: the original name is CommitTransactionsRoot
    UnmatchedCommittedRoot,

    /// The merkle tree witness hash of committed transactions does not match the one in header.
    // NOTE: the original name is WitnessesMerkleRoot
    UnmatchedWitnessesRoot,

    /// Invalid data in DAO header field is invalid
    InvalidDAO,

    /// Committed transactions verification error. It contains error for the first transaction that
    /// fails the verification. The errors are stored as a tuple, where the first item is the
    /// transaction index in the block and the second item is the transaction verification error.
    BlockTransactions,

    /// The parent of the block is unknown.
    UnknownParent(Byte32),

/// Uncles does not meet the consensus requirements.
    Uncles,

    /// Cellbase transaction is invalid.
    Cellbase,

    /// This error is returned when the committed transactions does not meet the 2-phases
    /// propose-then-commit consensus rule.
    Commit,

    /// Number of proposals exceeded the limit.
    // NOTE: the original name is ExceededMaximumProposalsLimit
    TooManyProposals,

    /// Cycles consumed by all scripts in all commit transactions of the block exceed
    /// the maximum allowed cycles in consensus rules
    // NOTE: the original name is ExceededMaximumCycles
    TooMuchCycles,

    /// The size of the block exceeded the limit.
    // NOTE: the original name is ExceededMaximumBlockBytes
    TooLargeSize,

    /// The field version in block header is not allowed.
    // NOTE: the original name is Version
    MismatchedVersion,
}

#[derive(Fail, Debug)]
#[fail(display = "index: {}, error: {}", index, error)]
pub struct BlockTransactionsError {
    pub index: u32,
    pub error: Error,
}

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
#[fail(display = "parent_hash: {:#x}", parent_hash)]
pub struct UnknownParentError {
    pub parent_hash: H256,
}

#[derive(Fail, Debug, PartialEq, Eq, Clone, Display)]
pub enum CommitError {
    /// Ancestor not found, should not happen, we check header first and check ancestor.
    // NOTE: the original name is AncestorNotFound
    NonexistentAncestor,

    /// Break propose-then-commit consensus rule.
    // NOTE: the original name is Invalid
    NotInProposalWindow,
}

#[derive(Fail, Debug, PartialEq, Eq, Clone, Display)]
pub enum CellbaseError {
    InvalidInput,
    InvalidRewardAmount,
    InvalidRewardTarget,
    InvalidWitness,
    InvalidTypeScript,
    InvalidQuantity,
    InvalidPosition,
    InvalidOutputData,
}

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum UnclesError {
    // NOTE: the original name is OverCount
    #[fail(display = "TooManyUncles{{max: {}, actual: {}}}", max, actual)]
    TooManyUncles { max: u32, actual: u32 },

    // NOTE: the original name is MissMatchCount
    #[fail(
        display = "UnmatchedCount{{expected: {}, actual: {}}}",
        expected, actual
    )]
    UnmatchedCount { expected: u32, actual: u32 },

    #[fail(
        display = "InvalidDepth{{min: {}, max: {}, actual: {}}}",
        min, max, actual
    )]
    InvalidDepth { max: u64, min: u64, actual: u64 },

    // NOTE: the original name is InvalidHash
    #[fail(
        display = "UnmatchedUnclesHash{{expected: {:#x}, actual: {:#x}}}",
        expected, actual
    )]
    UnmatchedUnclesHash { expected: H256, actual: H256 },

    // NOTE: the original name is InvalidNumber
    #[fail(display = "UnmatchedBlockNumber")]
    UnmatchedBlockNumber,

    #[fail(display = "UnmatchedDifficulty")]
    UnmatchedDifficulty,

    // NOTE: the original name is InvalidDifficultyEpoch
    #[fail(display = "UnmatchedEpochNumber")]
    UnmatchedEpochNumber,

    // NOTE: the original name is ProposalsHash
    #[fail(display = "UnmatchedProposalRoot")]
    UnmatchedProposalRoot,

    // NOTE: the original name is ProposalDuplicate
    #[fail(display = "DuplicatedProposalTransactions")]
    DuplicatedProposalTransactions,

    // NOTE: the original name is Duplicate
    #[fail(display = "DuplicatedUncles({:#x})", _0)]
    DuplicatedUncles(H256),

    #[fail(display = "DoubleInclusion({:#x})", _0)]
    DoubleInclusion(H256),

    #[fail(display = "DescendantLimit")]
    DescendantLimit,

    // NOTE: the original name is ExceededMaximumProposalsLimit
    #[fail(display = "TooManyProposals")]
    TooManyProposals,
}

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
#[fail(display = "parent_hash: {:#x}", parent_hash)]
pub struct InvalidParentError {
    pub parent_hash: H256,
}

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum PowError {
    #[fail(
        display = "Boundary{{expected: {:#x}, actual: {:#x}}}",
        expected, actual
    )]
    Boundary { expected: U256, actual: U256 },

    #[fail(display = "InvalidProof")]
    InvalidProof,
}

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum TimestampError {
    #[fail(display = "BlockTimeTooOld{{min: {}, actual: {}}}", min, actual)]
    BlockTimeTooOld { min: u64, actual: u64 },

    #[fail(display = "BlockTimeTooNew{{max: {}, actual: {}}}", max, actual)]
    BlockTimeTooNew { max: u64, actual: u64 },
}

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
#[fail(display = "expected: {}, actual: {}", expected, actual)]
pub struct NumberError {
    pub expected: u64,
    pub actual: u64,
}

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum EpochError {
    // NOTE: the original name is DifficultyMismatch
    #[fail(
        display = "UnmatchedDifficulty{{expected: {:#x}, actual: {:#x}}}",
        expected, actual
    )]
    UnmatchedDifficulty { expected: U256, actual: U256 },

    // NOTE: the original name is NumberMismatch
    #[fail(
        display = "UnmatchedNumber{{expected: {}, actual: {}}}",
        expected, actual
    )]
    UnmatchedNumber { expected: u64, actual: u64 },

    // NOTE: the original name is AncestorNotFound
    #[fail(display = "MissingAncestor")]
    MissingAncestor,
}

impl TransactionError {
    pub fn is_bad_tx(&self) -> bool {
        match self {
            TransactionError::OutputOverflowCapacity
            | TransactionError::DuplicatedDeps
            | TransactionError::MissingInputsOrOutputs
            | TransactionError::OccupiedOverflowCapacity
            | TransactionError::InvalidSinceFormat => true,
            _ => false,
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
    fn cause(&self) -> Option<&Fail> {
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
}

impl From<Context<BlockErrorKind>> for BlockError {
    fn from(kind: Context<BlockErrorKind>) -> Self {
        Self { kind }
    }
}

impl Fail for BlockError {
    fn cause(&self) -> Option<&Fail> {
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
