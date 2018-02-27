#![allow(dead_code)]

extern crate secp256k1;

use self::secp256k1::Error as SecpError;
use bigint::H256;
use error::Error;

pub type Message = H256;

lazy_static! {
    pub static ref SECP256K1: secp256k1::Secp256k1 = secp256k1::Secp256k1::new();
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

mod privkey;
mod pubkey;
mod signature;
mod generator;

pub use self::generator::Generator;
pub use self::privkey::Privkey;
pub use self::pubkey::Pubkey;
pub use self::signature::Signature;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sign_verify() {
        let gen = Generator::new();
        let (privkey, pubkey) = gen.random_keypair().unwrap();
        let message = Message::default();
        let signature = privkey.sign_recoverable(&message).unwrap();
        assert!(pubkey.verify(&message, &signature).unwrap());
    }

    #[test]
    fn test_recover() {
        let gen = Generator::new();
        let (privkey, pubkey) = gen.random_keypair().unwrap();
        let message = Message::default();
        let signature = privkey.sign_recoverable(&message).unwrap();
        assert_eq!(pubkey, signature.recover(&message).unwrap());
    }
}
