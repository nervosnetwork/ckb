use crate::error::{CellbaseError, CommitError, Error, UnclesError};
use crate::header_verifier::HeaderResolver;
use crate::{TransactionVerifier, Verifier};
use ckb_core::cell::ResolvedTransaction;
use ckb_core::extras::EpochExt;
use ckb_core::header::Header;
use ckb_core::transaction::{Capacity, CellInput, CellOutput, Transaction};
use ckb_core::Cycle;
use ckb_core::{block::Block, BlockNumber};
use ckb_store::ChainStore;
use ckb_traits::{BlockMedianTimeContext, ChainProvider};
use fnv::FnvHashSet;
use log::error;
use numext_fixed_uint::U256;
use rayon::iter::{IndexedParallelIterator, IntoParallelRefIterator, ParallelIterator};
use std::collections::HashSet;
use std::sync::Arc;

//TODO: cellbase, witness
#[derive(Clone)]
pub struct BlockVerifier<P> {
    // Verify if the committed and proposed transactions contains duplicate
    duplicate: DuplicateVerifier,
    // Verify the cellbase
    cellbase: CellbaseVerifier,
    // Verify the the committed and proposed transactions merkle root match header's announce
    merkle_root: MerkleRootVerifier,
    // Verify the the uncle
    uncles: UnclesVerifier<P>,
    // Verify the the propose-then-commit consensus rule
    commit: CommitVerifier<P>,
    // Verify the amount of proposals does not exceed the limit.
    block_proposals_limit: BlockProposalsLimitVerifier,
    // Verify the size of the block does not exceed the limit.
    block_bytes: BlockBytesVerifier,
}

pub trait EpochProvider {
    fn epoch(&self) -> &EpochExt;
}

#[derive(Clone)]
pub struct EpochMock;

impl EpochProvider for EpochMock {
    fn epoch(&self) -> &EpochExt {
        unimplemented!()
    }
}

impl<P> BlockVerifier<P>
where
    P: ChainProvider + Clone,
{
    pub fn new(provider: P) -> Self {
        let proof_size = provider.consensus().pow_engine().proof_size();
        let max_block_proposals_limit = provider.consensus().max_block_proposals_limit();
        let max_block_bytes = provider.consensus().max_block_bytes();

        BlockVerifier {
            // TODO change all new fn's chain to reference
            duplicate: DuplicateVerifier::new(),
            cellbase: CellbaseVerifier::new(),
            merkle_root: MerkleRootVerifier::new(),
            uncles: UnclesVerifier::new(provider.clone(), mock),
            commit: CommitVerifier::new(provider),
            block_proposals_limit: BlockProposalsLimitVerifier::new(max_block_proposals_limit),
            block_bytes: BlockBytesVerifier::new(max_block_bytes, proof_size),
        }
    }
}

impl<P> Verifier for BlockVerifier<P>
where
    P: ChainProvider + Clone,
{
    type Target = Block;

    fn verify(&self, target: &Block) -> Result<(), Error> {
        self.block_proposals_limit.verify(target)?;
        self.block_bytes.verify(target)?;
        self.cellbase.verify(target)?;
        self.duplicate.verify(target)?;
        self.merkle_root.verify(target)?;
        self.commit.verify(target)?;
        self.uncles.verify(target)
    }
}

#[derive(Clone)]
pub struct CellbaseVerifier {}

impl CellbaseVerifier {
    pub fn new() -> Self {
        CellbaseVerifier {}
    }

