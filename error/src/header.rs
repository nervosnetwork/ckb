use failure::Fail;
use numext_fixed_uint::U256;

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum HeaderError {
    /// The parent of this header was marked as invalid
    #[fail(display = "Invalid parent")]
    InvalidParent,

    /// The field pow in block header is invalid
    #[fail(display = "{}", _0)]
    Pow(#[fail(cause)] PowError),

    /// The field timestamp in block header is invalid.
    #[fail(display = "{}", _0)]
    Timestamp(#[fail(cause)] TimestampError),

    /// The field number in block header is invalid.
    #[fail(display = "{}", _0)]
    Number(#[fail(cause)] NumberError),

    /// The field difficulty in block header is invalid.
    #[fail(display = "{}", _0)]
    Epoch(#[fail(cause)] EpochError),
}

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum PowError {
    #[fail(display = "Expect pow boundary {:#x} but got {:#x}", expected, actual)]
    Boundary { expected: U256, actual: U256 },

    #[fail(display = "Invalid proof")]
    InvalidProof,
}

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum TimestampError {
    #[fail(display = "Too old block timestamp min({}) > actual({})", actual, min)]
    BlockTimeTooOld { min: u64, actual: u64 },
    #[fail(display = "Too new block timestamp max({}) < actual({})", actual, max)]
    BlockTimeTooNew { max: u64, actual: u64 },
}

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
#[fail(display = "Expect block number {} but got {}", expected, actual)]
pub struct NumberError {
    pub expected: u64,
    pub actual: u64,
}

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum EpochError {
    // NOTE: the original name is DifficultyMismatch
    #[fail(display = "Expect difficulty {:#x} but got {:#x}", expected, actual)]
    UnmatchedDifficulty { expected: U256, actual: U256 },

    // NOTE: the original name is NumberMismatch
    #[fail(display = "Expect epoch number {} but got {}", expected, actual)]
    UnmatchedNumber { expected: u64, actual: u64 },

    // NOTE: the original name is AncestorNotFound
    #[fail(display = "Missing ancestor")]
    MissingAncestor,
}
