//! Conversion errors.

use failure::Fail;

/// The associated error of [`FromStr`] which can be returned from parsing a string.
///
/// [`FromStr`]: https://doc.rust-lang.org/std/str/trait.FromStr.html#associatedtype.Err
#[derive(Debug, Fail)]
pub enum FromStrError {
    /// Invalid character.
    #[fail(display = "invalid character code `{}` at {}", chr, idx)]
    InvalidCharacter {
        /// The value of the invalid character.
        chr: u8,
        /// The index of the invalid character.
        idx: usize,
    },
    /// Invalid length.
    #[fail(display = "invalid length: {}", _0)]
    InvalidLength(usize),
}

/// The error which can be returned when convert a byte slice back into a Hash.
#[derive(Debug, Fail)]
pub enum FromSliceError {
    /// Invalid length.
    #[fail(display = "invalid length: {}", _0)]
    InvalidLength(usize),
}
