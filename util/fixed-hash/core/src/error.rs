//! TODO(doc): @yangby-cryptape

use failure::Fail;

/// TODO(doc): @yangby-cryptape
#[derive(Debug, Fail)]
pub enum FromStrError {
    /// TODO(doc): @yangby-cryptape
    #[fail(display = "invalid character code `{}` at {}", chr, idx)]
    InvalidCharacter {
        /// TODO(doc): @yangby-cryptape
        chr: u8,
        /// TODO(doc): @yangby-cryptape
        idx: usize,
    },
    /// TODO(doc): @yangby-cryptape
    #[fail(display = "invalid length: {}", _0)]
    InvalidLength(usize),
}

/// TODO(doc): @yangby-cryptape
#[derive(Debug, Fail)]
pub enum FromSliceError {
    /// TODO(doc): @yangby-cryptape
    #[fail(display = "invalid length: {}", _0)]
    InvalidLength(usize),
}
