use super::{TransactionVerifier, Verifier};
use chain::chain::ChainProvider;
use core::block::IndexedBlock;
use error::{Error, TransactionError};
use merkle_root::merkle_root;
use rayon::iter::{IndexedParallelIterator, IntoParallelRefIterator, ParallelIterator};
use std::collections::HashSet;
use std::sync::Arc;

// -  merkle_root
// -  cellbase(uniqueness, index)
// -  witness
// -  empty
// -  size

//TODO: cellbase, witness
pub struct BlockVerifier<'a, C> {
    pub empty_transactions: EmptyTransactionsVerifier<'a>,
    pub duplicate_transactions: DuplicateTransactionsVerifier<'a>,
    pub cellbase: CellbaseTransactionsVerifier<'a, C>,
    pub merkle_root: MerkleRootVerifier<'a>,
    pub transactions: Vec<TransactionVerifier<'a>>,
}

impl<'a, C> BlockVerifier<'a, C>
where
    C: ChainProvider,
{
    pub fn new(block: &'a IndexedBlock, chain: &Arc<C>) -> Self {
        BlockVerifier {
            empty_transactions: EmptyTransactionsVerifier::new(block),
            duplicate_transactions: DuplicateTransactionsVerifier::new(block),
            cellbase: CellbaseTransactionsVerifier::new(block, Arc::clone(chain)),
            merkle_root: MerkleRootVerifier::new(block),
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

impl<'a, C> Verifier for BlockVerifier<'a, C>
where
    C: ChainProvider,
{
    fn verify(&self) -> Result<(), Error> {
        self.empty_transactions.verify()?;
        self.duplicate_transactions.verify()?;
        self.cellbase.verify()?;
        self.merkle_root.verify()?;
        self.verify_transactions()
    }
}

pub struct CellbaseTransactionsVerifier<'a, C> {
    block: &'a IndexedBlock,
    chain: Arc<C>,
}

impl<'a, C> CellbaseTransactionsVerifier<'a, C>
where
    C: ChainProvider,
{
    pub fn new(block: &'a IndexedBlock, chain: Arc<C>) -> Self {
        CellbaseTransactionsVerifier { block, chain }
    }

    pub fn verify(&self) -> Result<(), Error> {
        if self.block.transactions.is_empty() {
            return Ok(());
        }
        let cellbase_len = self
            .block
            .transactions
            .iter()
            .filter(|tx| tx.is_cellbase())
            .count();
        if cellbase_len == 0 {
            return Ok(());
        }
        if cellbase_len > 1 {
            return Err(Error::MultipleCellbase);
        }
        if cellbase_len == 1 && (!self.block.transactions[0].is_cellbase()) {
            return Err(Error::CellbaseNotAtFirst);
        }

        let cellbase_transaction = &self.block.transactions[0];
        let block_reward = self.chain.block_reward(self.block.header.raw.number);
        let mut fee = 0;
        for transaction in self.block.transactions.iter().skip(1) {
            fee += self.chain.calculate_transaction_fee(transaction)?;
        }
        let total_reward = block_reward + fee;
        let output_capacity: u32 = cellbase_transaction
            .outputs
            .iter()
            .map(|output| output.capacity)
            .sum();
        if output_capacity > total_reward {
            Err(Error::Transaction(vec![(
                0,
                TransactionError::InvalidCapacity,
            )]))
        } else {
            Ok(())
        }
    }
}

pub struct EmptyTransactionsVerifier<'a> {
    block: &'a IndexedBlock,
}

impl<'a> EmptyTransactionsVerifier<'a> {
    pub fn new(block: &'a IndexedBlock) -> Self {
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
    block: &'a IndexedBlock,
}

impl<'a> DuplicateTransactionsVerifier<'a> {
    pub fn new(block: &'a IndexedBlock) -> Self {
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
    block: &'a IndexedBlock,
}

impl<'a> MerkleRootVerifier<'a> {
    pub fn new(block: &'a IndexedBlock) -> Self {
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
