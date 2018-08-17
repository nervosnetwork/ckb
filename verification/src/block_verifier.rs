use super::header_verifier::HeaderVerifier;
use super::{TransactionVerifier, Verifier};
use chain::chain::ChainProvider;
use core::block::IndexedBlock;
use core::transaction::{Capacity, CellInput};
use error::{CellbaseError, Error, TransactionError, UnclesError};
use fnv::FnvHashSet;
use merkle_root::merkle_root;
use pow_verifier::PowVerifier;
use rayon::iter::{IndexedParallelIterator, IntoParallelRefIterator, ParallelIterator};
use std::collections::HashSet;
use std::sync::Arc;

// -  merkle_root
// -  cellbase(uniqueness, index)
// -  witness
// -  empty
// -  size

//TODO: cellbase, witness
pub struct BlockVerifier<'a, C, P> {
    pub empty_transactions: EmptyTransactionsVerifier<'a>,
    pub duplicate_transactions: DuplicateTransactionsVerifier<'a>,
    pub cellbase: CellbaseTransactionsVerifier<'a, C>,
    pub merkle_root: MerkleRootVerifier<'a>,
    pub uncles: UnclesVerifier<'a, C, P>,
    pub transactions: TransactionsVerifier<'a, C>,
}

impl<'a, C, P> BlockVerifier<'a, C, P>
where
    C: ChainProvider,
    P: PowVerifier,
{
    pub fn new(block: &'a IndexedBlock, chain: &Arc<C>, pow: P) -> Self {
        BlockVerifier {
            empty_transactions: EmptyTransactionsVerifier::new(block),
            duplicate_transactions: DuplicateTransactionsVerifier::new(block),
            cellbase: CellbaseTransactionsVerifier::new(block, Arc::clone(chain)),
            merkle_root: MerkleRootVerifier::new(block),
            uncles: UnclesVerifier::new(block, Arc::clone(chain), pow),
            transactions: TransactionsVerifier::new(block, Arc::clone(chain)),
        }
    }
}

