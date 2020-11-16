use failure::Fail;
use secp256k1::Error as SecpError;

/// The error type wrap SecpError
#[derive(Debug, PartialEq, Eq, Fail)]
pub enum Error {
    /// Invalid privkey
    #[fail(display = "invalid privkey")]
    InvalidPrivKey,
    /// Invalid pubkey
    #[fail(display = "invalid pubkey")]
    InvalidPubKey,
    /// Invalid signature
    #[fail(display = "invalid signature")]
    InvalidSignature,
    /// Invalid message
    #[fail(display = "invalid message")]
    InvalidMessage,
    /// Invalid recovery_id
    #[fail(display = "invalid recovery_id")]
    InvalidRecoveryId,
    /// Any error not part of this list.
    #[fail(display = "{}", _0)]
    Other(String),
}

impl From<SecpError> for Error {
    fn from(e: SecpError) -> Self {
        match e {
            SecpError::InvalidPublicKey => Error::InvalidPubKey,
            SecpError::InvalidSecretKey => Error::InvalidPrivKey,
            SecpError::InvalidMessage => Error::InvalidMessage,
            SecpError::InvalidRecoveryId => Error::InvalidRecoveryId,
            _ => Error::InvalidSignature,
        }
    }
}
