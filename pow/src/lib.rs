//! TODO(doc): @quake
use byteorder::{ByteOrder, LittleEndian};
use ckb_types::{
    packed::{Byte32, Header},
    prelude::*,
};
use serde::{Deserialize, Serialize};
use std::any::Any;
use std::fmt;
use std::sync::Arc;

mod dummy;
mod eaglesong;
mod eaglesong_blake2b;

#[cfg(test)]
mod tests;

pub use crate::dummy::DummyPowEngine;
pub use crate::eaglesong::EaglesongPowEngine;
pub use crate::eaglesong_blake2b::EaglesongBlake2bPowEngine;

/// The PoW engine traits bundled
#[derive(Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Debug)]
#[serde(tag = "func", content = "params")]
pub enum Pow {
    /// Mocking dummy PoW engine
    Dummy,
    /// The Eaglesong PoW engine
    /// Check details of Eaglesong from: https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0010-eaglesong/0010-eaglesong.md
    Eaglesong,
    /// The Eaglesong PoW engine, similar to `Eaglesong`, but using `blake2b` hash as the final output.
    /// Check details of blake2b from: https://tools.ietf.org/html/rfc7693 and blake2b-rs from: https://github.com/nervosnetwork/blake2b-rs
    EaglesongBlake2b,
}

impl fmt::Display for Pow {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Pow::Dummy => write!(f, "Dummy"),
            Pow::Eaglesong => write!(f, "Eaglesong"),
            Pow::EaglesongBlake2b => write!(f, "EaglesongBlake2b"),
        }
    }
}

impl Pow {
    /// Allocates a new engine instance
    pub fn engine(&self) -> Arc<dyn PowEngine> {
        match *self {
            Pow::Dummy => Arc::new(DummyPowEngine),
            Pow::Eaglesong => Arc::new(EaglesongPowEngine),
            Pow::EaglesongBlake2b => Arc::new(EaglesongBlake2bPowEngine),
        }
    }

    /// Determine whether this engine is dummy(mocking)
    pub fn is_dummy(&self) -> bool {
        *self == Pow::Dummy
    }
}

/// Combine pow_hash and nonce to a message, in little endian
pub fn pow_message(pow_hash: &Byte32, nonce: u128) -> [u8; 48] {
    let mut message = [0; 48];
    message[0..32].copy_from_slice(pow_hash.as_slice());
    LittleEndian::write_u128(&mut message[32..48], nonce);
    message
}

/// A trait for PoW engine, which is used to verify PoW
pub trait PowEngine: Send + Sync + AsAny {
    /// Verify header
    fn verify(&self, header: &Header) -> bool;
}

/// A trait for casting to trait `Any`
pub trait AsAny {
    /// Cast to trait `Any`
    fn as_any(&self) -> &dyn Any;
}

impl<T: Any> AsAny for T {
    fn as_any(&self) -> &dyn Any {
        self
    }
}
