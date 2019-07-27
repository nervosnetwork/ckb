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
    #[fail(display = "BlockInvalid")]
    BlockInvalid,
    #[fail(display = "HeaderInvalid")]
    HeaderInvalid,
}

#[derive(Debug, Fail, Eq, PartialEq)]
pub enum Ignored {
    #[fail(display = "Not a better block")]
    NotBetter,
    #[fail(display = "Already pending compact block")]
    AlreadyPending,
    #[fail(display = "Already in-flight compact block")]
    AlreadyInFlight,
    #[fail(display = "Already stored")]
    AlreadyStored,
}
