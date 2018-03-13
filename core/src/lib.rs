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

pub mod adapter;
pub mod block;
pub mod difficulty;
pub mod error;
pub mod global;
pub mod keygroup;
pub mod proof;
pub mod transaction;

pub use error::Error;

pub type PublicKey = bigint::H512;
pub type ProofPublickey = bigint::H328;
pub type ProofPublicG = bigint::H328;
