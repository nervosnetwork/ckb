use super::header_verifier::HeaderResolver;
use super::{TransactionVerifier, Verifier};
use bigint::{H256, U256};
use chain::chain::ChainProvider;
use chain::PowEngine;
use core::block::IndexedBlock;
use core::cell::{CellProvider, CellState};
use core::header::IndexedHeader;
use core::transaction::{Capacity, CellInput, OutPoint};
use error::{CellbaseError, CommitError, Error, TransactionError, UnclesError};
use fnv::{FnvHashMap, FnvHashSet};
use merkle_root::merkle_root;
use rayon::iter::{IndexedParallelIterator, IntoParallelRefIterator, ParallelIterator};
use std::collections::HashSet;
use std::sync::Arc;

//TODO: cellbase, witness
pub struct BlockVerifier<'a, C, P> {
    // Verify if the committed transactions is empty
    pub empty: EmptyVerifier<'a>,
    // Verify if the committed and proposed transactions contains duplicate
    pub duplicate: DuplicateVerifier<'a>,
    // Verify the cellbase
    pub cellbase: CellbaseVerifier<'a, C>,
    // Verify the the committed and proposed transactions merkle root match header's announce
    pub merkle_root: MerkleRootVerifier<'a>,
    // Verify the the uncle
    pub uncles: UnclesVerifier<'a, C, P>,
    // Verify the the propose-then-commit consensus rule
    pub commit: CommitVerifier<'a, C>,
    // Verify all the committed transactions through TransactionVerifier
    pub transactions: TransactionsVerifier<'a, C>,
}

impl<'a, C, P> BlockVerifier<'a, C, P>
where
    C: ChainProvider,
    P: PowEngine,
{
    pub fn new(block: &'a IndexedBlock, chain: &Arc<C>, pow: &Arc<P>) -> Self {
        BlockVerifier {
            // TODO change all new fn's chain to reference
            empty: EmptyVerifier::new(block),
            duplicate: DuplicateVerifier::new(block),
            cellbase: CellbaseVerifier::new(block, Arc::clone(chain)),
            merkle_root: MerkleRootVerifier::new(block),
            uncles: UnclesVerifier::new(block, chain, pow),
            commit: CommitVerifier::new(block, Arc::clone(chain)),
            transactions: TransactionsVerifier::new(block, Arc::clone(chain)),
        }
    }
}

impl<'a, C, P> Verifier for BlockVerifier<'a, C, P>
where
    C: ChainProvider,
    P: PowEngine,
{
    fn verify(&self) -> Result<(), Error> {
        // EmptyTransactionsVerifier must be executed first. Other verifiers may depend on the
        // assumption that the transactions list is not empty.
        self.empty.verify()?;
        self.duplicate.verify()?;
        self.cellbase.verify()?;
        self.merkle_root.verify()?;
        self.commit.verify()?;
        self.uncles.verify()?;
        self.transactions.verify()
    }
}

pub struct CellbaseVerifier<'a, C> {
    block: &'a IndexedBlock,
    chain: Arc<C>,
}

