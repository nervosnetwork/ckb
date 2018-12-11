use failure::Fail;
use secp256k1::Error as SecpError;

#[derive(Debug, PartialEq, Eq, Fail)]
pub enum Error {
    #[fail(display = "invalid privkey")]
    InvalidPrivKey,
    #[fail(display = "invalid pubkey")]
    InvalidPubKey,
    #[fail(display = "invalid signature")]
    InvalidSignature,
    #[fail(display = "invalid message")]
    InvalidMessage,
    #[fail(display = "invalid recovery_id")]
    InvalidRecoveryId,
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