impl<'a, C, P> Verifier for BlockVerifier<'a, C, P>
where
    C: ChainProvider,
    P: PowVerifier,
{
    fn verify(&self) -> Result<(), Error> {
        self.empty_transactions.verify()?;
        self.duplicate_transactions.verify()?;
        self.cellbase.verify()?;
        self.merkle_root.verify()?;
        self.uncles.verify()?;
        self.transactions.verify()
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
            return Err(Error::Cellbase(CellbaseError::InvalidQuantity));
        }
        if cellbase_len == 1 && (!self.block.transactions[0].is_cellbase()) {
            return Err(Error::Cellbase(CellbaseError::InvalidPosition));
        }

        let cellbase_transaction = &self.block.transactions[0];
        if cellbase_transaction.inputs[0] != CellInput::new_cellbase_input(self.block.header.number)
        {
            return Err(Error::Cellbase(CellbaseError::InvalidInput));
        }
        let block_reward = self.chain.block_reward(self.block.header.raw.number);
        let mut fee = 0;
        for transaction in self.block.transactions.iter().skip(1) {
            fee += self.chain.calculate_transaction_fee(transaction)?;
        }
        let total_reward = block_reward + fee;
        let output_capacity: Capacity = cellbase_transaction
            .outputs
            .iter()
            .map(|output| output.capacity)
            .sum();
        if output_capacity > total_reward {
            Err(Error::Cellbase(CellbaseError::InvalidReward))
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

pub struct UnclesVerifier<'a, C, P> {
    block: &'a IndexedBlock,
    chain: Arc<C>,
    pow: P,
}

impl<'a, C, P> UnclesVerifier<'a, C, P>
where
    C: ChainProvider,
    P: PowVerifier,
{
    pub fn new(block: &'a IndexedBlock, chain: Arc<C>, pow: P) -> Self {
        UnclesVerifier { block, chain, pow }
    }

    // -  uncles_hash
    // -  uncles_len
    // -  depth
    // -  uncle cellbase_id
    // -  uncle not in main chain
    // -  uncle parent
    // -  uncle duplicate
    // -  header verifier
    pub fn verify(&self) -> Result<(), Error> {
        let actual_uncles_hash = self.block.cal_uncles_hash();
        if actual_uncles_hash != self.block.header.uncles_hash {
            return Err(Error::Uncles(UnclesError::InvalidHash {
                expected: self.block.header.uncles_hash,
                actual: actual_uncles_hash,
            }));
        }

        if self.block.uncles().is_empty() {
            return Ok(());
        }

        let uncles_len = self.block.uncles().len();
        let max_uncles_len = self.chain.consensus().max_uncles_len();
        if uncles_len > max_uncles_len {
            return Err(Error::Uncles(UnclesError::OverLength {
                max: max_uncles_len,
                actual: uncles_len,
            }));
        }

        let max_uncles_age = self.chain.consensus().max_uncles_age();
        for uncle in self.block.uncles() {
            let depth = if self.block.number() > uncle.number() {
                self.block.number() - uncle.number()
            } else {
                0
            };

            if depth > max_uncles_age as u64 || depth < 1 {
                return Err(Error::Uncles(UnclesError::InvalidDepth {
                    min: self.block.number() - max_uncles_age as u64,
                    max: self.block.number() - 1,
                    actual: uncle.number(),
                }));
            }
        }

        // cB
        // cB.p^0       1 depth, valid uncle
        // cB.p^1   ---/  2
        // cB.p^2   -----/  3
        // cB.p^3   -------/  4
        // cB.p^4   ---------/  5
        // cB.p^5   -----------/  6
        // cB.p^6   -------------/
        // cB.p^7
        let mut ancestors = FnvHashSet::default();
        let mut excluded = FnvHashSet::default();
        let mut included = FnvHashSet::default();

        excluded.insert(self.block.hash());
        let mut block_hash = self.block.header.parent_hash;
        excluded.insert(block_hash);
        for _ in 0..max_uncles_age {
            if let Some(block) = self.chain.block(&block_hash) {
                ancestors.insert(block.header.parent_hash);
                excluded.insert(block.header.parent_hash);
                for uncle in block.uncles() {
                    excluded.insert(uncle.header.hash());
                }

                block_hash = block.header.parent_hash;
            } else {
                break;
            }
        }

        for uncle in self.block.uncles() {
            if uncle.header.cellbase_id != uncle.cellbase.hash() {
                return Err(Error::Uncles(UnclesError::InvalidCellbase));
            }

            let uncle_hash = uncle.header.hash();
            let uncle_parent_hash = uncle.header.parent_hash;

            if included.contains(&uncle_hash) {
                return Err(Error::Uncles(UnclesError::Duplicate(uncle_hash)));
            }

            if excluded.contains(&uncle_hash) {
                return Err(Error::Uncles(UnclesError::InvalidInclude(uncle_hash)));
            }

            if !ancestors.contains(&uncle_parent_hash) {
                return Err(Error::Uncles(UnclesError::InvalidParent(uncle_parent_hash)));
            }

            let uncle_parent = self
                .chain
                .block_header(&uncle_parent_hash)
                .expect("parent verified");

            HeaderVerifier::new(
                &uncle_parent,
                &uncle.header.clone().into(),
                self.pow.clone(),
            ).verify()?;

            included.insert(uncle_hash);
        }

        Ok(())
    }
}

pub struct TransactionsVerifier<'a, C> {
    block: &'a IndexedBlock,
    chain: Arc<C>,
}

impl<'a, C> TransactionsVerifier<'a, C>
where
    C: ChainProvider,
{
    pub fn new(block: &'a IndexedBlock, chain: Arc<C>) -> Self {
        TransactionsVerifier { block, chain }
    }

    pub fn verify(&self) -> Result<(), Error> {
        let parent_hash = self.block.header.parent_hash;
        // make verifiers orthogonal
        // skip first tx, assume the first is cellbase, other verifier will verify cellbase
        let err: Vec<(usize, TransactionError)> = self
            .block
            .transactions
            .par_iter()
            .skip(1)
            .map(|x| self.chain.resolve_transaction_at(x, &parent_hash))
            .enumerate()
            .filter_map(|(index, tx)| {
                TransactionVerifier::new(&tx)
                    .verify()
                    .err()
                    .map(|e| (index, e))
            })
            .collect();
        if err.is_empty() {
            Ok(())
        } else {
            Err(Error::Transaction(err))
        }
    }
}
