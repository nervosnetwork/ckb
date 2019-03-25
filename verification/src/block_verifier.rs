use crate::error::{CellbaseError, CommitError, Error, UnclesError};
use crate::header_verifier::HeaderResolver;
use crate::{InputVerifier, TransactionVerifier, Verifier};
use ckb_core::block::Block;
use ckb_core::cell::ResolvedTransaction;
use ckb_core::header::Header;
use ckb_core::transaction::{Capacity, CellInput};
use ckb_core::Cycle;
use ckb_merkle_tree::merkle_root;
use ckb_traits::ChainProvider;
use fnv::FnvHashSet;
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use rayon::iter::{IndexedParallelIterator, IntoParallelRefIterator, ParallelIterator};
use std::collections::HashSet;

//TODO: cellbase, witness
#[derive(Clone)]
pub struct BlockVerifier<P> {
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
}

impl<P> BlockVerifier<P>
where
    P: ChainProvider + Clone + 'static,
{
    pub fn new(provider: P) -> Self {
        BlockVerifier {
            // TODO change all new fn's chain to reference
            duplicate: DuplicateVerifier::new(),
            cellbase: CellbaseVerifier::new(provider.clone()),
            merkle_root: MerkleRootVerifier::new(),
            uncles: UnclesVerifier::new(provider.clone()),
            commit: CommitVerifier::new(provider),
        }
    }
}

impl<P: ChainProvider + Clone> Verifier for BlockVerifier<P> {
    type Target = Block;

