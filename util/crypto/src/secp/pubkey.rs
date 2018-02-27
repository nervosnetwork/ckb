use bigint::H512;
use error::Error;
use secp::Message;
use secp::SECP256K1;
use secp::secp256k1::Error as SecpError;
use secp::secp256k1::Message as SecpMessage;
use secp::secp256k1::key;
use secp::signature::Signature;
use std::ops;

#[derive(Debug, Eq, PartialEq)]
pub struct Pubkey {
    inner: H512,
}

impl Pubkey {
    /// Checks that `signature` is a valid ECDSA signature for `message` using the public
    /// key `pubkey`
    pub fn verify(&self, message: &Message, signature: &Signature) -> Result<bool, Error> {
        let context = &SECP256K1;

        // non-compressed key prefix 4
        let prefix_key: [u8; 65] = {
            let mut temp = [4u8; 65];
            temp[1..65].copy_from_slice(self);
            temp
        };

        let pubkey = key::PublicKey::from_slice(context, &prefix_key)?;
        let recoverable_signature = signature.to_recoverable()?;
        let signature = recoverable_signature.to_standard(context);

        let message = SecpMessage::from_slice(message)?;
        match context.verify(&message, &signature, &pubkey) {
            Ok(_) => Ok(true),
            Err(SecpError::IncorrectSignature) => Ok(false),
            Err(x) => Err(x.into()),
        }
    }
}

impl From<H512> for Pubkey {
    fn from(key: H512) -> Self {
        Pubkey { inner: key }
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
        let mut pubkey = H512::default();
        pubkey.copy_from_slice(&serialized[1..65]);
        pubkey.into()
    }
}
