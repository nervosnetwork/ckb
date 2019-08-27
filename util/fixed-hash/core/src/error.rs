use failure::Fail;

#[derive(Debug, Fail)]
pub enum FromStrError {
    #[fail(display = "invalid character code `{}` at {}", chr, idx)]
    InvalidCharacter { chr: u8, idx: usize },
    #[fail(display = "invalid length: {}", _0)]
    InvalidLength(usize),
}

#[derive(Debug, Fail)]
pub enum FromSliceError {
    #[fail(display = "invalid length: {}", _0)]
    InvalidLength(usize),
}