    fn verify(&self, target: &Block) -> Result<(), Error> {
        self.cellbase.verify(target)?;
        self.duplicate.verify(target)?;
        self.merkle_root.verify(target)?;
        self.commit.verify(target)?;
        self.uncles.verify(target)
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

        // currently, we enforce`type` field of a cellbase output cell must be absent
        if cellbase_transaction
            .outputs()
            .iter()
            .any(|op| op.type_.is_some())
        {
            return Err(Error::Cellbase(CellbaseError::InvalidOutput));
        }

        Ok(())
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

        if block.header().txs_commit() != &merkle_root(&commits[..]) {
            return Err(Error::CommitTransactionsRoot);
        }

        let proposals = block
            .proposal_transactions()
            .iter()
            .map(|id| id.hash())
            .collect::<Vec<_>>();

        if block.header().txs_proposal() != &merkle_root(&proposals[..]) {
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
#[derive(Clone)]
pub struct UnclesVerifier<CP> {
    provider: CP,
}

impl<CP: ChainProvider + Clone> UnclesVerifier<CP> {
    pub fn new(provider: CP) -> Self {
        UnclesVerifier { provider }
    }

    // -  uncles_hash
    // -  uncles_num
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
        if &actual_uncles_hash != block.header().uncles_hash() {
            return Err(Error::Uncles(UnclesError::InvalidHash {
                expected: block.header().uncles_hash().clone(),
                actual: actual_uncles_hash,
            }));
        }
        // if block.uncles is empty, return
        if block.uncles().is_empty() {
            return Ok(());
        }

        // verify uncles lenght =< max_uncles_num
        let uncles_num = block.uncles().len();
        let max_uncles_num = self.provider.consensus().max_uncles_num();
        if uncles_num > max_uncles_num {
            return Err(Error::Uncles(UnclesError::OverCount {
                max: max_uncles_num,
                actual: uncles_num,
            }));
        }

        // verify uncles age
        let max_uncles_age = self.provider.consensus().max_uncles_age() as u64;
        for uncle in block.uncles() {
            let depth = block.header().number().saturating_sub(uncle.number());

            if depth > max_uncles_age || depth < 1 {
                return Err(Error::Uncles(UnclesError::InvalidDepth {
                    min: block.header().number().saturating_sub(max_uncles_age),
                    max: block.header().number().saturating_sub(1),
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
        excluded.insert(block.header().hash().clone());
        let mut block_hash = block.header().parent_hash().clone();
        excluded.insert(block_hash.clone());
        for _ in 0..max_uncles_age {
            if let Some(block) = self.provider.block(&block_hash) {
                excluded.insert(block.header().parent_hash().clone());
                for uncle in block.uncles() {
                    excluded.insert(uncle.header.hash().clone());
                }

                block_hash = block.header().parent_hash().clone();
            } else {
                break;
            }
        }

        let block_difficulty_epoch =
            block.header().number() / self.provider.consensus().difficulty_adjustment_interval();

        for uncle in block.uncles() {
            let uncle_difficulty_epoch = uncle.header().number()
                / self.provider.consensus().difficulty_adjustment_interval();

            if uncle.header().difficulty() != block.header().difficulty() {
                return Err(Error::Uncles(UnclesError::InvalidDifficulty));
            }

            if block_difficulty_epoch != uncle_difficulty_epoch {
                return Err(Error::Uncles(UnclesError::InvalidDifficultyEpoch));
            }

            if uncle.header().cellbase_id() != &uncle.cellbase().hash() {
                return Err(Error::Uncles(UnclesError::InvalidCellbase));
            }

            let uncle_header = uncle.header.clone();

            let uncle_hash = uncle_header.hash().clone();
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

            if uncle_header.txs_proposal() != &merkle_root(&proposals[..]) {
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

#[derive(Clone)]
pub struct TransactionsVerifier {
    max_cycles: Cycle,
}

impl TransactionsVerifier {
    pub fn new(max_cycles: Cycle) -> Self {
        TransactionsVerifier { max_cycles }
    }

    pub fn verify(
        &self,
        txs_verify_cache: &mut LruCache<H256, Cycle>,
        resolved: &[ResolvedTransaction],
        block_reward: Capacity,
    ) -> Result<(), Error> {
        // verify cellbase reward
        let cellbase = &resolved[0];
        let fee: Capacity = resolved.iter().skip(1).map(|rt| rt.fee()).sum();
        if cellbase.transaction.outputs_capacity() > block_reward + fee {
            return Err(Error::Cellbase(CellbaseError::InvalidReward));
        }
        // TODO use TransactionScriptsVerifier to verify cellbase script

        // make verifiers orthogonal
        let cycles_set = resolved
            .par_iter()
            .skip(1)
            .enumerate()
            .map(|(index, tx)| {
                if let Some(cycles) = txs_verify_cache.get(&tx.transaction.hash()) {
                    InputVerifier::new(&tx)
                        .verify()
                        .map_err(|e| Error::Transactions((index, e)))
                        .map(|_| (None, *cycles))
                } else {
                    TransactionVerifier::new(&tx)
                        .verify(self.max_cycles)
                        .map_err(|e| Error::Transactions((index, e)))
                        .map(|cycles| (Some(tx.transaction.hash()), cycles))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;

        let sum: Cycle = cycles_set.iter().map(|(_, cycles)| cycles).sum();

        for (hash, cycles) in cycles_set {
            if let Some(h) = hash {
                txs_verify_cache.insert(h, cycles);
            }
        }

        if sum > self.max_cycles {
            Err(Error::ExceededMaximumCycles)
        } else {
            Ok(())
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
        if block.is_genesis() {
            return Ok(());
        }
        let block_number = block.header().number();
        let proposal_window = self.provider.consensus().tx_proposal_window();
        let proposal_start = block_number.saturating_sub(proposal_window.start());
        let mut proposal_end = block_number.saturating_sub(proposal_window.end());

        let mut block_hash = self
            .provider
            .get_ancestor(&block.header().parent_hash(), proposal_end)
            .map(|h| h.hash())
            .ok_or_else(|| Error::Commit(CommitError::AncestorNotFound))?;

        let mut proposal_txs_ids = FnvHashSet::default();

        while proposal_end >= proposal_start {
            let block = self
                .provider
                .block(&block_hash)
                .ok_or_else(|| Error::Commit(CommitError::AncestorNotFound))?;
            if block.is_genesis() {
                break;
            }
            proposal_txs_ids.extend(block.union_proposal_ids());

            block_hash = block.header().parent_hash().clone();
            proposal_end -= 1;
        }

        let committed_ids: FnvHashSet<_> = block
            .commit_transactions()
            .par_iter()
            .skip(1)
            .map(|tx| tx.proposal_short_id())
            .collect();

        let difference: Vec<_> = committed_ids.difference(&proposal_txs_ids).collect();

        if !difference.is_empty() {
            return Err(Error::Commit(CommitError::Invalid));
        }
        Ok(())
    }
}