impl<'a, C> CellbaseVerifier<'a, C>
where
    C: ChainProvider,
{
    pub fn new(block: &'a IndexedBlock, chain: Arc<C>) -> Self {
        CellbaseVerifier { block, chain }
    }

    pub fn verify(&self) -> Result<(), Error> {
        if self.block.commit_transactions.is_empty() {
            return Ok(());
        }
        let cellbase_len = self
            .block
            .commit_transactions
            .iter()
            .filter(|tx| tx.is_cellbase())
            .count();

        // empty checked, block must contain cellbase
        if cellbase_len != 1 {
            return Err(Error::Cellbase(CellbaseError::InvalidQuantity));
        }

        if !self.block.commit_transactions[0].is_cellbase() {
            return Err(Error::Cellbase(CellbaseError::InvalidPosition));
        }

        let cellbase_transaction = &self.block.commit_transactions[0];
        if cellbase_transaction.inputs[0] != CellInput::new_cellbase_input(self.block.header.number)
        {
            return Err(Error::Cellbase(CellbaseError::InvalidInput));
        }
        let block_reward = self.chain.block_reward(self.block.header.raw.number);
        let mut fee = 0;
        for transaction in self.block.commit_transactions.iter().skip(1) {
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

pub struct EmptyVerifier<'a> {
    block: &'a IndexedBlock,
}

impl<'a> EmptyVerifier<'a> {
    pub fn new(block: &'a IndexedBlock) -> Self {
        EmptyVerifier { block }
    }

    pub fn verify(&self) -> Result<(), Error> {
        if self.block.commit_transactions.is_empty() {
            Err(Error::CommitTransactionsEmpty)
        } else {
            Ok(())
        }
    }
}

pub struct DuplicateVerifier<'a> {
    block: &'a IndexedBlock,
}

impl<'a> DuplicateVerifier<'a> {
    pub fn new(block: &'a IndexedBlock) -> Self {
        DuplicateVerifier { block }
    }

    pub fn verify(&self) -> Result<(), Error> {
        let mut seen = HashSet::with_capacity(self.block.commit_transactions.len());
        if !self
            .block
            .commit_transactions
            .iter()
            .all(|tx| seen.insert(tx.hash()))
        {
            return Err(Error::CommitTransactionDuplicate);
        }

        let mut seen = HashSet::with_capacity(self.block.proposal_transactions.len());
        if !self
            .block
            .proposal_transactions
            .iter()
            .all(|id| seen.insert(id))
        {
            return Err(Error::ProposalTransactionDuplicate);
        }
        Ok(())
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
        let commits = self
            .block
            .commit_transactions
            .iter()
            .map(|tx| tx.hash())
            .collect::<Vec<_>>();

        if self.block.header.txs_commit != merkle_root(&commits[..]) {
            return Err(Error::CommitTransactionsRoot);
        }

        let proposals = self
            .block
            .proposal_transactions
            .iter()
            .map(|id| id.hash())
            .collect::<Vec<_>>();

        if self.block.header.txs_proposal != merkle_root(&proposals[..]) {
            return Err(Error::ProposalTransactionsRoot);
        }

        Ok(())
    }
}

pub struct HeaderResolverWrapper<'a, C> {
    chain: Arc<C>,
    header: &'a IndexedHeader,
    parent: Option<IndexedHeader>,
}

impl<'a, C> HeaderResolverWrapper<'a, C>
where
    C: ChainProvider,
{
    pub fn new(header: &'a IndexedHeader, chain: &Arc<C>) -> Self {
        let parent = chain.block_header(&header.parent_hash);
        HeaderResolverWrapper {
            parent,
            header,
            chain: Arc::clone(chain),
        }
    }
}

impl<'a, C> HeaderResolver for HeaderResolverWrapper<'a, C>
where
    C: ChainProvider,
{
    fn header(&self) -> &IndexedHeader {
        self.header
    }

    fn parent(&self) -> Option<&IndexedHeader> {
        self.parent.as_ref()
    }

    fn calculate_difficulty(&self) -> Option<U256> {
        self.parent()
            .and_then(|parent| self.chain.calculate_difficulty(parent))
    }
}

// TODO redo uncle verifier, check uncle proposal duplicate
pub struct UnclesVerifier<'a, C, P> {
    block: &'a IndexedBlock,
    chain: Arc<C>,
    pow: Arc<P>,
}

