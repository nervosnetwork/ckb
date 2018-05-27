use super::block_verifier::BlockVerifier;
use super::header_verifier::HeaderVerifier;
use super::transaction_verifier::TransactionVerifier;
use super::Verifier;
use core::block::Block;
use core::header::Header;
use error::{Error, TransactionError};
use ethash::Ethash;
use rayon::prelude::*;
use std::sync::Arc;

pub struct ChainVerifier<'a> {
    pub block: BlockVerifier<'a>,
    pub header: HeaderVerifier<'a>,
    pub transactions: Vec<TransactionVerifier<'a>>,
}

impl<'a> Verifier for ChainVerifier<'a> {
    fn verify(&self) -> Result<(), Error> {
        self.block.verify()?;
        self.header.verify()?;
        self.verify_transactions()?;
        Ok(())
    }
}

impl<'a> ChainVerifier<'a> {
    pub fn new(parent_header: &'a Header, block: &'a Block, ethash: &Arc<Ethash>) -> Self {
        ChainVerifier {
            block: BlockVerifier::new(block),
            header: HeaderVerifier::new(parent_header, &block.header, ethash),
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
