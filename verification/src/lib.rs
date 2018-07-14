extern crate bigint;
extern crate ethash;
extern crate merkle_root;
extern crate nervos_chain as chain;
extern crate nervos_core as core;
#[cfg(test)]
extern crate nervos_db as db;
extern crate nervos_time as time;
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
pub use transaction_verifier::TransactionVerifier;

pub trait Verifier {
    fn verify(&self) -> Result<(), Error>;
}
