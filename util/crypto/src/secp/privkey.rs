use bigint::H256;
use error::Error;
use secp::{Message, SECP256K1};
use secp::secp256k1::Message as SecpMessage;
use secp::secp256k1::key;
use secp::signature::Signature;
use std::ops;
use std::str::FromStr;

#[derive(Debug, Eq, PartialEq)]
pub struct Privkey {
    /// ECDSA key.
    inner: H256,
}

impl Privkey {
    /// sign recoverable
    pub fn sign_recoverable(&self, message: &Message) -> Result<Signature, Error> {
        let context = &SECP256K1;
        let message = message.as_ref();
        let privkey = key::SecretKey::from_slice(context, &self.inner)?;
        let message = SecpMessage::from_slice(message)?;
        let data = context.sign_recoverable(&message, &privkey)?;
        let (rec_id, data) = data.serialize_compact(context);
        Ok(Signature::from_compact(rec_id, data))
    }

    pub fn from_slice(key: &[u8]) -> Self {
        assert_eq!(32, key.len(), "should provide 32-byte length slice");

        let mut h = H256::default();
        h.copy_from_slice(&key[0..32]);
        Privkey { inner: h }
    }
}

impl From<H256> for Privkey {
    fn from(key: H256) -> Self {
        Privkey { inner: key }
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
        let mut h = H256::default();
        h.copy_from_slice(&key[0..32]);
        Privkey { inner: h }
    }
}
