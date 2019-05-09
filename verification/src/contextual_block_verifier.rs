use crate::error::{CellbaseError, CommitError, Error};
use crate::{ContextualTransactionVerifier, TransactionVerifier};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::cell::ResolvedTransaction;
use ckb_core::header::Header;
use ckb_core::script::Script;
use ckb_core::transaction::{Capacity, ProposalShortId};
use ckb_core::Cycle;
use ckb_core::{block::Block, BlockNumber};
use ckb_script::ScriptConfig;
use ckb_store::ChainStore;
use ckb_traits::{BlockMedianTimeContext, ChainProvider};
use fnv::{FnvHashMap, FnvHashSet};
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use rayon::iter::{IndexedParallelIterator, IntoParallelRefIterator, ParallelIterator};
use std::collections::BTreeMap;
use std::sync::Arc;

// Verification context for fork
struct ForkContext<'a, CS> {
    pub fork_attached_blocks: &'a [Block],
    pub store: Arc<CS>,
    pub consensus: &'a Consensus,
}

impl<'a, CS: ChainStore> ForkContext<'a, CS> {
    fn get_header(&self, number: BlockNumber) -> Option<Header> {
        match self
            .fork_attached_blocks
            .iter()
            .find(|b| b.header().number() == number)
        {
            Some(block) => Some(block.header().to_owned()),
            None => self
                .store
                .get_block_hash(number)
                .and_then(|hash| self.store.get_header(&hash)),
        }
    }
}

impl<'a, CS: ChainStore> BlockMedianTimeContext for ForkContext<'a, CS> {
    fn median_block_count(&self) -> u64 {
        self.consensus.median_time_block_count() as u64
    }

    fn timestamp(&self, number: BlockNumber) -> Option<u64> {
        self.get_header(number).map(|header| header.timestamp())
    }
}

pub struct RewardVerifier<'a, P> {
    header: &'a Header,
    resolved: &'a [ResolvedTransaction<'a>],
    provider: P,
    block_reward: Capacity,
}

type ProposalIds = BTreeMap<BlockNumber, FnvHashSet<ProposalShortId>>;
type Proposers = FnvHashMap<BlockNumber, Script>;

impl<'a, P> RewardVerifier<'a, P>
where
    P: ChainProvider + Clone,
{
    pub fn new(
        provider: P,
        header: &'a Header,
        resolved: &'a [ResolvedTransaction],
        block_reward: Capacity,
    ) -> Self {
        RewardVerifier {
            provider,
            header,
            resolved,
            block_reward,
        }
    }

    fn resolve_proposer(&self) -> Result<(ProposalIds, Proposers), Error> {
        let store = self.provider.store();

        let block_number = self.header.number();
        let proposal_window = self.provider.consensus().tx_proposal_window();
        let proposal_start = block_number.saturating_sub(proposal_window.start());
        let proposal_end = block_number.saturating_sub(proposal_window.end());

        let mut index = proposal_end;

        let mut block_hash = self
            .provider
            .get_ancestor(&self.header.parent_hash(), index)
            .map(|h| h.hash().to_owned())
            .ok_or_else(|| Error::Commit(CommitError::AncestorNotFound))?;

        let mut proposal_ids = BTreeMap::new();
        let mut proposers = FnvHashMap::with_capacity_and_hasher(
            proposal_window.start() as usize,
            Default::default(),
        );

        while index >= proposal_start {
            let header = self
                .provider
                .block_header(&block_hash)
                .ok_or_else(|| Error::Commit(CommitError::AncestorNotFound))?;
            if header.is_genesis() {
                break;
            }

            let mut ids: FnvHashSet<ProposalShortId> = FnvHashSet::default();
            let block_ids = store
                .get_block_proposal_txs_ids(&block_hash)
                .ok_or_else(|| Error::Commit(CommitError::AncestorNotFound))?;
            ids.extend(block_ids);
            let uncles = store
                .get_block_uncles(&block_hash)
                .ok_or_else(|| Error::Commit(CommitError::AncestorNotFound))?;
            for uncle in uncles {
                ids.extend(uncle.proposals())
            }
            let proposer_cellbase = store
                .get_cellbase(&block_hash)
                .ok_or_else(|| Error::Commit(CommitError::AncestorNotFound))?;

            let proposer_lock = proposer_cellbase
                .outputs()
                .get(0)
                .ok_or_else(|| Error::Commit(CommitError::AncestorNotFound))?
                .lock
                .clone();

            proposal_ids.insert(index, ids);
            proposers.insert(index, proposer_lock);

            block_hash = header.parent_hash().to_owned();
            index -= 1;
        }
        Ok((proposal_ids, proposers))
    }

    fn verify_total_fee(&self) -> Result<(), Error> {
        let cellbase = &self.resolved[0];
        let total_fee: Capacity = self
            .resolved
            .iter()
            .skip(1)
            .map(ResolvedTransaction::fee)
            .try_fold(Capacity::zero(), |acc, rhs| {
                rhs.and_then(|x| acc.safe_add(x))
            })?;

        if cellbase.transaction.outputs_capacity()? > self.block_reward.safe_add(total_fee)? {
            return Err(Error::Cellbase(CellbaseError::InvalidReward));
        }
        Ok(())
    }

    fn verify_reward(&self) -> Result<(), Error> {
        let proposal_window = self.provider.consensus().tx_proposal_window();
        let cellbase = &self.resolved[0];
        let miner_output = &cellbase.transaction.outputs()[0];
        let (proposal_ids, proposers) = self.resolve_proposer()?;

        let mut proposers_reward: FnvHashMap<Script, Capacity> =
            FnvHashMap::with_capacity_and_hasher(
                proposal_window.start() as usize,
                Default::default(),
            );
        let mut miner_fee = Capacity::zero();
        for rtx in self.resolved.iter().skip(1) {
            let tx_fee = rtx.fee()?;
            let proposal_short_id = rtx.transaction.proposal_short_id();
            let proposer = proposal_ids
                .iter()
                .find(|(_, ids)| ids.contains(&proposal_short_id))
                .and_then(|(n, _)| proposers.get(n))
                .cloned()
                .ok_or_else(|| Error::Commit(CommitError::Invalid))?;

            if proposer.eq(&miner_output.lock) {
                miner_fee = miner_fee.safe_add(tx_fee)?;
            } else {
                let proposer_reward = proposers_reward
                    .entry(proposer)
                    .or_insert_with(Capacity::zero);
                let proposer_fee = tx_fee.safe_mul_ratio(4, 10)?;
                *proposer_reward = proposer_reward.safe_add(proposer_fee)?;
                let surplus = tx_fee.safe_sub(proposer_fee)?;
                miner_fee = miner_fee.safe_add(surplus)?;
            }
        }

        let miner_reward = self.block_reward.safe_add(miner_fee)?;
        if miner_output.capacity != miner_reward {
            return Err(Error::Cellbase(CellbaseError::InvalidReward));
        }

        Ok(())
    }

    pub fn verify(&self) -> Result<(), Error> {
        if self.header.is_genesis() {
            return Ok(());
        }
        self.verify_total_fee()?;
        self.verify_reward()?;
        Ok(())
    }
}

