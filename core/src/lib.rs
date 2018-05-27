extern crate bigint;
extern crate bincode;
extern crate crypto;
extern crate hash;
extern crate merkle_root;
extern crate nervos_protocol;
extern crate nervos_time as time;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate bit_vec;

pub mod block;
pub mod cell;
pub mod chain;
pub mod difficulty;
pub mod error;
pub mod extras;
pub mod global;
pub mod header;
pub mod transaction;
pub mod transaction_meta;

pub use error::Error;

pub type PublicKey = bigint::H512;
pub type ProofPublickey = bigint::H328;
pub type ProofPublicG = bigint::H328;
