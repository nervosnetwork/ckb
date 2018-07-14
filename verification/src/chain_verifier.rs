use super::block_verifier::BlockVerifier;
use super::header_verifier::HeaderVerifier;
use super::pow_verifier::PowVerifier;
use super::transaction_verifier::TransactionVerifier;
use super::Verifier;
use core::block::Block;
use core::header::Header;
use error::{Error, TransactionError};
use rayon::prelude::*;

pub struct ChainVerifier<'a, T> {
    pub block: BlockVerifier<'a>,
    pub header: HeaderVerifier<'a, T>,
    pub transactions: Vec<TransactionVerifier<'a>>,
}

impl<'a, T> Verifier for ChainVerifier<'a, T>
where
    T: PowVerifier,
{
    fn verify(&self) -> Result<(), Error> {
        self.block.verify()?;
        self.header.verify()?;
        self.verify_transactions()?;
        Ok(())
    }
}

impl<'a, T> ChainVerifier<'a, T>
where
    T: PowVerifier,
{
    pub fn new(parent_header: &'a Header, block: &'a Block, pow_verifier: T) -> Self {
        ChainVerifier {
            block: BlockVerifier::new(block),
            header: HeaderVerifier::new(parent_header, &block.header, pow_verifier),
            transactions: block
                .transactions
                .iter()
                .map(TransactionVerifier::new)
                .collect(),
        }
    }

    fn verify_transactions(&self) -> Result<(), Error> {
        let err: Vec<(usize, TransactionError)> = self
            .transactions
            .par_iter()
            .enumerate()
            .filter_map(|(index, tx)| tx.verify().err().map(|e| (index, e)))
            .collect();
        if err.is_empty() {
            Ok(())
        } else {
            Err(Error::Transaction(err))
        }
    }
}
