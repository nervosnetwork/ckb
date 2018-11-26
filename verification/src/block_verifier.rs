use super::header_verifier::HeaderResolver;
use super::{TransactionVerifier, Verifier};
use bigint::{H256, U256};
use ckb_core::block::Block;
use ckb_core::cell::{CellProvider, CellStatus};
use ckb_core::header::Header;
use ckb_core::transaction::{Capacity, CellInput, OutPoint};
use ckb_shared::shared::ChainProvider;
use error::TransactionError;
use error::{CellbaseError, CommitError, Error, UnclesError};
use fnv::{FnvHashMap, FnvHashSet};
use merkle_root::merkle_root;
use rayon::iter::{IndexedParallelIterator, IntoParallelRefIterator, ParallelIterator};
use std::collections::HashSet;

//TODO: cellbase, witness
pub struct BlockVerifier<P> {
    // Verify if the committed transactions is empty
    empty: EmptyVerifier,
    // Verify if the committed and proposed transactions contains duplicate
    duplicate: DuplicateVerifier,
    // Verify the cellbase
    cellbase: CellbaseVerifier<P>,
    // Verify the the committed and proposed transactions merkle root match header's announce
    merkle_root: MerkleRootVerifier,
    // Verify the the uncle
    uncles: UnclesVerifier<P>,
    // Verify the the propose-then-commit consensus rule
    commit: CommitVerifier<P>,
    // Verify all the committed transactions through TransactionVerifier
    transactions: TransactionsVerifier<P>,
}

impl<P: ChainProvider + CellProvider + Clone> ::std::clone::Clone for BlockVerifier<P> {
    fn clone(&self) -> Self {
        BlockVerifier {
            empty: self.empty.clone(),
            duplicate: self.duplicate.clone(),
            cellbase: self.cellbase.clone(),
            merkle_root: self.merkle_root.clone(),
            uncles: self.uncles.clone(),
            commit: self.commit.clone(),
            transactions: self.transactions.clone(),
        }
    }
}

impl<P> BlockVerifier<P>
where
    P: ChainProvider + CellProvider + Clone + 'static,
{
    pub fn new(provider: P) -> Self {
        BlockVerifier {
            // TODO change all new fn's chain to reference
            empty: EmptyVerifier::new(),
            duplicate: DuplicateVerifier::new(),
            cellbase: CellbaseVerifier::new(provider.clone()),
            merkle_root: MerkleRootVerifier::new(),
            uncles: UnclesVerifier::new(provider.clone()),
            commit: CommitVerifier::new(provider.clone()),
            transactions: TransactionsVerifier::new(provider),
        }
    }
}

impl<P: ChainProvider + CellProvider + Clone> Verifier for BlockVerifier<P> {
    type Target = Block;

    fn verify(&self, target: &Block) -> Result<(), Error> {
        // EmptyTransactionsVerifier must be executed first. Other verifiers may depend on the
        // assumption that the transactions list is not empty.
        self.empty.verify(target)?;
        self.duplicate.verify(target)?;
        self.cellbase.verify(target)?;
        self.merkle_root.verify(target)?;
        self.commit.verify(target)?;
        self.uncles.verify(target)?;
        self.transactions.verify(target)
    }
}

#[derive(Clone)]
pub struct CellbaseVerifier<CP> {
    provider: CP,
}

impl<CP: ChainProvider + Clone> CellbaseVerifier<CP> {
    pub fn new(provider: CP) -> Self {
        CellbaseVerifier { provider }
    }

