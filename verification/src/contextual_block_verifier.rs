use crate::error::{CellbaseError, CommitError, Error};
use crate::uncles_verifier::{UncleProvider, UnclesVerifier};
use crate::{ContextualTransactionVerifier, TransactionVerifier};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::cell::ResolvedTransaction;
use ckb_core::extras::EpochExt;
use ckb_core::header::Header;
use ckb_core::transaction::Transaction;
use ckb_core::uncle::UncleBlock;
use ckb_core::Cycle;
use ckb_core::{block::Block, BlockNumber, Capacity, EpochNumber};
use ckb_dao::DaoCalculator;
use ckb_logger::error_target;
use ckb_store::ChainStore;
use ckb_traits::{BlockMedianTimeContext, ChainProvider};
use fnv::FnvHashSet;
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use rayon::iter::{IndexedParallelIterator, IntoParallelRefIterator, ParallelIterator};
use std::sync::Arc;

// Verification context for fork
pub struct ForkContext<'a, P> {
    pub attached_blocks: Vec<&'a Block>,
    pub detached_blocks: Vec<&'a Block>,
    pub provider: P,
}

impl<'a, P: ChainProvider> ForkContext<'a, P> {
    fn get_block_header(&self, block_hash: &H256) -> Option<Header> {
        self.attached_blocks
            .iter()
            .find(|b| b.header().hash() == block_hash)
            .and_then(|b| Some(b.header().to_owned()))
            .or_else(|| self.provider.store().get_block_header(block_hash))
    }
}

impl<'a, P: ChainProvider> BlockMedianTimeContext for ForkContext<'a, P> {
    fn median_block_count(&self) -> u64 {
        self.provider.consensus().median_time_block_count() as u64
    }

    fn timestamp_and_parent(&self, block_hash: &H256) -> (u64, H256) {
        let header = self
            .get_block_header(block_hash)
            .expect("[ForkContext] blocks used for median time exist");
        (header.timestamp(), header.parent_hash().to_owned())
    }

    fn get_block_hash(&self, block_number: BlockNumber) -> Option<H256> {
        self.attached_blocks
            .iter()
            .find(|b| b.header().number() == block_number)
            .and_then(|b| Some(b.header().hash().to_owned()))
            .or_else(|| self.provider.store().get_block_hash(block_number))
    }
}

pub(crate) struct UncleVerifierContext<'a, P> {
    epoch: &'a EpochExt,
    excluded: FnvHashSet<&'a H256>,
    detached: FnvHashSet<&'a H256>,
    detached_uncles: FnvHashSet<&'a H256>,
    provider: &'a P,
}

impl<'a, P: ChainProvider> UncleVerifierContext<'a, P> {
    pub(crate) fn new(fork: &'a ForkContext<'a, P>, epoch: &'a EpochExt, block: &'a Block) -> Self {
        let mut excluded = FnvHashSet::default();
        excluded.insert(block.header().hash());

        for pre in &fork.attached_blocks {
            excluded.insert(pre.header().hash());
            for uncle in pre.uncles() {
                excluded.insert(uncle.header.hash());
            }
        }
        let detached = fork
            .detached_blocks
            .iter()
            .map(|b| b.header().hash())
            .collect();
        let detached_uncles = fork
            .detached_blocks
            .iter()
            .flat_map(|b| b.uncles().iter().map(UncleBlock::hash))
            .collect();
        UncleVerifierContext {
            epoch,
            excluded,
            detached,
            detached_uncles,
            provider: &fork.provider,
        }
    }
}

impl<'a, P: ChainProvider> UncleProvider for UncleVerifierContext<'a, P> {
    fn double_inclusion(&self, hash: &H256) -> bool {
        if self.excluded.contains(hash) {
            return true;
        }

        // main chain
        if self.provider.store().get_block_number(hash).is_some() {
            return !self.detached.contains(hash);
        }

        if self.provider.store().is_uncle(hash) {
            return !self.detached_uncles.contains(hash);
        }

        false
    }

    fn epoch(&self) -> &EpochExt {
        self.epoch
    }

    fn consensus(&self) -> &Consensus {
        self.provider.consensus()
    }
}

#[derive(Clone)]
pub struct CommitVerifier<'a, CP> {
    provider: &'a CP,
    block: &'a Block,
}

impl<'a, CP: ChainProvider + Clone> CommitVerifier<'a, CP> {
    pub fn new(provider: &'a CP, block: &'a Block) -> Self {
        CommitVerifier { provider, block }
    }

