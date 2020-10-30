use failure::Fail;
use secp256k1::Error as SecpError;

/// TODO(doc): @zhangsoledad
#[derive(Debug, PartialEq, Eq, Fail)]
pub enum Error {
    /// TODO(doc): @zhangsoledad
    #[fail(display = "invalid privkey")]
    InvalidPrivKey,
    /// TODO(doc): @zhangsoledad
    #[fail(display = "invalid pubkey")]
    InvalidPubKey,
    /// TODO(doc): @zhangsoledad
    #[fail(display = "invalid signature")]
    InvalidSignature,
    /// TODO(doc): @zhangsoledad
    #[fail(display = "invalid message")]
    InvalidMessage,
    /// TODO(doc): @zhangsoledad
    #[fail(display = "invalid recovery_id")]
    InvalidRecoveryId,
    /// TODO(doc): @zhangsoledad
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