    pub fn verify(&self, block: &Block) -> Result<(), Error> {
        if block.commit_transactions().is_empty() {
            return Ok(());
        }
        let cellbase_len = block
            .commit_transactions()
            .iter()
            .filter(|tx| tx.is_cellbase())
            .count();

        // empty checked, block must contain cellbase
        if cellbase_len != 1 {
            return Err(Error::Cellbase(CellbaseError::InvalidQuantity));
        }

        if !block.commit_transactions()[0].is_cellbase() {
            return Err(Error::Cellbase(CellbaseError::InvalidPosition));
        }

        let cellbase_transaction = &block.commit_transactions()[0];
        if cellbase_transaction.inputs()[0]
            != CellInput::new_cellbase_input(block.header().number())
        {
            return Err(Error::Cellbase(CellbaseError::InvalidInput));
        }
        let block_reward = self.provider.block_reward(block.header().number());
        let mut fee = 0;
        for transaction in block.commit_transactions().iter().skip(1) {
            fee += self.provider.calculate_transaction_fee(transaction)?;
        }
        let total_reward = block_reward + fee;
        let output_capacity: Capacity = cellbase_transaction
            .outputs()
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

#[derive(Clone)]
pub struct EmptyVerifier {}

impl EmptyVerifier {
    pub fn new() -> Self {
        EmptyVerifier {}
    }

    pub fn verify(&self, block: &Block) -> Result<(), Error> {
        if block.commit_transactions().is_empty() {
            Err(Error::CommitTransactionsEmpty)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone)]
pub struct DuplicateVerifier {}

impl DuplicateVerifier {
    pub fn new() -> Self {
        DuplicateVerifier {}
    }

    pub fn verify(&self, block: &Block) -> Result<(), Error> {
        let mut seen = HashSet::with_capacity(block.commit_transactions().len());
        if !block
            .commit_transactions()
            .iter()
            .all(|tx| seen.insert(tx.hash()))
        {
            return Err(Error::CommitTransactionDuplicate);
        }

        let mut seen = HashSet::with_capacity(block.proposal_transactions().len());
        if !block
            .proposal_transactions()
            .iter()
            .all(|id| seen.insert(id))
        {
            return Err(Error::ProposalTransactionDuplicate);
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct MerkleRootVerifier {}

impl MerkleRootVerifier {
    pub fn new() -> Self {
        MerkleRootVerifier {}
    }

    pub fn verify(&self, block: &Block) -> Result<(), Error> {
        let commits = block
            .commit_transactions()
            .iter()
            .map(|tx| tx.hash())
            .collect::<Vec<_>>();

        if block.header().txs_commit() != merkle_root(&commits[..]) {
            return Err(Error::CommitTransactionsRoot);
        }

        let proposals = block
            .proposal_transactions()
            .iter()
            .map(|id| id.hash())
            .collect::<Vec<_>>();

        if block.header().txs_proposal() != merkle_root(&proposals[..]) {
            return Err(Error::ProposalTransactionsRoot);
        }

        Ok(())
    }
}

pub struct HeaderResolverWrapper<'a, CP> {
    provider: CP,
    header: &'a Header,
    parent: Option<Header>,
}

impl<'a, CP: ChainProvider> HeaderResolverWrapper<'a, CP> {
    pub fn new(header: &'a Header, provider: CP) -> Self {
        let parent = provider.block_header(&header.parent_hash());
        HeaderResolverWrapper {
            parent,
            header,
            provider,
        }
    }
}

impl<'a, CP: ChainProvider> HeaderResolver for HeaderResolverWrapper<'a, CP> {
    fn header(&self) -> &Header {
        self.header
    }

    fn parent(&self) -> Option<&Header> {
        self.parent.as_ref()
    }

    fn calculate_difficulty(&self) -> Option<U256> {
        self.parent()
            .and_then(|parent| self.provider.calculate_difficulty(parent))
    }
}

// TODO redo uncle verifier, check uncle proposal duplicate
pub struct UnclesVerifier<CP> {
    provider: CP,
}

impl<CP: ChainProvider + Clone> ::std::clone::Clone for UnclesVerifier<CP> {
    fn clone(&self) -> Self {
        UnclesVerifier {
            provider: self.provider.clone(),
        }
    }
}

impl<CP: ChainProvider + Clone> UnclesVerifier<CP> {
    pub fn new(provider: CP) -> Self {
        UnclesVerifier { provider }
    }

    // -  uncles_hash
    // -  uncles_len
    // -  depth
    // -  uncle cellbase_id
    // -  uncle not in main chain
    // -  uncle duplicate
    pub fn verify(&self, block: &Block) -> Result<(), Error> {
        // verify uncles_count
        let uncles_count = block.uncles().len() as u32;
        if uncles_count != block.header().uncles_count() {
            return Err(Error::Uncles(UnclesError::MissMatchCount {
                expected: block.header().uncles_count(),
                actual: uncles_count,
            }));
        }

        // verify uncles_hash
        let actual_uncles_hash = block.cal_uncles_hash();
        if actual_uncles_hash != block.header().uncles_hash() {
            return Err(Error::Uncles(UnclesError::InvalidHash {
                expected: block.header().uncles_hash(),
                actual: actual_uncles_hash,
            }));
        }
        // if block.uncles is empty, return
        if block.uncles().is_empty() {
            return Ok(());
        }

        // verify uncles lenght =< max_uncles_len
        let uncles_len = block.uncles().len();
        let max_uncles_len = self.provider.consensus().max_uncles_len();
        if uncles_len > max_uncles_len {
            return Err(Error::Uncles(UnclesError::OverCount {
                max: max_uncles_len,
                actual: uncles_len,
            }));
        }

        // verify uncles age
        let max_uncles_age = self.provider.consensus().max_uncles_age();
        for uncle in block.uncles() {
            let depth = block.header().number().saturating_sub(uncle.number());

            if depth > max_uncles_age as u64 || depth < 1 {
                return Err(Error::Uncles(UnclesError::InvalidDepth {
                    min: block.header().number() - max_uncles_age as u64,
                    max: block.header().number() - 1,
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
        excluded.insert(block.header().hash());
        let mut block_hash = block.header().parent_hash();
        excluded.insert(block_hash);
        for _ in 0..max_uncles_age {
            if let Some(block) = self.provider.block(&block_hash) {
                excluded.insert(block.header().parent_hash());
                for uncle in block.uncles() {
                    excluded.insert(uncle.header.hash());
                }

                block_hash = block.header().parent_hash();
            } else {
                break;
            }
        }

        let block_difficulty_epoch =
            block.header().number() / self.provider.consensus().difficulty_adjustment_interval();

        for uncle in block.uncles() {
            let uncle_difficulty_epoch =
                uncle.header().number()
                    / self.provider.consensus().difficulty_adjustment_interval();

            if uncle.header().difficulty() != block.header().difficulty() {
                return Err(Error::Uncles(UnclesError::InvalidDifficulty));
            }

            if block_difficulty_epoch != uncle_difficulty_epoch {
                return Err(Error::Uncles(UnclesError::InvalidDifficultyEpoch));
            }

            if uncle.header().cellbase_id() != uncle.cellbase().hash() {
                return Err(Error::Uncles(UnclesError::InvalidCellbase));
            }

            let uncle_header = uncle.header.clone();

            let uncle_hash = uncle_header.hash();
            if included.contains(&uncle_hash) {
                return Err(Error::Uncles(UnclesError::Duplicate(uncle_hash)));
            }

            if excluded.contains(&uncle_hash) {
                return Err(Error::Uncles(UnclesError::InvalidInclude(uncle_hash)));
            }

            let proposals = uncle
                .proposal_transactions()
                .iter()
                .map(|id| id.hash())
                .collect::<Vec<_>>();

            if uncle_header.txs_proposal() != merkle_root(&proposals[..]) {
                return Err(Error::Uncles(UnclesError::ProposalTransactionsRoot));
            }

            let mut seen = HashSet::with_capacity(uncle.proposal_transactions().len());
            if !uncle
                .proposal_transactions()
                .iter()
                .all(|id| seen.insert(id))
            {
                return Err(Error::Uncles(UnclesError::ProposalTransactionDuplicate));
            }

            if !self
                .provider
                .consensus()
                .pow_engine()
                .verify_header(&uncle_header)
            {
                return Err(Error::Uncles(UnclesError::InvalidProof));
            }

            included.insert(uncle_hash);
        }

        Ok(())
    }
}

pub struct TransactionsVerifier<P> {
    provider: P,
}

impl<P: ChainProvider + CellProvider + Clone> ::std::clone::Clone for TransactionsVerifier<P> {
    fn clone(&self) -> Self {
        TransactionsVerifier {
            provider: self.provider.clone(),
        }
    }
}

struct TransactionsVerifierWrapper<'a, P: CellProvider + 'a> {
    verifier: &'a TransactionsVerifier<P>,
    block: &'a Block,
    output_indexs: FnvHashMap<H256, usize>,
}

impl<'a, P: CellProvider> CellProvider for TransactionsVerifierWrapper<'a, P> {
    fn cell(&self, _o: &OutPoint) -> CellStatus {
        unreachable!()
    }

    fn cell_at(&self, o: &OutPoint, parent: &H256) -> CellStatus {
        if let Some(i) = self.output_indexs.get(&o.hash) {
            match self.block.commit_transactions()[*i]
                .outputs()
                .get(o.index as usize)
            {
                Some(x) => CellStatus::Current(x.clone()),
                None => CellStatus::Unknown,
            }
        } else {
            let chain_cell_state = self.verifier.provider.cell_at(o, parent);
            if chain_cell_state.is_current() {
                CellStatus::Current(chain_cell_state.take_current().expect("state checked"))
            } else if chain_cell_state.is_old() {
                CellStatus::Old
            } else {
                CellStatus::Unknown
            }
        }
    }
}

impl<P: ChainProvider + CellProvider> TransactionsVerifier<P> {
    pub fn new(provider: P) -> Self {
        TransactionsVerifier { provider }
    }

    pub fn verify(&self, block: &Block) -> Result<(), Error> {
        let mut output_indexs = FnvHashMap::default();

        for (i, tx) in block.commit_transactions().iter().enumerate() {
            output_indexs.insert(tx.hash(), i);
        }
        let wrapper = TransactionsVerifierWrapper {
            verifier: &self,
            block,
            output_indexs,
        };

        let parent_hash = block.header().parent_hash();
        // make verifiers orthogonal
        // skip first tx, assume the first is cellbase, other verifier will verify cellbase
        let err: Vec<(usize, TransactionError)> = block
            .commit_transactions()
            .par_iter()
            .skip(1)
            .map(|x| wrapper.resolve_transaction_at(x, &parent_hash))
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

#[derive(Clone)]
pub struct CommitVerifier<CP> {
    provider: CP,
}

impl<CP: ChainProvider + Clone> CommitVerifier<CP> {
    pub fn new(provider: CP) -> Self {
        CommitVerifier { provider }
    }

    pub fn verify(&self, block: &Block) -> Result<(), Error> {
        let block_number = block.header().number();
        let t_prop = self.provider.consensus().transaction_propagation_time;
        let mut walk = self.provider.consensus().transaction_propagation_timeout;
        let start = block_number.saturating_sub(t_prop);

        if start < 1 {
            return Ok(());
        }

        let mut block_hash = block.header().parent_hash();
        let mut proposal_txs_ids = FnvHashSet::default();

        while walk > 0 {
            let block = self
                .provider
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

            block_hash = block.header().parent_hash();
            walk -= 1;
        }

        let commited_ids: FnvHashSet<_> = block
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
