//! Conversion errors.

use thiserror::Error;

/// The associated error of [`FromStr`] which can be returned from parsing a string.
///
/// [`FromStr`]: https://doc.rust-lang.org/std/str/trait.FromStr.html#associatedtype.Err
#[derive(Error, Debug, PartialEq, Eq)]
pub enum FromStrError {
    /// Invalid character.
    #[error("invalid character code `{chr}` at {idx}")]
    InvalidCharacter {
        /// The value of the invalid character.
        chr: u8,
        /// The index of the invalid character.
        idx: usize,
    },
    /// Invalid length.
    #[error("invalid length: {0}")]
    InvalidLength(usize),
}

/// The error which can be returned when convert a byte slice back into a Hash.
#[derive(Error, Debug, PartialEq, Eq)]
pub enum FromSliceError {
    /// Invalid length.
    #[error("invalid length: {0}")]
    InvalidLength(usize),
}
