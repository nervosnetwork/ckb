use ckb_types::packed::{Byte32, ProposalShortId};
use failure::Fail;

#[derive(Debug, Fail, Eq, PartialEq)]
pub enum Error {
    #[fail(display = "internal error: {}", _0)]
    Internal(Internal),
    #[fail(display = "misbehavior error: {}", _0)]
    Misbehavior(Misbehavior),
    #[fail(display = "ignored error: {}", _0)]
    Ignored(Ignored),
}

#[derive(Debug, Fail, Eq, PartialEq)]
pub enum Internal {
    #[fail(display = "InflightBlocksReachLimit")]
    InflightBlocksReachLimit,
}

#[derive(Debug, Fail, Eq, PartialEq)]
pub enum Misbehavior {
    #[fail(display = "CompactBlockError::CellbaseNotPrefilled")]
    CellbaseNotPrefilled,
    #[fail(display = "CompactBlockError::DuplicatedShortIds")]
    DuplicatedShortIds,
    #[fail(display = "CompactBlockError::UnorderedPrefilledTransactions")]
    UnorderedPrefilledTransactions,
    #[fail(display = "CompactBlockError::OverflowPrefilledTransactions")]
    OverflowPrefilledTransactions,
    #[fail(display = "CompactBlockError::IntersectedPrefilledTransactions")]
    IntersectedPrefilledTransactions,
    #[fail(display = "CompactBlockError::InvalidTransactionRoot")]
    InvalidTransactionRoot,
    #[fail(
        display = "InvalidBlockTransactionsLength(expected: {}, actual: {})",
        expected, actual
    )]
    InvalidBlockTransactionsLength { expected: usize, actual: usize },
    #[fail(
        display = "InvalidBlockTransactions(expected: {:#?}, actual: {:#?})",
        expected, actual
    )]
    InvalidBlockTransactions {
        expected: ProposalShortId,
        actual: ProposalShortId,
    },
    #[fail(
        display = "block uncles' length is invalid, expect {}, but got {}",
        expect, got
    )]
    InvalidBlockUnclesLength { expect: usize, got: usize },
    #[fail(
        display = "block unlces' hash is invalid, expect {:#?}, but got {:#?}",
        expect, got
    )]
    InvalidBlockUncles { expect: Byte32, got: Byte32 },
    #[fail(display = "BlockInvalid")]
    BlockInvalid,
    #[fail(display = "HeaderInvalid")]
    HeaderInvalid,
    #[fail(display = "UncleInvalid")]
    InvalidUncle,
}

#[derive(Debug, Fail, Eq, PartialEq)]
pub enum Ignored {
    #[fail(display = "Already pending compact block")]
    AlreadyPending,
    #[fail(display = "Already in-flight compact block")]
    AlreadyInFlight,
    #[fail(display = "Already stored")]
    AlreadyStored,
    #[fail(display = "Block is too old")]
    TooOldBlock,
}
