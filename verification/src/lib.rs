mod block_verifier;
mod error;
mod header_verifier;
mod shared;
mod transaction_verifier;

#[cfg(test)]
mod tests;

pub use crate::block_verifier::{
    BlockVerifier, HeaderResolverWrapper, MerkleRootVerifier, TransactionsVerifier,
};
pub use crate::error::{Error, TransactionError};
pub use crate::header_verifier::{HeaderResolver, HeaderVerifier};
pub use crate::transaction_verifier::{
    InputVerifier, PoolTransactionVerifier, TransactionVerifier,
};

pub trait Verifier {
    type Target;
    fn verify(&self, target: &Self::Target) -> Result<(), Error>;
}
