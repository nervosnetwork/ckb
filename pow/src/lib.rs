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

pub use crate::dummy::DummyPowEngine;
pub use crate::eaglesong::EaglesongPowEngine;
pub use crate::eaglesong_blake2b::EaglesongBlake2bPowEngine;

/// TODO(doc): @quake
#[derive(Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Debug)]
#[serde(tag = "func", content = "params")]
pub enum Pow {
    /// TODO(doc): @quake
    Dummy,
    /// TODO(doc): @quake
    Eaglesong,
    /// TODO(doc): @quake
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
    /// TODO(doc): @quake
    pub fn engine(&self) -> Arc<dyn PowEngine> {
        match *self {
            Pow::Dummy => Arc::new(DummyPowEngine),
            Pow::Eaglesong => Arc::new(EaglesongPowEngine),
            Pow::EaglesongBlake2b => Arc::new(EaglesongBlake2bPowEngine),
        }
    }

    /// TODO(doc): @quake
    pub fn is_dummy(&self) -> bool {
        *self == Pow::Dummy
    }
}

/// TODO(doc): @quake
pub fn pow_message(pow_hash: &Byte32, nonce: u128) -> [u8; 48] {
    let mut message = [0; 48];
    message[0..32].copy_from_slice(pow_hash.as_slice());
    LittleEndian::write_u128(&mut message[32..48], nonce);
    message
}

/// TODO(doc): @quake
pub trait PowEngine: Send + Sync + AsAny {
    /// TODO(doc): @quake
    fn verify(&self, header: &Header) -> bool;
}

/// TODO(doc): @quake
pub trait AsAny {
    /// TODO(doc): @quake
    fn as_any(&self) -> &dyn Any;
}

impl<T: Any> AsAny for T {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use ckb_hash::blake2b_256;
    #[test]
    fn test_pow_message() {
        let zero_hash = blake2b_256(&[]).pack();
        let nonce = u128::max_value();
        let message = pow_message(&zero_hash, nonce);
        assert_eq!(
            message.to_vec(),
            [
                68, 244, 198, 151, 68, 213, 248, 197, 93, 100, 32, 98, 148, 157, 202, 228, 155,
                196, 231, 239, 67, 211, 136, 197, 161, 47, 66, 181, 99, 61, 22, 62, 255, 255, 255,
                255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255
            ]
            .to_vec()
        );
    }
}