    pub fn verify(&self) -> Result<(), Error> {
        if self.block.is_genesis() {
            return Ok(());
        }
        let block_number = self.block.header().number();
        let proposal_window = self.provider.consensus().tx_proposal_window();
        let proposal_start = block_number.saturating_sub(proposal_window.farthest());
        let mut proposal_end = block_number.saturating_sub(proposal_window.closest());

        let mut block_hash = self
            .provider
            .store()
            .get_ancestor(self.block.header().parent_hash(), proposal_end)
            .map(|h| h.hash().to_owned())
            .ok_or_else(|| Error::Commit(CommitError::AncestorNotFound))?;

        let mut proposal_txs_ids = FnvHashSet::default();

        while proposal_end >= proposal_start {
            let header = self
                .provider
                .store()
                .get_block_header(&block_hash)
                .ok_or_else(|| Error::Commit(CommitError::AncestorNotFound))?;
            if header.is_genesis() {
                break;
            }

            if let Some(ids) = self
                .provider
                .store()
                .get_block_proposal_txs_ids(&block_hash)
            {
                proposal_txs_ids.extend(ids);
            }
            if let Some(uncles) = self.provider.store().get_block_uncles(&block_hash) {
                uncles
                    .iter()
                    .for_each(|uncle| proposal_txs_ids.extend(uncle.proposals()));
            }

            block_hash = header.parent_hash().to_owned();
            proposal_end -= 1;
        }

        let committed_ids: FnvHashSet<_> = self
            .block
            .transactions()
            .iter()
            .skip(1)
            .map(Transaction::proposal_short_id)
            .collect();

        let difference: Vec<_> = committed_ids.difference(&proposal_txs_ids).collect();

        if !difference.is_empty() {
            error_target!(
                crate::LOG_TARGET,
                "Block {} {:x}",
                self.block.header().number(),
                self.block.header().hash()
            );
            error_target!(crate::LOG_TARGET, "proposal_window {:?}", proposal_window);
            error_target!(
                crate::LOG_TARGET,
                "committed_ids {} ",
                serde_json::to_string(&committed_ids).unwrap()
            );
            error_target!(
                crate::LOG_TARGET,
                "proposal_txs_ids {} ",
                serde_json::to_string(&proposal_txs_ids).unwrap()
            );
            return Err(Error::Commit(CommitError::Invalid));
        }
        Ok(())
    }
}

pub struct RewardVerifier<'a, P> {
    resolved: &'a [ResolvedTransaction<'a>],
    parent: &'a Header,
    provider: &'a P,
}

impl<'a, P> RewardVerifier<'a, P>
where
    P: ChainProvider,
{
    pub fn new(provider: &'a P, resolved: &'a [ResolvedTransaction], parent: &'a Header) -> Self {
        RewardVerifier {
            parent,
            provider,
            resolved,
        }
    }

    pub fn verify(&self) -> Result<Vec<Capacity>, Error> {
        let cellbase = &self.resolved[0];
        let (target_lock, block_reward) = self
            .provider
            .finalize_block_reward(self.parent)
            .map_err(|_| Error::CannotFetchBlockReward)?;
        if cellbase.transaction.outputs_capacity()? != block_reward {
            return Err(Error::Cellbase(CellbaseError::InvalidRewardAmount));
        }
        if cellbase.transaction.outputs()[0].lock != target_lock {
            return Err(Error::Cellbase(CellbaseError::InvalidRewardTarget));
        }
        let txs_fees = self
            .resolved
            .iter()
            .skip(1)
            .map(|tx| {
                DaoCalculator::new(self.provider.consensus(), Arc::clone(self.provider.store()))
                    .transaction_fee(&tx)
                    .map_err(|_| Error::FeeCalculation)
            })
            .collect::<Result<Vec<Capacity>, Error>>()?;

        Ok(txs_fees)
    }
}

struct DaoHeaderVerifier<'a, P> {
    provider: &'a P,
    resolved: &'a [ResolvedTransaction<'a>],
    parent: &'a Header,
    header: &'a Header,
}

impl<'a, P> DaoHeaderVerifier<'a, P>
where
    P: ChainProvider,
{
    pub fn new(
        provider: &'a P,
        resolved: &'a [ResolvedTransaction<'a>],
        parent: &'a Header,
        header: &'a Header,
    ) -> Self {
        DaoHeaderVerifier {
            provider,
            resolved,
            parent,
            header,
        }
    }

    pub fn verify(&self) -> Result<(), Error> {
        let dao = DaoCalculator::new(
            &self.provider.consensus(),
            Arc::clone(self.provider.store()),
        )
        .dao_field(&self.resolved, self.parent)
        .map_err(|e| {
            error_target!(
                crate::LOG_TARGET,
                "Error generating dao data for block {:x}: {:?}",
                self.header.hash(),
                e
            );
            Error::DAOGeneration
        })?;

        if dao != self.header.dao() {
            return Err(Error::InvalidDAO);
        }
        Ok(())
    }
}