impl<'a, C, P> UnclesVerifier<'a, C, P>
where
    C: ChainProvider,
    P: PowEngine,
{
    pub fn new(block: &'a IndexedBlock, chain: &Arc<C>, pow: &Arc<P>) -> Self {
        UnclesVerifier {
            block,
            chain: Arc::clone(chain),
            pow: Arc::clone(pow),
        }
    }

    // -  uncles_hash
    // -  uncles_len
    // -  depth
    // -  uncle cellbase_id
    // -  uncle not in main chain
    // -  uncle duplicate
    pub fn verify(&self) -> Result<(), Error> {
        // verify uncles_hash
        let actual_uncles_hash = self.block.cal_uncles_hash();
        if actual_uncles_hash != self.block.header.uncles_hash {
            return Err(Error::Uncles(UnclesError::InvalidHash {
                expected: self.block.header.uncles_hash,
                actual: actual_uncles_hash,
            }));
        }
        // if block.uncles is empty, return
        if self.block.uncles().is_empty() {
            return Ok(());
        }

        // verify uncles lenght =< max_uncles_len
        let uncles_len = self.block.uncles().len();
        let max_uncles_len = self.chain.consensus().max_uncles_len();
        if uncles_len > max_uncles_len {
            return Err(Error::Uncles(UnclesError::OverLength {
                max: max_uncles_len,
                actual: uncles_len,
            }));
        }

        // verify uncles age
        let max_uncles_age = self.chain.consensus().max_uncles_age();
        for uncle in self.block.uncles() {
            let depth = self.block.number().saturating_sub(uncle.number());

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
        // verify uncles is not included in main chain
        // TODO: cache context
        let mut excluded = FnvHashSet::default();
        let mut included = FnvHashSet::default();
        excluded.insert(self.block.hash());
        let mut block_hash = self.block.header.parent_hash;
        excluded.insert(block_hash);
        for _ in 0..max_uncles_age {
            if let Some(block) = self.chain.block(&block_hash) {
                excluded.insert(block.header.parent_hash);
                for uncle in block.uncles() {
                    excluded.insert(uncle.header.hash());
                }

                block_hash = block.header.parent_hash;
            } else {
                break;
            }
        }

        let block_difficulty_epoch =
            self.block.number() / self.chain.consensus().difficulty_adjustment_interval();

        for uncle in self.block.uncles() {
            let uncle_difficulty_epoch =
                uncle.number() / self.chain.consensus().difficulty_adjustment_interval();

            if uncle.header.difficulty != self.block.header.difficulty {
                return Err(Error::Uncles(UnclesError::InvalidDifficulty));
            }

            if block_difficulty_epoch != uncle_difficulty_epoch {
                return Err(Error::Uncles(UnclesError::InvalidDifficultyEpoch));
            }

            if uncle.header.cellbase_id != uncle.cellbase.hash() {
                return Err(Error::Uncles(UnclesError::InvalidCellbase));
            }

            let uncle_header: IndexedHeader = uncle.header.clone().into();

            let uncle_hash = uncle_header.hash();
            if included.contains(&uncle_hash) {
                return Err(Error::Uncles(UnclesError::Duplicate(uncle_hash)));
            }

            if excluded.contains(&uncle_hash) {
                return Err(Error::Uncles(UnclesError::InvalidInclude(uncle_hash)));
            }

            let proposals = uncle
                .proposal_transactions
                .iter()
                .map(|id| id.hash())
                .collect::<Vec<_>>();

            if uncle_header.txs_proposal != merkle_root(&proposals[..]) {
                return Err(Error::Uncles(UnclesError::ProposalTransactionsRoot));
            }

            let mut seen = HashSet::with_capacity(uncle.proposal_transactions.len());
            if !uncle.proposal_transactions.iter().all(|id| seen.insert(id)) {
                return Err(Error::Uncles(UnclesError::ProposalTransactionDuplicate));
            }

            if !self.pow.verify_header(&uncle_header) {
                return Err(Error::Uncles(UnclesError::InvalidProof));
            }

            included.insert(uncle_hash);
        }

        Ok(())
    }
}

pub struct TransactionsVerifier<'a, C> {
    block: &'a IndexedBlock,
    output_indexs: FnvHashMap<H256, usize>,
    chain: Arc<C>,
}

