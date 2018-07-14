extern crate bigint;
extern crate ethash;
extern crate merkle_root;
extern crate nervos_core as core;
extern crate nervos_time as time;
extern crate rayon;

mod block_verifier;
mod chain_verifier;
mod error;
mod header_verifier;
mod pow_verifier;
mod shared;
mod tests;
mod transaction_verifier;

pub use block_verifier::BlockVerifier;
pub use chain_verifier::ChainVerifier;
pub use error::{Error, TransactionError};
pub use header_verifier::HeaderVerifier;
pub use pow_verifier::{EthashVerifier, NoopVerifier, PowVerifier, PowVerifierImpl};
pub use transaction_verifier::TransactionVerifier;

pub trait Verifier {
    fn verify(&self) -> Result<(), Error>;
}

#[derive(Debug, Clone)]
pub enum VerifierType {
    Normal,
    // Skip seal verification
    NoSeal,
    // Used in tests.
    Noop,
}

// TODO
// 1. add Verifiers trait
// 2. use factory and composite pattern
// 3. add NoopVerifier for test