struct BlockTxsVerifier<'a, M, CS> {
    cellbase_maturity: BlockNumber,
    script_config: &'a ScriptConfig,
    max_cycles: Cycle,
    block_median_time_context: &'a M,
    tip_number: BlockNumber,
    store: &'a Arc<CS>,
    resolved: &'a [ResolvedTransaction<'a>],
}

impl<'a, M, CS> BlockTxsVerifier<'a, M, CS>
where
    M: BlockMedianTimeContext + Sync,
    CS: ChainStore,
{
    pub fn new(
        cellbase_maturity: BlockNumber,
        script_config: &'a ScriptConfig,
        max_cycles: Cycle,
        block_median_time_context: &'a M,
        tip_number: BlockNumber,
        store: &'a Arc<CS>,
        resolved: &'a [ResolvedTransaction<'a>],
    ) -> BlockTxsVerifier<'a, M, CS> {
        BlockTxsVerifier {
            cellbase_maturity,
            script_config,
            max_cycles,
            block_median_time_context,
            tip_number,
            store,
            resolved,
        }
    }

    pub fn verify(&self, txs_verify_cache: &mut LruCache<H256, Cycle>) -> Result<(), Error> {
        let ret_set = self
            .resolved
            .par_iter()
            .enumerate()
            .map(|(index, tx)| {
                let tx_hash = tx.transaction.hash().to_owned();
                if let Some(cycles) = txs_verify_cache.get(&tx_hash) {
                    ContextualTransactionVerifier::new(
                        &tx,
                        self.block_median_time_context,
                        self.tip_number,
                        self.cellbase_maturity,
                    )
                    .verify()
                    .map_err(|e| Error::Transactions((index, e)))
                    .map(|_| (tx_hash, *cycles))
                } else {
                    TransactionVerifier::new(
                        &tx,
                        Arc::clone(self.store),
                        self.block_median_time_context,
                        self.tip_number,
                        self.cellbase_maturity,
                        self.script_config,
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

pub struct ContextualBlockVerifier<P> {
    provider: P,
}

impl<P: ChainProvider> ContextualBlockVerifier<P>
where
    P: ChainProvider + Clone,
{
    pub fn new(provider: P) -> Self {
        ContextualBlockVerifier { provider }
    }

    pub fn verify(
        &self,
        header: &Header,
        resolved: &[ResolvedTransaction],
        fork_attached_blocks: &[Block],
        block_reward: Capacity,
        tip_number: BlockNumber,
        txs_verify_cache: &mut LruCache<H256, Cycle>,
    ) -> Result<(), Error> {
        let consensus = self.provider.consensus();
        let store = self.provider.store();
        RewardVerifier::new(self.provider.clone(), header, resolved, block_reward).verify()?;

        let block_median_time_context = ForkContext {
            fork_attached_blocks,
            store: Arc::clone(store),
            consensus,
        };

        BlockTxsVerifier::new(
            consensus.cellbase_maturity(),
            self.provider.script_config(),
            consensus.max_block_cycles(),
            &block_median_time_context,
            tip_number,
            self.provider.store(),
            resolved,
        )
        .verify(txs_verify_cache)
    }
}
