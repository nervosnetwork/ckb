use super::error::Error;
use super::signature::Signature;
use super::{Message, Pubkey, SECP256K1};
use numext_fixed_hash::H256;
use secp256k1::key;
use secp256k1::Message as SecpMessage;
use std::str::FromStr;
use std::{fmt, ops};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Privkey {
    /// ECDSA key.
    inner: H256,
}

impl Privkey {
    /// sign recoverable
    pub fn sign_recoverable(&self, message: &Message) -> Result<Signature, Error> {
        let context = &SECP256K1;
        let message = message.as_ref();
        let privkey = key::SecretKey::from_slice(self.inner.as_bytes())?;
        let message = SecpMessage::from_slice(message)?;
        let data = context.sign_recoverable(&message, &privkey);
        let (rec_id, data) = data.serialize_compact();
        Ok(Signature::from_compact(rec_id, data))
    }

    pub fn pubkey(&self) -> Result<Pubkey, Error> {
        let context = &SECP256K1;
        let privkey = key::SecretKey::from_slice(self.inner.as_bytes())?;
        let pubkey = key::PublicKey::from_secret_key(context, &privkey);
        Ok(Pubkey::from(pubkey))
    }

    pub fn from_slice(key: &[u8]) -> Self {
        assert_eq!(32, key.len(), "should provide 32-byte length slice");

        let mut h = [0u8; 32];
        h.copy_from_slice(&key[0..32]);
        Privkey { inner: h.into() }
    }
}

impl From<H256> for Privkey {
    fn from(key: H256) -> Self {
        Privkey { inner: key }
    }
}

impl Into<H256> for Privkey {
    fn into(self) -> H256 {
        self.inner
    }
}

impl AsRef<[u8]> for Privkey {
    fn as_ref(&self) -> &[u8] {
        self.inner.as_bytes()
    }
}

impl FromStr for Privkey {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(H256::from_str(s)
            .map_err(|e| Error::Other(format!("{:?}", e)))?
            .into())
    }
}

impl ops::Deref for Privkey {
    type Target = H256;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<key::SecretKey> for Privkey {
    fn from(key: key::SecretKey) -> Self {
        let mut h = [0u8; 32];
        h.copy_from_slice(&key[0..32]);
        Privkey { inner: h.into() }
    }
}

impl fmt::Display for Privkey {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:x}", self.inner)
    }
}
