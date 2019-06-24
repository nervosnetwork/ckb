use std::io;

use failure::Fail;
use numext_fixed_hash::H160;

#[derive(Debug, Fail, Eq, PartialEq)]
pub enum Error {
    #[fail(display = "Account locked: {:x}", _0)]
    AccountLocked(H160),

    #[fail(display = "Account not found: {:x}", _0)]
    AccountNotFound(H160),

    #[fail(display = "Key mismatch, got {:x}, expected: {:x}", got, expected)]
    KeyMismatch { got: H160, expected: H160 },

    #[fail(display = "Wrong password for {:x}", _0)]
    WrongPassword(H160),

    #[fail(display = "Check password failed")]
    CheckPasswordFailed,

    #[fail(display = "Parse json failed: {}", _0)]
    ParseJsonFailed(String),

    #[fail(display = "Unsupported cipher: {}", _0)]
    UnsupportedCipher(String),

    #[fail(display = "Unsupported kdf: {}", _0)]
    UnsupportedKdf(String),

    #[fail(display = "Generate secp256k1 secret failed, tried: {}", _0)]
    GenSecpFailed(u16),

    #[fail(display = "Invalid secp256k1 secret key")]
    InvalidSecpSecret,

    #[fail(display = "IO error: {}", _0)]
    Io(String),

    #[fail(display = "Other error: {}", _0)]
    Other(String),
}

impl From<String> for Error {
    fn from(err: String) -> Error {
        Error::Other(err)
    }
}

impl From<&str> for Error {
    fn from(err: &str) -> Error {
        Error::Other(err.to_owned())
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::Io(err.to_string())
    }
}
