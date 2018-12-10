use super::error::Error;
use super::signature::Signature;
use super::Message;
use super::SECP256K1;
use numext_fixed_hash::H512;
use secp256k1::key;
use secp256k1::Message as SecpMessage;
use std::{fmt, ops};

#[derive(Debug, Eq, PartialEq)]
pub struct Pubkey {
    inner: H512,
}

impl Pubkey {
    /// Checks that `signature` is a valid ECDSA signature for `message` using the public
    /// key `pubkey`
    pub fn verify(&self, message: &Message, signature: &Signature) -> Result<(), Error> {
        let context = &SECP256K1;

        // non-compressed key prefix 4
        let prefix_key: [u8; 65] = {
            let mut temp = [4u8; 65];
            temp[1..65].copy_from_slice(self.inner.as_bytes());
            temp
        };

        let pubkey = key::PublicKey::from_slice(context, &prefix_key)?;
        let recoverable_signature = signature.to_recoverable()?;
        let signature = recoverable_signature.to_standard(context);

        let message = SecpMessage::from_slice(message.as_bytes())?;
        context.verify(&message, &signature, &pubkey)?;
        Ok(())
    }

    pub fn verify_schnorr(&self, message: &Message, signature: &Signature) -> Result<(), Error> {
        let context = &SECP256K1;

        // non-compressed key prefix 4
        let prefix_key: [u8; 65] = {
            let mut temp = [4u8; 65];
            temp[1..65].copy_from_slice(self.inner.as_bytes());
            temp
        };

        let pubkey = key::PublicKey::from_slice(context, &prefix_key)?;
        let schnorr_signature = signature.to_schnorr();

        let message = SecpMessage::from_slice(message.as_bytes())?;
        context.verify_schnorr(&message, &schnorr_signature, &pubkey)?;
        Ok(())
    }

    pub fn serialize(&self) -> Vec<u8> {
        let context = &SECP256K1;

        // non-compressed key prefix 4
        let prefix_key: [u8; 65] = {
            let mut temp = [4u8; 65];
            temp[1..65].copy_from_slice(self.inner.as_bytes());
            temp
        };
        let pubkey = key::PublicKey::from_slice(context, &prefix_key).unwrap();
        Vec::from(&pubkey.serialize()[..])
    }
}

impl From<[u8; 64]> for Pubkey {
    fn from(key: [u8; 64]) -> Self {
        Pubkey { inner: key.into() }
    }
}

impl From<H512> for Pubkey {
    fn from(key: H512) -> Self {
        Pubkey { inner: key }
    }
}

impl Into<H512> for Pubkey {
    fn into(self) -> H512 {
        self.inner
    }
}

impl ops::Deref for Pubkey {
    type Target = H512;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<key::PublicKey> for Pubkey {
    fn from(key: key::PublicKey) -> Self {
        let serialized = key.serialize_uncompressed();
        let mut pubkey = [0u8; 64];
        pubkey.copy_from_slice(&serialized[1..65]);
        Pubkey {
            inner: pubkey.into(),
        }
    }
}

impl fmt::Display for Pubkey {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:x}", self.inner)
    }
}
