use secp256k1::Error as SecpError;
use thiserror::Error;

/// The error type wrap SecpError
#[derive(Error, Debug, PartialEq, Eq)]
pub enum Error {
    /// Invalid privkey
    #[error("invalid privkey")]
    InvalidPrivKey,
    /// Invalid pubkey
    #[error("invalid pubkey")]
    InvalidPubKey,
    /// Invalid signature
    #[error("invalid signature")]
    InvalidSignature,
    /// Invalid message
    #[error("invalid message")]
    InvalidMessage,
    /// Invalid recovery_id
    #[error("invalid recovery_id")]
    InvalidRecoveryId,
    /// Any error not part of this list.
    #[error("{0}")]
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
