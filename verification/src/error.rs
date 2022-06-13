use ckb_error::{def_error_base_on_kind, prelude::*, Error};
use ckb_types::{core::Version, packed::Byte32};
use derive_more::Display;

pub use ckb_types::core::{
    error::{TransactionError, TransactionErrorSource},
    EpochNumberWithFraction,
};

/// A list specifying categories of ckb header error.
///
/// This list is intended to grow over time and it is not recommended to exhaustively match against it.
///
/// It is used with the [`HeaderError`].
///
/// [`HeaderError`]: ../ckb_verification/struct.HeaderError.html
#[derive(Debug, PartialEq, Eq, Clone, Copy, Display)]
pub enum HeaderErrorKind {
    /// It indicates that the underlying error is [`InvalidParentError`].
    ///
    /// [`InvalidParentError`]: ../ckb_verification/struct.InvalidParentError.html
    InvalidParent,
    /// It indicates that the underlying error is [`PowError`].
    ///
    /// [`PowError`]: ../ckb_verification/enum.PowError.html
    Pow,
    /// It indicates that the underlying error is [`TimestampError`].
    ///
    /// [`TimestampError`]: ../ckb_verification/enum.TimestampError.html
    Timestamp,
    /// It indicates that the underlying error is [`NumberError`].
    ///
    /// [`NumberError`]: ../ckb_verification/struct.NumberError.html
    Number,
    /// It indicates that the underlying error is [`EpochError`].
    ///
    /// [`EpochError`]: ../ckb_verification/enum.EpochError.html
    Epoch,
    /// It indicates that the underlying error is [`BlockVersionError`].
    ///
    /// [`BlockVersionError`]: ../ckb_verification/struct.BlockVersionError.html
    Version,
}

def_error_base_on_kind!(
    HeaderError,
    HeaderErrorKind,
    "Errors due the fact that the header rule is not respected."
);

/// A list specifying categories of ckb block error.
///
/// This list is intended to grow over time and it is not recommended to exhaustively match against it.
///
/// It is used with the [`BlockError`].
///
/// [`BlockError`]: ../ckb_verification/struct.BlockError.html
#[derive(Debug, PartialEq, Eq, Clone, Copy, Display)]
pub enum BlockErrorKind {
    /// There are duplicated proposal transactions.
    ProposalTransactionDuplicate,

    /// There are duplicate committed transactions.
    CommitTransactionDuplicate,

    /// The calculated Merkle tree hash of proposed transactions does not match the one in the header.
    ProposalTransactionsHash,

    /// The calculated Merkle tree hash of committed transactions does not match the one in the header.
    TransactionsRoot,

    /// The calculated dao field does not match with the one in the header.
    InvalidDAO,

    /// It indicates that the underlying error is [`BlockTransactionsError`].
    ///
    /// [`BlockTransactionsError`]: ../ckb_verification/struct.BlockTransactionsError.html
    BlockTransactions,

    /// It indicates that the underlying error is [`UnknownParentError`].
    ///
    /// [`UnknownParentError`]: ../ckb_verification/struct.UnknownParentError.html
    UnknownParent,

    /// It indicates that the underlying error is [`UnclesError`].
    ///
    /// [`UnclesError`]: ../ckb_verification/enum.UnclesError.html
    Uncles,

    /// It indicates that the underlying error is [`CellbaseError`].
    ///
    /// [`CellbaseError`]: ../ckb_verification/enum.CellbaseError.html
    Cellbase,

    /// It indicates that the underlying error is [`CommitError`].
    ///
    /// [`CommitError`]: ../ckb_verification/struct.CommitError.html
    Commit,

    /// The number of block proposals exceeds limit.
    ExceededMaximumProposalsLimit,

    /// Total cycles of the block transactions exceed limit.
    ExceededMaximumCycles,

    /// Total bytes of block exceeds limit.
    ExceededMaximumBlockBytes,

    /// Empty block extension.
    EmptyBlockExtension,

    /// Total bytes of block extension exceeds limit.
    ExceededMaximumBlockExtensionBytes,

    /// No block extension.
    ///
    /// The block extension should be existed after light client supported.
    NoBlockExtension,

    /// The data length of block extension mismatches.
    InvalidBlockExtension,

    /// The block has unknown field.
    UnknownFields,

    /// The calculated extra-hash does not match with the one in the header.
    InvalidExtraHash,

    /// The calculated hash of chain root does not match with the one in the header.
    InvalidChainRoot,
}

