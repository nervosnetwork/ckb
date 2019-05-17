use failure::Fail;

#[derive(Debug, Fail, Eq, PartialEq)]
pub enum Error {
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
}
