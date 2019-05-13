use crate::error::{CellbaseError, CommitError, Error, UnclesError};
use crate::header_verifier::HeaderResolver;
use crate::{ContextualTransactionVerifier, TransactionVerifier, Verifier};
use ckb_core::cell::ResolvedTransaction;
use ckb_core::extras::EpochExt;
use ckb_core::header::Header;
use ckb_core::transaction::{Capacity, CellInput, Transaction};
use ckb_core::Cycle;
use ckb_core::{block::Block, BlockNumber};
use ckb_script::ScriptConfig;
use ckb_store::ChainStore;
use ckb_traits::{BlockMedianTimeContext, ChainProvider};
use dao_utils::calculate_transaction_fee;
use fnv::FnvHashSet;
use log::error;
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use rayon::iter::{IndexedParallelIterator, IntoParallelRefIterator, ParallelIterator};
use std::collections::HashSet;
use std::sync::Arc;

//TODO: cellbase, witness
#[derive(Clone)]
pub struct BlockVerifier<P> {
    provider: P,
}

fn prepare_epoch_ext<P: ChainProvider>(provider: &P, block: &Block) -> Result<EpochExt, Error> {
    if block.is_genesis() {
        return Ok(provider.consensus().genesis_epoch_ext().to_owned());
    }
    let parent_hash = block.header().parent_hash();
    let parent_ext = provider
        .get_block_epoch(parent_hash)
        .ok_or_else(|| Error::UnknownParent(parent_hash.clone()))?;
    let parent = provider
        .block_header(parent_hash)
        .ok_or_else(|| Error::UnknownParent(parent_hash.clone()))?;
    Ok(provider
        .next_epoch_ext(&parent_ext, &parent)
        .unwrap_or(parent_ext))
}

impl<P> BlockVerifier<P>
where
    P: ChainProvider + Clone,
{
    pub fn new(provider: P) -> Self {
        BlockVerifier { provider }
    }
}

impl<P> Verifier for BlockVerifier<P>
where
    P: ChainProvider + Clone,
{
    type Target = Block;

    fn verify(&self, target: &Block) -> Result<(), Error> {
        let consensus = self.provider.consensus();
        let proof_size = consensus.pow_engine().proof_size();
        let max_block_proposals_limit = consensus.max_block_proposals_limit();
        let max_block_bytes = consensus.max_block_bytes();
        let epoch_ext = prepare_epoch_ext(&self.provider, target)?;
        BlockProposalsLimitVerifier::new(max_block_proposals_limit).verify(target)?;
        BlockBytesVerifier::new(max_block_bytes, proof_size).verify(target)?;
        CellbaseVerifier::new().verify(target)?;
        DuplicateVerifier::new().verify(target)?;
        MerkleRootVerifier::new().verify(target)?;
        CommitVerifier::new(self.provider.clone()).verify(target)?;
        UnclesVerifier::new(self.provider.clone(), &epoch_ext).verify(target)
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

        if block.header().proposals_hash() != &block.cal_proposals_hash() {
            return Err(Error::ProposalTransactionsRoot);
        }

        Ok(())
    }
}

pub struct HeaderResolverWrapper<'a> {
    header: &'a Header,
    parent: Option<Header>,
    epoch: Option<EpochExt>,
}

impl<'a> HeaderResolverWrapper<'a> {
    pub fn new<CP>(header: &'a Header, provider: CP) -> Self
    where
        CP: ChainProvider,
    {
        let parent = provider.block_header(&header.parent_hash());
        let epoch = parent
            .as_ref()
            .and_then(|parent| {
                provider
                    .get_block_epoch(&parent.hash())
                    .map(|ext| (parent, ext))
            })
            .map(|(parent, last_epoch)| {
                provider
                    .next_epoch_ext(&last_epoch, parent)
                    .unwrap_or(last_epoch)
            });

        HeaderResolverWrapper {
            parent,
            header,
            epoch,
        }
    }
}

impl<'a> HeaderResolver for HeaderResolverWrapper<'a> {
    fn header(&self) -> &Header {
        self.header
    }

    fn parent(&self) -> Option<&Header> {
        self.parent.as_ref()
    }

    fn epoch(&self) -> Option<&EpochExt> {
        self.epoch.as_ref()
    }
}

// TODO redo uncle verifier, check uncle proposal duplicate
#[derive(Clone)]
pub struct UnclesVerifier<'a, P> {
    provider: P,
    epoch: &'a EpochExt,
}