def_error_base_on_kind!(
    BlockError,
    BlockErrorKind,
    "Errors due the fact that the block rule is not respected."
);

/// Errors occur during block transactions verification.
#[derive(Error, Debug)]
#[error("BlockTransactionsError(index: {index}, error: {error})")]
pub struct BlockTransactionsError {
    /// The index of the first erroneous transaction.
    pub index: u32,
    /// The underlying error to that erroneous transaction.
    pub error: Error,
}

/// Cannot access the parent block to the cannonical chain.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
#[error("UnknownParentError(parent_hash: {parent_hash})")]
pub struct UnknownParentError {
    /// The hash of parent block.
    pub parent_hash: Byte32,
}

/// Errors due to the fact that the 2pc rule is not respected.
///
/// See also [Two-Step Transaction Confirmation](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0020-ckb-consensus-protocol/0020-ckb-consensus-protocol.md#two-step-transaction-confirmation)
#[derive(Error, Debug, PartialEq, Eq, Clone, Display)]
pub enum CommitError {
    /// There are blocks required at 2pc verification but not found.
    AncestorNotFound,
    /// There are block transactions that have not been proposed in the proposal window.
    Invalid,
}

/// Errors due to the fact that the cellbase rule is not respected.
///
/// See more about cellbase transaction: [cellbase transaction]
///
/// [cellbase transaction]: https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0022-transaction-structure/0022-transaction-structure.md#exceptions
#[derive(Error, Debug, PartialEq, Eq, Clone, Display)]
pub enum CellbaseError {
    /// The cellbase input is unexpected. The structure reference of correct cellbase input: [`new_cellbase_input`].
    ///
    /// [`new_cellbase_input`]: https://github.com/nervosnetwork/ckb/blob/ee0ccecd87013821a2e68120ba3510393c0373e7/util/types/src/extension/shortcuts.rs#L107-L109
    InvalidInput,
    /// The cellbase output capacity is not equal to the total block reward.
    InvalidRewardAmount,
    /// The cellbase output lock does not match with the target lock.
    ///
    /// As for 0 ~ PROPOSAL_WINDOW.farthest blocks, cellbase outputs should be empty; otherwise, lock of first cellbase output should match with the target block.
    ///
    /// Assumes the current block number is `i`, then its target block is that: (1) on that same chain with current block; (2) number is `i - PROPOSAL_WINDOW.farthest - 1`.
    InvalidRewardTarget,
    /// The cellbase witness is not in [`CellbaseWitness`] format.
    ///
    /// [`CellbaseWitness`]: ../ckb_types/packed/struct.CellbaseWitness.html
    InvalidWitness,
    /// The cellbase type script is not none.
    InvalidTypeScript,
    /// The length of cellbase outputs and outputs-data should be equal and less than `1`.
    InvalidOutputQuantity,
    /// There are multiple cellbase transactions inside the same block.
    InvalidQuantity,
    /// The first block transaction is not a valid cellbase transaction.
    ///
    /// See also [`is_cellbase`].
    ///
    /// [`is_cellbase`]: https://github.com/nervosnetwork/ckb/blob/ee0ccecd87013821a2e68120ba3510393c0373e7/util/types/src/core/views.rs#L387-L389
    InvalidPosition,
    /// The cellbase output-data is not empty.
    InvalidOutputData,
}

/// Errors due to the fact that the uncle rule is not respected.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum UnclesError {
    /// The number of block uncles exceeds limit.
    #[error("OverCount(max: {max}, actual: {actual})")]
    OverCount {
        /// The limited number of block uncles.
        max: u32,
        /// The actual number of block uncles.
        actual: u32,
    },

    /// There is an uncle whose number is greater than or equal to current block number.
    #[error("InvalidNumber")]
    InvalidNumber,

    /// There is an uncle who belongs to a different epoch from the current block.
    #[error("InvalidTarget")]
    InvalidTarget,

    /// There is an uncle who belongs to a different epoch from the current block.
    #[error("InvalidDifficultyEpoch")]
    InvalidDifficultyEpoch,

    /// There is an uncle whose proposals-hash does not match with the calculated result.
    #[error("ProposalsHash")]
    ProposalsHash,

    /// There is an uncle whose proposals have duplicated items.
    #[error("ProposalDuplicate")]
    ProposalDuplicate,

    /// There are duplicated uncles in the current block.
    #[error("Duplicate({0})")]
    Duplicate(Byte32),

    /// There is an uncle that has already been included before.
    #[error("DoubleInclusion({0})")]
    DoubleInclusion(Byte32),

    /// The depth of uncle descendant exceeds limit.
    #[error("DescendantLimit")]
    DescendantLimit,

    /// The number of uncle proposals exceeds limit.
    #[error("ExceededMaximumProposalsLimit")]
    ExceededMaximumProposalsLimit,
}

