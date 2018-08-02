extern crate bigint;
extern crate ckb_chain as chain;
extern crate ckb_core as core;
#[cfg(test)]
extern crate ckb_db as db;
#[cfg(test)]
extern crate ckb_notify as notify;
extern crate ckb_script as script;
extern crate ckb_time as time;
extern crate ethash;
extern crate merkle_root;
extern crate rayon;

mod block_verifier;
// mod chain_verifier;
mod error;
mod header_verifier;
mod pow_verifier;
mod shared;
mod transaction_verifier;

#[cfg(test)]
pub mod tests;

pub use block_verifier::BlockVerifier;
pub use error::{Error, TransactionError};
pub use header_verifier::HeaderVerifier;
pub use pow_verifier::{EthashVerifier, PowVerifier};
pub use transaction_verifier::TransactionVerifier;

pub trait Verifier {
    fn verify(&self) -> Result<(), Error>;
}