struct BlockTxsVerifier<'a, P> {
    context: &'a ForkContext<'a, P>,
    block_number: BlockNumber,
    epoch_number: EpochNumber,
    resolved: &'a [ResolvedTransaction<'a>],
}

impl<'a, P> BlockTxsVerifier<'a, P>
where
    P: ChainProvider,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        context: &'a ForkContext<'a, P>,
        block_number: BlockNumber,
        epoch_number: EpochNumber,
        resolved: &'a [ResolvedTransaction<'a>],
    ) -> BlockTxsVerifier<'a, P> {
        BlockTxsVerifier {
            context,
            block_number,
            epoch_number,
            resolved,
        }
    }

    pub fn verify(&self, txs_verify_cache: &mut LruCache<H256, Cycle>) -> Result<Cycle, Error> {
        // make verifiers orthogonal
        let ret_set = self
            .resolved
            .par_iter()
            .enumerate()
            .map(|(index, tx)| {
                let tx_hash = tx.transaction.hash().to_owned();
                if let Some(cycles) = txs_verify_cache.get(&tx_hash) {
                    ContextualTransactionVerifier::new(
                        &tx,
                        self.context,
                        self.block_number,
                        self.epoch_number,
                        self.context.provider.consensus(),
                    )
                    .verify()
                    .map_err(|e| Error::Transactions((index, e)))
                    .map(|_| (tx_hash, *cycles))
                } else {
                    TransactionVerifier::new(
                        &tx,
                        self.context,
                        self.block_number,
                        self.epoch_number,
                        self.context.provider.consensus(),
                        self.context.provider.script_config(),
                        self.context.provider.store(),
                    )
                    .verify(self.context.provider.consensus().max_block_cycles())
                    .map_err(|e| Error::Transactions((index, e)))
                    .map(|cycles| (tx_hash, cycles))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;

        let sum: Cycle = ret_set.iter().map(|(_, cycles)| cycles).sum();

        for (hash, cycles) in ret_set {
            txs_verify_cache.insert(hash, cycles);
        }

        if sum > self.context.provider.consensus().max_block_cycles() {
            Err(Error::ExceededMaximumCycles)
        } else {
            Ok(sum)
        }
    }
}

fn prepare_epoch_ext<P: ChainProvider>(provider: &P, parent: &Header) -> Result<EpochExt, Error> {
    let parent_ext = provider
        .get_block_epoch(parent.hash())
        .ok_or_else(|| Error::UnknownParent(parent.hash().clone()))?;
    Ok(provider
        .next_epoch_ext(&parent_ext, parent)
        .unwrap_or(parent_ext))
}

pub struct ContextualBlockVerifier<'a, P> {
    context: &'a ForkContext<'a, P>,
}

impl<'a, P: ChainProvider> ContextualBlockVerifier<'a, P>
where
    P: ChainProvider + Clone,
{
    pub fn new(context: &'a ForkContext<'a, P>) -> Self {
        ContextualBlockVerifier { context }
    }

    pub fn verify(
        &self,
        resolved: &[ResolvedTransaction],
        block: &Block,
        txs_verify_cache: &mut LruCache<H256, Cycle>,
    ) -> Result<(Cycle, Vec<Capacity>), Error> {
        let parent_hash = block.header().parent_hash();
        let parent = self
            .context
            .provider
            .store()
            .get_block_header(parent_hash)
            .ok_or_else(|| Error::UnknownParent(parent_hash.clone()))?;

        let epoch_ext = if block.is_genesis() {
            self.context
                .provider
                .consensus()
                .genesis_epoch_ext()
                .to_owned()
        } else {
            prepare_epoch_ext(&self.context.provider, &parent)?
        };

        let uncle_verifier_context = UncleVerifierContext::new(self.context, &epoch_ext, block);
        UnclesVerifier::new(uncle_verifier_context, block).verify()?;

        CommitVerifier::new(&self.context.provider, block).verify()?;
        DaoHeaderVerifier::new(&self.context.provider, resolved, &parent, &block.header())
            .verify()?;
        let txs_fees = RewardVerifier::new(&self.context.provider, resolved, &parent).verify()?;

        let cycles = BlockTxsVerifier::new(
            self.context,
            block.header().number(),
            block.header().epoch(),
            resolved,
        )
        .verify(txs_verify_cache)?;

        Ok((cycles, txs_fees))
    }
}
