use crate::TransactionError;
use failure::Fail;
use numext_fixed_hash::H256;

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum BlockError {
    /// There are duplicate proposed transactions.
    // NOTE: the original name is ProposalTransactionDuplicate
    #[fail(display = "Duplicated proposal transactions")]
    DuplicatedProposalTransactions,

    /// There are duplicate committed transactions.
    // NOTE: the original name is CommitTransactionDuplicate
    #[fail(display = "Duplicated committed transactions")]
    DuplicatedCommittedTransactions,

    /// The merkle tree hash of proposed transactions does not match the one in header.
    // NOTE: the original name is ProposalTransactionsRoot
    #[fail(display = "Unmatched proposal transactions root")]
    UnmatchedProposalRoot,

    /// The merkle tree hash of committed transactions does not match the one in header.
    // NOTE: the original name is CommitTransactionsRoot
    #[fail(display = "Unmatched committed transactions root")]
    UnmatchedCommittedRoot,

    /// The merkle tree witness hash of committed transactions does not match the one in header.
    // NOTE: the original name is WitnessesMerkleRoot
    #[fail(display = "Unmatched witnesses root")]
    UnmatchedWitnessesRoot,

    /// Invalid data in DAO header field is invalid
    #[fail(display = "Invalid DAO")]
    InvalidDAO,

    /// Committed transactions verification error. It contains error for the first transaction that
    /// fails the verification. The errors are stored as a tuple, where the first item is the
    /// transaction index in the block and the second item is the transaction verification error.
    #[fail(display = "Block transactions error, index: {}, error: {}", _0, _1)]
    Transactions(usize, TransactionError),

    /// The parent of the block is unknown.
    #[fail(display = "Unknown parent: {:#x}", _0)]
    UnknownParent(H256),

    /// Uncles does not meet the consensus requirements.
    #[fail(display = "{}", _0)]
    Uncles(#[fail(cause)] UnclesError),

    /// Cellbase transaction is invalid.
    #[fail(display = "{}", _0)]
    Cellbase(#[fail(cause)] CellbaseError),

    /// This error is returned when the committed transactions does not meet the 2-phases
    /// propose-then-commit consensus rule.
    #[fail(display = "{}", _0)]
    Commit(#[fail(cause)] CommitError),

    /// Number of proposals exceeded the limit.
    // NOTE: the original name is ExceededMaximumProposalsLimit
    #[fail(display = "Too many proposals")]
    TooManyProposals,

    /// Cycles consumed by all scripts in all commit transactions of the block exceed
    /// the maximum allowed cycles in consensus rules
    // NOTE: the original name is ExceededMaximumCycles
    #[fail(display = "Too much cycles")]
    TooMuchCycles,

    /// The size of the block exceeded the limit.
    // NOTE: the original name is ExceededMaximumBlockBytes
    #[fail(display = "Too large size")]
    TooLargeSize,

    /// The field version in block header is not allowed.
    // NOTE: the original name is Version
    #[fail(display = "Mismatched version")]
    MismatchedVersion,
}

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum CommitError {
    /// Ancestor not found, should not happen, we check header first and check ancestor.
    // NOTE: the original name is AncestorNotFound
    #[fail(display = "Nonexistent ancestor")]
    NonexistentAncestor,

    /// Break propose-then-commit consensus rule.
    // NOTE: the original name is Invalid
    #[fail(display = "Not in proposal window")]
    NotInProposalWindow,
}

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum CellbaseError {
    #[fail(display = "Invalid input")]
    InvalidInput,

    #[fail(display = "Invalid reward amount")]
    InvalidRewardAmount,

    #[fail(display = "Invalid reward target")]
    InvalidRewardTarget,

    #[fail(display = "Invalid witness")]
    InvalidWitness,

    #[fail(display = "Invalid type script")]
    InvalidTypeScript,

    #[fail(display = "Invalid quantity")]
    InvalidQuantity,

    #[fail(display = "Invalid position")]
    InvalidPosition,
}

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum UnclesError {
    // NOTE: the original name is OverCount
    #[fail(display = "Too many uncles, max({}) < actual({})", max, actual)]
    TooManyUncles { max: u32, actual: u32 },

    // NOTE: the original name is MissMatchCount
    #[fail(
        display = "Unmatched count, expected({}), actual({})",
        expected, actual
    )]
    UnmatchedCount { expected: u32, actual: u32 },

    #[fail(
        display = "Invalid depth, min({}), max({}), actual({})",
        min, max, actual
    )]
    InvalidDepth { max: u64, min: u64, actual: u64 },

    // NOTE: the original name is InvalidHash
    #[fail(
        display = "Unmatched uncles-hash, expected({:#x}), actual({:#x})",
        expected, actual
    )]
    UnmatchedUnclesHash { expected: H256, actual: H256 },

    // NOTE: the original name is InvalidNumber
    #[fail(display = "Unmatched block number")]
    UnmatchedBlockNumber,

    #[fail(display = "Unmatched difficulty")]
    UnmatchedDifficulty,

    // NOTE: the original name is InvalidDifficultyEpoch
    #[fail(display = "Unmatched epoch number")]
    UnmatchedEpochNumber,

    // NOTE: the original name is ProposalsHash
    #[fail(display = "Unmatched proposal root")]
    UnmatchedProposalRoot,

    // NOTE: the original name is ProposalDuplicate
    #[fail(display = "Duplicated proposal transactions")]
    DuplicatedProposalTransactions,

    // NOTE: the original name is Duplicate
    #[fail(display = "Duplicated uncles {:#x}", _0)]
    DuplicatedUncles(H256),

    #[fail(display = "Double Inclusion {:#x}", _0)]
    DoubleInclusion(H256),

    #[fail(display = "Descendant limit")]
    DescendantLimit,

    // NOTE: the original name is ExceededMaximumProposalsLimit
    #[fail(display = "Too many proposals")]
    TooManyProposals,
}