/// The block version is unexpected.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
#[error("BlockVersionError(expected: {expected}, actual: {actual})")]
pub struct BlockVersionError {
    /// The expected block version.
    pub expected: Version,
    /// The actual block version.
    pub actual: Version,
}

/// The block's parent is marked as invalid.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
#[error("InvalidParentError(parent_hash: {parent_hash})")]
pub struct InvalidParentError {
    /// The parent block hash.
    pub parent_hash: Byte32,
}

/// Errors due to the fact that the pow rule is not respected.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum PowError {
    /// Error occurs during PoW verification.
    #[error("InvalidNonce: please set logger.filter to \"info,ckb-pow=debug\" to see detailed PoW verification information in the log")]
    InvalidNonce,
}

/// Errors due to the fact that the block timestamp rule is not respected.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum TimestampError {
    /// The block timestamp is older than the allowed oldest timestamp.
    #[error("BlockTimeTooOld(min: {min}, actual: {actual})")]
    BlockTimeTooOld {
        /// The allowed oldest block timestamp.
        min: u64,
        /// The actual block timestamp.
        actual: u64,
    },

    /// The block timestamp is newer than the allowed newest timestamp.
    #[error("BlockTimeTooNew(max: {max}, actual: {actual})")]
    BlockTimeTooNew {
        /// The allowed newest block timestamp.
        max: u64,
        /// The actual block timestamp.
        actual: u64,
    },
}

impl TimestampError {
    /// Return `true` if this error is `TimestampError::BlockTimeTooNew`.
    #[doc(hidden)]
    pub fn is_too_new(&self) -> bool {
        match self {
            Self::BlockTimeTooOld { .. } => false,
            Self::BlockTimeTooNew { .. } => true,
        }
    }
}

/// The block number is not equal to parent number + `1`.
/// Specially genesis block number is `0`.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
#[error("NumberError(expected: {expected}, actual: {actual})")]
pub struct NumberError {
    /// The expected block number.
    pub expected: u64,
    /// The actual block number.
    pub actual: u64,
}

/// Errors due to the fact that the block epoch is not expected.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum EpochError {
    /// The format of header epoch is malformed.
    #[error("Malformed(value: {value:#})")]
    Malformed {
        /// The malformed header epoch.
        value: EpochNumberWithFraction,
    },

    /// The header epoch is not continuous.
    #[error("NonContinuous(current: {current:#}, parent: {parent:#})")]
    NonContinuous {
        /// The current header epoch.
        current: EpochNumberWithFraction,
        /// The parent header epoch.
        parent: EpochNumberWithFraction,
    },

    /// The compact-target of block epoch is unexpected.
    #[error("TargetMismatch(expected: {expected:x}, actual: {actual:x})")]
    TargetMismatch {
        /// The expected compact-target of block epoch.
        expected: u32,
        /// The actual compact-target of block epoch.
        actual: u32,
    },

    /// The number of block epoch is unexpected.
    #[error("NumberMismatch(expected: {expected}, actual: {actual})")]
    NumberMismatch {
        /// The expected number of block epoch.
        expected: u64,
        /// The actual number of block epoch.
        actual: u64,
    },
}

impl HeaderError {
    /// Downcast `HeaderError` to `TimestampError` then check [`TimestampError::is_too_new`].
    ///
    /// Note: if the header is invalid, that may also be grounds for disconnecting the peer,
    /// However, there is a circumstance where that does not hold:
    /// if the header's timestamp is more than ALLOWED_FUTURE_BLOCKTIME ahead of our current time.
    /// In that case, the header may become valid in the future,
    /// and we don't want to disconnect a peer merely for serving us one too-far-ahead block header,
    /// to prevent an attacker from splitting the network by mining a block right at the ALLOWED_FUTURE_BLOCKTIME boundary.
    ///
    /// [`TimestampError::is_too_new`]
    #[doc(hidden)]
    pub fn is_too_new(&self) -> bool {
        self.downcast_ref::<TimestampError>()
            .map(|e| e.is_too_new())
            .unwrap_or(false)
    }
}