    pub fn verify(&self, block: &Block) -> Result<(), Error> {
        let cellbase_len = block
            .transactions()
            .iter()
            .filter(|tx| tx.is_cellbase())
            .count();

        // empty checked, block must contain cellbase
        if cellbase_len != 1 {
            return Err(Error::Cellbase(CellbaseError::InvalidQuantity));
        }

        let cellbase_transaction = &block.transactions()[0];
        if !cellbase_transaction.is_cellbase() {
            return Err(Error::Cellbase(CellbaseError::InvalidPosition));
        }

        let cellbase_input = &cellbase_transaction.inputs()[0];
        if cellbase_input != &CellInput::new_cellbase_input(block.header().number()) {
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

        if cellbase_transaction
            .outputs()
            .iter()
            .any(CellOutput::is_occupied_capacity_overflow)
        {
            return Err(Error::CapacityOverflow);
        };

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
        let mut seen = HashSet::with_capacity(block.transactions().len());
        if !block.transactions().iter().all(|tx| seen.insert(tx.hash())) {
            return Err(Error::CommitTransactionDuplicate);
        }

        let mut seen = HashSet::with_capacity(block.proposals().len());
        if !block.proposals().iter().all(|id| seen.insert(id)) {
            return Err(Error::ProposalTransactionDuplicate);
        }
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct MerkleRootVerifier {}

impl MerkleRootVerifier {
    pub fn new() -> Self {
        MerkleRootVerifier::default()
    }

    pub fn verify(&self, block: &Block) -> Result<(), Error> {
        if block.header().transactions_root() != &block.cal_transactions_root() {
            return Err(Error::CommitTransactionsRoot);
        }

        if block.header().witnesses_root() != &block.cal_witnesses_root() {
            return Err(Error::WitnessesMerkleRoot);
        }

        if block.header().proposals_root() != &block.cal_proposals_root() {
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

    fn epoch(&self) -> Option<&EpochExt> {
        unimplemented!();
    }
}

// TODO redo uncle verifier, check uncle proposal duplicate
#[derive(Clone)]
pub struct UnclesVerifier<P> {
    provider: P,
    epoch: EpochMock,
}

impl<P> UnclesVerifier<P>
where
    P: ChainProvider + Clone,
{
    pub fn new(provider: P, epoch: EpochMock) -> Self {
        UnclesVerifier { provider, epoch }
    }

    // -  uncles_hash
    // -  uncles_num
    // -  depth
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
                expected: block.header().uncles_hash().to_owned(),
                actual: actual_uncles_hash,
            }));
        }

        // if block.uncles is empty, return
        if uncles_count == 0 {
            return Ok(());
        }

        // if block is genesis, which is expected with zero uncles, return error
        if block.is_genesis() {
            return Err(Error::Uncles(UnclesError::OverCount {
                max: 0,
                actual: uncles_count,
            }));
        }

        // verify uncles length =< max_uncles_num
        let max_uncles_num = self.provider.consensus().max_uncles_num() as u32;
        if uncles_count > max_uncles_num {
            return Err(Error::Uncles(UnclesError::OverCount {
                max: max_uncles_num,
                actual: uncles_count,
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
        excluded.insert(block.header().hash());
        let mut block_hash = block.header().parent_hash().to_owned();
        excluded.insert(block_hash.clone());
        for _ in 0..max_uncles_age {
            if let Some(header) = self.provider.block_header(&block_hash) {
                let parent_hash = header.parent_hash().to_owned();
                excluded.insert(parent_hash.clone());
                if let Some(uncles) = self.provider.uncles(&block_hash) {
                    uncles.iter().for_each(|uncle| {
                        excluded.insert(uncle.header.hash());
                    });
                };
                block_hash = parent_hash;
            } else {
                break;
            }
        }

        let epoch = self.epoch.epoch();

        for uncle in block.uncles() {
            if uncle.header().difficulty() != epoch.difficulty() {
                return Err(Error::Uncles(UnclesError::InvalidDifficulty));
            }

            if epoch.number() != uncle.header().epoch() {
                return Err(Error::Uncles(UnclesError::InvalidDifficultyEpoch));
            }

            let uncle_header = uncle.header.clone();

            let uncle_hash = uncle_header.hash();
            if included.contains(&uncle_hash) {
                return Err(Error::Uncles(UnclesError::Duplicate(uncle_hash)));
            }

            if excluded.contains(&uncle_hash) {
                return Err(Error::Uncles(UnclesError::InvalidInclude(uncle_hash)));
            }

            if uncle_header.proposals_root() != &uncle.cal_proposals_root() {
                return Err(Error::Uncles(UnclesError::ProposalTransactionsRoot));
            }

            let mut seen = HashSet::with_capacity(uncle.proposals().len());
            if !uncle.proposals().iter().all(|id| seen.insert(id)) {
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

    pub fn verify<M, CS: ChainStore>(
        &self,
        resolved: &[ResolvedTransaction],
        store: Arc<CS>,
        block_reward: Capacity,
        block_median_time_context: M,
        tip_number: BlockNumber,
        cellbase_maturity: BlockNumber,
    ) -> Result<(), Error>
    where
        M: BlockMedianTimeContext + Sync,
    {
        // verify cellbase reward
        let cellbase = &resolved[0];
        let fee: Capacity = resolved
            .iter()
            .skip(1)
            .map(ResolvedTransaction::fee)
            .try_fold(Capacity::zero(), |acc, rhs| {
                rhs.and_then(|x| acc.safe_add(x))
            })?;
        if cellbase.transaction.outputs_capacity()? > block_reward.safe_add(fee)? {
            return Err(Error::Cellbase(CellbaseError::InvalidReward));
        }

        // make verifiers orthogonal
        let cycles_set = resolved
            .par_iter()
            .skip(1)
            .enumerate()
            .map(|(index, tx)| {
                TransactionVerifier::new(
                    &tx,
                    Arc::clone(&store),
                    &block_median_time_context,
                    tip_number,
                    cellbase_maturity,
                )
                .verify(self.max_cycles)
                .map_err(|e| Error::Transactions((index, e)))
                .map(|cycles| cycles)
            })
            .collect::<Result<Vec<_>, _>>()?;

        let sum: Cycle = cycles_set.iter().sum();

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
            let header = self
                .provider
                .block_header(&block_hash)
                .ok_or_else(|| Error::Commit(CommitError::AncestorNotFound))?;
            if header.is_genesis() {
                break;
            }

            if let Some(ids) = self.provider.block_proposal_txs_ids(&block_hash) {
                proposal_txs_ids.extend(ids);
            }
            if let Some(uncles) = self.provider.uncles(&block_hash) {
                uncles
                    .iter()
                    .for_each(|uncle| proposal_txs_ids.extend(uncle.proposals()));
            }

            block_hash = header.parent_hash().to_owned();
            proposal_end -= 1;
        }

        let committed_ids: FnvHashSet<_> = block
            .transactions()
            .par_iter()
            .skip(1)
            .map(Transaction::proposal_short_id)
            .collect();

        let difference: Vec<_> = committed_ids.difference(&proposal_txs_ids).collect();

        if !difference.is_empty() {
            error!(target: "chain",  "Block {} {:x}", block.header().number(), block.header().hash());
            error!(target: "chain",  "proposal_window proposal_start {}", proposal_start);
            error!(target: "chain",  "committed_ids {} ", serde_json::to_string(&committed_ids).unwrap());
            error!(target: "chain",  "proposal_txs_ids {} ", serde_json::to_string(&proposal_txs_ids).unwrap());
            return Err(Error::Commit(CommitError::Invalid));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct BlockProposalsLimitVerifier {
    block_proposals_limit: u64,
}

impl BlockProposalsLimitVerifier {
    pub fn new(block_proposals_limit: u64) -> Self {
        BlockProposalsLimitVerifier {
            block_proposals_limit,
        }
    }

    pub fn verify(&self, block: &Block) -> Result<(), Error> {
        let proposals_len = block.proposals().len() as u64;
        if proposals_len <= self.block_proposals_limit {
            Ok(())
        } else {
            Err(Error::ExceededMaximumProposalsLimit)
        }
    }
}

#[derive(Clone)]
pub struct BlockBytesVerifier {
    block_bytes_limit: u64,
    proof_size: usize,
}

impl BlockBytesVerifier {
    pub fn new(block_bytes_limit: u64, proof_size: usize) -> Self {
        BlockBytesVerifier {
            block_bytes_limit,
            proof_size,
        }
    }

    pub fn verify(&self, block: &Block) -> Result<(), Error> {
        let block_bytes = block.serialized_size(self.proof_size) as u64;
        if block_bytes <= self.block_bytes_limit {
            Ok(())
        } else {
            Err(Error::ExceededMaximumBlockBytes)
        }
    }
}
