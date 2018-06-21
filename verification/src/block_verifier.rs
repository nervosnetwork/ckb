use core::block::Block;
use error::Error;
use merkle_root::merkle_root;
use std::collections::HashSet;

// -  merkle_root
// -  cellbase(uniqueness, index)
// -  witness
// -  empty
// -  size

//TODO: cellbase, witness
pub struct BlockVerifier<'a> {
    pub empty_transactions: EmptyTransactionsVerifier<'a>,
    pub duplicate_transactions: DuplicateTransactionsVerifier<'a>,
    pub merkle_root: MerkleRootVerifier<'a>,
}

impl<'a> BlockVerifier<'a> {
    pub fn new(block: &'a Block) -> Self {
        BlockVerifier {
            empty_transactions: EmptyTransactionsVerifier::new(block),
            duplicate_transactions: DuplicateTransactionsVerifier::new(block),
            merkle_root: MerkleRootVerifier::new(block),
        }
    }

    pub fn verify(&self) -> Result<(), Error> {
        self.empty_transactions.verify()?;
        self.duplicate_transactions.verify()?;
        self.merkle_root.verify()?;
        Ok(())
    }
}

pub struct EmptyTransactionsVerifier<'a> {
    block: &'a Block,
}

impl<'a> EmptyTransactionsVerifier<'a> {
    pub fn new(block: &'a Block) -> Self {
        EmptyTransactionsVerifier { block }
    }

    pub fn verify(&self) -> Result<(), Error> {
        if self.block.transactions.is_empty() {
            Err(Error::EmptyTransactions)
        } else {
            Ok(())
        }
    }
}

pub struct DuplicateTransactionsVerifier<'a> {
    block: &'a Block,
}

impl<'a> DuplicateTransactionsVerifier<'a> {
    pub fn new(block: &'a Block) -> Self {
        DuplicateTransactionsVerifier { block }
    }

    pub fn verify(&self) -> Result<(), Error> {
        let hashes = self
            .block
            .transactions
            .iter()
            .map(|tx| tx.hash())
            .collect::<HashSet<_>>();
        if hashes.len() == self.block.transactions.len() {
            Ok(())
        } else {
            Err(Error::DuplicateTransactions)
        }
    }
}

pub struct MerkleRootVerifier<'a> {
    block: &'a Block,
}

impl<'a> MerkleRootVerifier<'a> {
    pub fn new(block: &'a Block) -> Self {
        MerkleRootVerifier { block }
    }

    pub fn verify(&self) -> Result<(), Error> {
        let hashes = self
            .block
            .transactions
            .iter()
            .map(|tx| tx.hash())
            .collect::<Vec<_>>();

        if self.block.header.txs_commit == merkle_root(&hashes[..]) {
            Ok(())
        } else {
            Err(Error::TransactionsRoot)
        }
    }
}
