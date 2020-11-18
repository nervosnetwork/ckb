use super::error::Error;
use super::signature::Signature;
use super::{Message, Pubkey, SECP256K1};
use ckb_fixed_hash::H256;
use secp256k1::key;
use secp256k1::Message as SecpMessage;
use std::str::FromStr;
use std::{ptr, sync::atomic};

/// TODO(doc): @zhangsoledad
#[derive(Clone, Eq, PartialEq)]
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

    /// TODO(doc): @zhangsoledad
    pub fn pubkey(&self) -> Result<Pubkey, Error> {
        let context = &SECP256K1;
        let privkey = key::SecretKey::from_slice(self.inner.as_bytes())?;
        let pubkey = key::PublicKey::from_secret_key(context, &privkey);
        Ok(Pubkey::from(pubkey))
    }

    /// TODO(doc): @zhangsoledad
    pub fn from_slice(key: &[u8]) -> Self {
        assert_eq!(32, key.len(), "should provide 32-byte length slice");

        let mut h = [0u8; 32];
        h.copy_from_slice(&key[0..32]);
        Privkey { inner: h.into() }
    }

    // uses core::ptr::write_volatile and core::sync::atomic memory fences to zeroing
    pub(crate) fn zeroize(&mut self) {
        for elem in self.inner.0.iter_mut() {
            volatile_write(elem, Default::default());
            atomic_fence();
        }
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

impl From<key::SecretKey> for Privkey {
    fn from(key: key::SecretKey) -> Self {
        let mut h = [0u8; 32];
        h.copy_from_slice(&key[0..32]);
        Privkey { inner: h.into() }
    }
}

#[inline]
fn atomic_fence() {
    atomic::compiler_fence(atomic::Ordering::SeqCst);
}

#[inline]
fn volatile_write<T: Copy + Sized>(dst: &mut T, src: T) {
    unsafe { ptr::write_volatile(dst, src) }
}

impl Drop for Privkey {
    fn drop(&mut self) {
        self.zeroize()
    }
}