impl<'a, P> UnclesVerifier<'a, P>
where
    P: ChainProvider + Clone,
{
    pub fn new(provider: P, epoch: &'a EpochExt) -> Self {
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
        excluded.insert(block.header().hash().to_owned());
        let mut block_hash = block.header().parent_hash().to_owned();
        excluded.insert(block_hash.clone());
        for _ in 0..max_uncles_age {
            if let Some(header) = self.provider.block_header(&block_hash) {
                let parent_hash = header.parent_hash().to_owned();
                excluded.insert(parent_hash.clone());
                if let Some(uncles) = self.provider.uncles(&block_hash) {
                    uncles.iter().for_each(|uncle| {
                        excluded.insert(uncle.header.hash().to_owned());
                    });
                };
                block_hash = parent_hash;
            } else {
                break;
            }
        }

        for uncle in block.uncles() {
            if uncle.header().difficulty() != self.epoch.difficulty() {
                return Err(Error::Uncles(UnclesError::InvalidDifficulty));
            }

            if self.epoch.number() != uncle.header().epoch() {
                return Err(Error::Uncles(UnclesError::InvalidDifficultyEpoch));
            }

            let uncle_header = uncle.header.clone();

            let uncle_hash = uncle_header.hash().to_owned();
            if included.contains(&uncle_hash) {
                return Err(Error::Uncles(UnclesError::Duplicate(uncle_hash.clone())));
            }

            if excluded.contains(&uncle_hash) {
                return Err(Error::Uncles(UnclesError::InvalidInclude(
                    uncle_hash.clone(),
                )));
            }

            if uncle_header.proposals_hash() != &uncle.cal_proposals_hash() {
                return Err(Error::Uncles(UnclesError::ProposalsHash));
            }

            let mut seen = HashSet::with_capacity(uncle.proposals().len());
            if !uncle.proposals().iter().all(|id| seen.insert(id)) {
                return Err(Error::Uncles(UnclesError::ProposalDuplicate));
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
pub struct TransactionsVerifier<'a, CP> {
    provider: CP,
    max_cycles: Cycle,
    script_config: &'a ScriptConfig,
}

impl<'a, CP: ChainProvider + Clone> TransactionsVerifier<'a, CP> {
    pub fn new(provider: CP, max_cycles: Cycle, script_config: &'a ScriptConfig) -> Self {
        TransactionsVerifier {
            provider,
            max_cycles,
            script_config,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn verify<M, CS: ChainStore>(
        &self,
        resolved: &[ResolvedTransaction],
        store: Arc<CS>,
        epoch: &EpochExt,
        block_median_time_context: M,
        header: &Header,
        cellbase_maturity: BlockNumber,
        txs_verify_cache: &mut LruCache<H256, Cycle>,
    ) -> Result<(), Error>
    where
        M: BlockMedianTimeContext + Sync,
    {
        let tip_number = header.number();
        let block_reward = epoch
            .block_reward(tip_number)
            .map_err(|_| Error::CannotFetchBlockReward)?;
        // Verify cellbase reward
        let cellbase = &resolved[0];
        let fee: Capacity = resolved
            .iter()
            .skip(1)
            .try_fold(Capacity::zero(), |acc, tx| {
                calculate_transaction_fee(Arc::clone(&store), &tx)
                    .ok_or(Error::FeeCalculation)
                    .and_then(|x| acc.safe_add(x).map_err(|_| Error::CapacityOverflow))
            })?;
        if cellbase.transaction.outputs_capacity()? > block_reward.safe_add(fee)? {
            return Err(Error::Cellbase(CellbaseError::InvalidReward));
        }

        // make verifiers orthogonal
        let ret_set = resolved
            .par_iter()
            .enumerate()
            .map(|(index, tx)| {
                let tx_hash = tx.transaction.hash().to_owned();
                if let Some(cycles) = txs_verify_cache.get(&tx_hash) {
                    ContextualTransactionVerifier::new(
                        &tx,
                        &block_median_time_context,
                        tip_number,
                        cellbase_maturity,
                    )
                    .verify()
                    .map_err(|e| Error::Transactions((index, e)))
                    .map(|_| (tx_hash, *cycles))
                } else {
                    TransactionVerifier::new(
                        &tx,
                        Arc::clone(&store),
                        &block_median_time_context,
                        tip_number,
                        cellbase_maturity,
                        &self.script_config,
                    )
                    .verify(self.max_cycles)
                    .map_err(|e| Error::Transactions((index, e)))
                    .map(|cycles| (tx_hash, cycles))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;

        let sum: Cycle = ret_set.iter().map(|(_, cycles)| cycles).sum();

        for (hash, cycles) in ret_set {
            txs_verify_cache.insert(hash, cycles);
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
            .map(|h| h.hash().to_owned())
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
            error!(target: "chain",  "proposal_window {:?}", proposal_window);
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
