//! `secp256k1` wrapper

use ckb_fixed_hash::H256;

/// A (hashed) message input to an ECDSA signature
pub type Message = H256;

/// The reference to lazily-initialized static secp256k1 engine, used to execute all signature operations
pub static SECP256K1: std::sync::LazyLock<secp256k1::Secp256k1<secp256k1::All>> =
    std::sync::LazyLock::new(secp256k1::Secp256k1::new);

mod error;
mod generator;
mod privkey;
mod pubkey;
mod signature;

pub use self::error::Error;
pub use self::generator::Generator;
pub use self::privkey::Privkey;
pub use self::pubkey::Pubkey;
pub use self::signature::Signature;

#[cfg(test)]
mod tests;
