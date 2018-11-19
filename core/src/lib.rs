extern crate bigint;
extern crate bls;
extern crate crypto;
extern crate hash;
extern crate merkle_root;
extern crate nervos_time as time;
#[macro_use]
extern crate serde_derive;

extern crate bincode;
extern crate serde;

pub mod transaction;
pub mod block;
pub mod proof;
pub mod difficulty;
pub mod global;
pub mod adapter;
pub mod error;
pub mod keygroup;

pub use error::Error;

pub type PublicKey = bigint::H512;
pub type ProofPublickey = bigint::H328;
pub type ProofPublicG = bigint::H328;
