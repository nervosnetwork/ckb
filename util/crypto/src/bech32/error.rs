use failure::Fail;
use std::error::Error as StdError;
use std::string::FromUtf8Error;

/// Error types for Bech32 encoding / decoding
#[derive(Debug, PartialEq, Eq, Fail)]
pub enum Error {
    #[fail(display = "missing human-readable separator")]
    MissingSeparator,
    #[fail(display = "invalid checksum")]
    InvalidChecksum,
    #[fail(display = "invalid length")]
    InvalidLength,
    #[fail(display = "invalid character (code={})", _0)]
    InvalidChar(u8),
    #[fail(display = "invalid data point ({})", _0)]
    InvalidData(u8),
    #[fail(display = "mixed-case strings not allowed")]
    MixedCase,
    #[fail(display = "{}", _0)]
    Utf8Error(String),
}

impl From<FromUtf8Error> for Error {
    fn from(e: FromUtf8Error) -> Self {
        Error::Utf8Error(e.description().to_owned())
    }
}