impl<'a, C> CellProvider for TransactionsVerifier<'a, C>
where
    C: ChainProvider,
{
    fn cell(&self, _o: &OutPoint) -> CellState {
        unreachable!()
    }

    fn cell_at(&self, o: &OutPoint, parent: &H256) -> CellState {
        if let Some(i) = self.output_indexs.get(&o.hash) {
            match self.block.commit_transactions[*i]
                .outputs
                .get(o.index as usize)
            {
                Some(x) => CellState::Head(x.clone()),
                None => CellState::Unknown,
            }
        } else {
            let chain_cell_state = self.chain.cell_at(o, parent);
            if chain_cell_state.is_head() {
                CellState::Head(chain_cell_state.take_head().expect("state checked"))
            } else if chain_cell_state.is_tail() {
                CellState::Tail
            } else {
                CellState::Unknown
            }
        }
    }
}

impl<'a, C> TransactionsVerifier<'a, C>
where
    C: ChainProvider,
{
    pub fn new(block: &'a IndexedBlock, chain: Arc<C>) -> Self {
        let mut output_indexs = FnvHashMap::default();

        for (i, tx) in block.commit_transactions.iter().enumerate() {
            output_indexs.insert(tx.hash(), i);
        }

        TransactionsVerifier {
            block,
            output_indexs,
            chain,
        }
    }

    pub fn verify(&self) -> Result<(), Error> {
        let parent_hash = self.block.header.parent_hash;
        // make verifiers orthogonal
        // skip first tx, assume the first is cellbase, other verifier will verify cellbase
        let err: Vec<(usize, TransactionError)> = self
            .block
            .commit_transactions
            .par_iter()
            .skip(1)
            .map(|x| self.resolve_transaction_at(x, &parent_hash))
            .enumerate()
            .filter_map(|(index, tx)| {
                TransactionVerifier::new(&tx)
                    .verify()
                    .err()
                    .map(|e| (index, e))
            }).collect();
        if err.is_empty() {
            Ok(())
        } else {
            Err(Error::Transactions(err))
        }
    }
}

pub struct CommitVerifier<'a, C> {
    block: &'a IndexedBlock,
    chain: Arc<C>,
}

impl<'a, C> CommitVerifier<'a, C>
where
    C: ChainProvider,
{
    pub fn new(block: &'a IndexedBlock, chain: Arc<C>) -> Self {
        CommitVerifier { block, chain }
    }

    pub fn verify(&self) -> Result<(), Error> {
        let block_number = self.block.number();
        let t_prop = self.chain.consensus().transaction_propagation_time;
        let mut walk = self.chain.consensus().transaction_propagation_timeout;
        let start = block_number.saturating_sub(t_prop);

        if start < 1 {
            return Ok(());
        }

        let mut block_hash = self.block.header.parent_hash;
        let mut proposal_txs_ids = FnvHashSet::default();

        while walk > 0 {
            let block = self
                .chain
                .block(&block_hash)
                .ok_or_else(|| Error::Commit(CommitError::AncestorNotFound))?;
            if block.is_genesis() {
                break;
            }
            proposal_txs_ids.extend(
                block.proposal_transactions().iter().chain(
                    block
                        .uncles()
                        .iter()
                        .flat_map(|uncle| uncle.proposal_transactions()),
                ),
            );

            block_hash = block.header.parent_hash;
            walk -= 1;
        }

        let commited_ids: FnvHashSet<_> = self
            .block
            .commit_transactions()
            .par_iter()
            .skip(1)
            .map(|tx| tx.proposal_short_id())
            .collect();

        let difference: Vec<_> = commited_ids.difference(&proposal_txs_ids).collect();

        if !difference.is_empty() {
            return Err(Error::Commit(CommitError::Invalid));
        }
        Ok(())
    }
}
