use crate::error::{CellbaseError, CommitError, Error};
use crate::uncles_verifier::UnclesVerifier;
use crate::{ContextualTransactionVerifier, TransactionVerifier};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::cell::ResolvedTransaction;
use ckb_core::extras::EpochExt;
use ckb_core::header::Header;
use ckb_core::transaction::Capacity;
use ckb_core::transaction::Transaction;
use ckb_core::Cycle;
use ckb_core::{block::Block, BlockNumber, EpochNumber};
use ckb_script::ScriptConfig;
use ckb_store::ChainStore;
use ckb_traits::{BlockMedianTimeContext, ChainProvider};
use dao_utils::calculate_transaction_fee;
use fnv::FnvHashSet;
use log::error;
use lru_cache::LruCache;
use numext_fixed_hash::H256;
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

#[derive(Clone)]
pub struct CommitVerifier<'a, CP> {
    provider: CP,
    block: &'a Block,
}

impl<'a, CP: ChainProvider + Clone> CommitVerifier<'a, CP> {
    pub fn new(provider: CP, block: &'a Block) -> Self {
        CommitVerifier { provider, block }
    }

    pub fn verify(&self) -> Result<(), Error> {
        if self.block.is_genesis() {
            return Ok(());
        }
        let block_number = self.block.header().number();
        let proposal_window = self.provider.consensus().tx_proposal_window();
        let proposal_start = block_number.saturating_sub(proposal_window.start());
        let mut proposal_end = block_number.saturating_sub(proposal_window.end());

        let mut block_hash = self
            .provider
            .get_ancestor(self.block.header().parent_hash(), proposal_end)
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

        let committed_ids: FnvHashSet<_> = self
            .block
            .transactions()
            .iter()
            .skip(1)
            .map(Transaction::proposal_short_id)
            .collect();

        let difference: Vec<_> = committed_ids.difference(&proposal_txs_ids).collect();

        if !difference.is_empty() {
            error!(target: "chain",  "Block {} {:x}", self.block.header().number(), self.block.header().hash());
            error!(target: "chain",  "proposal_window {:?}", proposal_window);
            error!(target: "chain",  "committed_ids {} ", serde_json::to_string(&committed_ids).unwrap());
            error!(target: "chain",  "proposal_txs_ids {} ", serde_json::to_string(&proposal_txs_ids).unwrap());
            return Err(Error::Commit(CommitError::Invalid));
        }
        Ok(())
    }
}

pub struct RewardVerifier<'a, CS> {
    resolved: &'a [ResolvedTransaction<'a>],
    epoch: &'a EpochExt,
    number: BlockNumber,
    store: &'a Arc<CS>,
}

impl<'a, CS> RewardVerifier<'a, CS>
where
    CS: ChainStore,
{
    pub fn new(
        store: &'a Arc<CS>,
        resolved: &'a [ResolvedTransaction],
        epoch: &'a EpochExt,
        number: BlockNumber,
    ) -> Self {
        RewardVerifier {
            number,
            store,
            resolved,
            epoch,
        }
    }

    pub fn verify(&self) -> Result<(), Error> {
        let block_reward = self
            .epoch
            .block_reward(self.number)
            .map_err(|_| Error::CannotFetchBlockReward)?;
        // Verify cellbase reward
        let cellbase = &self.resolved[0];
        let fee: Capacity =
            self.resolved
                .iter()
                .skip(1)
                .try_fold(Capacity::zero(), |acc, tx| {
                    calculate_transaction_fee(Arc::clone(self.store), &tx)
                        .ok_or(Error::FeeCalculation)
                        .and_then(|x| acc.safe_add(x).map_err(|_| Error::CapacityOverflow))
                })?;
        if cellbase.transaction.outputs_capacity()? > block_reward.safe_add(fee)? {
            return Err(Error::Cellbase(CellbaseError::InvalidReward));
        }

        Ok(())
    }
}

struct BlockTxsVerifier<'a, M, CS> {
    cellbase_maturity: BlockNumber,
    script_config: &'a ScriptConfig,
    max_cycles: Cycle,
    block_median_time_context: &'a M,
    number: BlockNumber,
    epoch: EpochNumber,
    store: &'a Arc<CS>,
    resolved: &'a [ResolvedTransaction<'a>],
}

impl<'a, M, CS> BlockTxsVerifier<'a, M, CS>
where
    M: BlockMedianTimeContext + Sync,
    CS: ChainStore,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cellbase_maturity: BlockNumber,
        script_config: &'a ScriptConfig,
        max_cycles: Cycle,
        block_median_time_context: &'a M,
        number: BlockNumber,
        epoch: EpochNumber,
        store: &'a Arc<CS>,
        resolved: &'a [ResolvedTransaction<'a>],
    ) -> BlockTxsVerifier<'a, M, CS> {
        BlockTxsVerifier {
            cellbase_maturity,
            script_config,
            max_cycles,
            block_median_time_context,
            number,
            epoch,
            store,
            resolved,
        }
    }

    pub fn verify(&self, txs_verify_cache: &mut LruCache<H256, Cycle>) -> Result<Cycle, Error> {
        // make verifiers orthogonal
        let ret_set = self
            .resolved
            .iter()
            .enumerate()
            .map(|(index, tx)| {
                let tx_hash = tx.transaction.hash().to_owned();
                if let Some(cycles) = txs_verify_cache.get(&tx_hash) {
                    ContextualTransactionVerifier::new(
                        &tx,
                        self.block_median_time_context,
                        self.number,
                        self.epoch,
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
                        self.number,
                        self.epoch,
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
            Ok(sum)
        }
    }
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
        resolved: &[ResolvedTransaction],
        fork_attached_blocks: &[Block],
        block: &Block,
        txs_verify_cache: &mut LruCache<H256, Cycle>,
    ) -> Result<Cycle, Error> {
        let consensus = self.provider.consensus();
        let store = self.provider.store();
        let epoch_ext = prepare_epoch_ext(&self.provider, block)?;

        CommitVerifier::new(self.provider.clone(), block).verify()?;
        UnclesVerifier::new(self.provider.clone(), &epoch_ext, block).verify()?;
        RewardVerifier::new(
            self.provider.store(),
            resolved,
            &epoch_ext,
            block.header().number(),
        )
        .verify()?;

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
            block.header().number(),
            block.header().epoch(),
            self.provider.store(),
            resolved,
        )
        .verify(txs_verify_cache)
    }
}
