extern crate bigint;
extern crate ckb_core as core;
extern crate ckb_pow as pow;
extern crate ckb_script as script;
extern crate ckb_shared;
extern crate ckb_time as time;
extern crate fnv;
extern crate merkle_root;
extern crate rayon;

#[cfg(test)]
extern crate ckb_chain as chain;
#[cfg(test)]
extern crate ckb_chain_spec as chain_spec;
#[cfg(test)]
extern crate ckb_db as db;
#[cfg(test)]
extern crate ckb_notify as notify;
#[cfg(test)]
extern crate hash;

mod block_verifier;
mod error;
mod header_verifier;
mod shared;
mod transaction_verifier;

#[cfg(test)]
pub mod tests;

pub use block_verifier::{BlockVerifier, HeaderResolverWrapper};
pub use error::{Error, TransactionError};
pub use header_verifier::{HeaderResolver, HeaderVerifier};
pub use transaction_verifier::TransactionVerifier;

pub trait Verifier {
    type Target;
    fn verify(&self, target: &Self::Target) -> Result<(), Error>;
}
