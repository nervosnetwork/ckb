use crate::error::{CellbaseError, CommitError, Error};
use crate::txs_verify_cache::{FetchCache, UpdateCache};
use crate::uncles_verifier::{UncleProvider, UnclesVerifier};
use crate::{ContextualTransactionVerifier, TransactionVerifier};
use ckb_chain_spec::consensus::Consensus;
use ckb_dao::DaoCalculator;
use ckb_logger::error_target;
use ckb_reward_calculator::RewardCalculator;
use ckb_script::ScriptConfig;
use ckb_store::ChainStore;
use ckb_traits::BlockMedianTimeContext;
use ckb_types::{
    core::{
        cell::{HeaderChecker, ResolvedTransaction, UnresolvableError},
        BlockNumber, BlockReward, BlockView, Capacity, Cycle, EpochExt, EpochNumber, HeaderView,
        TransactionView,
    },
    packed::{Byte32, Script},
};
use futures::future::Future;
use lru_cache::LruCache;
use rayon::iter::{IndexedParallelIterator, IntoParallelRefIterator, ParallelIterator};
use std::collections::{HashMap, HashSet};
use tokio::sync::lock::Lock;

pub struct VerifyContext<'a, CS> {
    pub(crate) store: &'a CS,
    pub(crate) consensus: &'a Consensus,
    pub(crate) script_config: &'a ScriptConfig,
}

impl<'a, CS: ChainStore<'a>> VerifyContext<'a, CS> {
    pub fn new(store: &'a CS, consensus: &'a Consensus, script_config: &'a ScriptConfig) -> Self {
        VerifyContext {
            store,
            consensus,
            script_config,
        }
    }

    fn finalize_block_reward(&self, parent: &HeaderView) -> Result<(Script, BlockReward), Error> {
        RewardCalculator::new(self.consensus, self.store)
            .block_reward(parent)
            .map_err(|_| Error::CannotFetchBlockReward)
    }

    fn next_epoch_ext(&self, last_epoch: &EpochExt, header: &HeaderView) -> Option<EpochExt> {
        self.consensus.next_epoch_ext(
            last_epoch,
            header,
            |hash| self.store.get_block_header(hash),
            |hash| {
                self.store
                    .get_block_ext(hash)
                    .map(|ext| ext.total_uncles_count)
            },
        )
    }
}

impl<'a, CS: ChainStore<'a>> BlockMedianTimeContext for VerifyContext<'a, CS> {
    fn median_block_count(&self) -> u64 {
        self.consensus.median_time_block_count() as u64
    }

    fn timestamp_and_parent(&self, block_hash: &Byte32) -> (u64, BlockNumber, Byte32) {
        let header = self
            .store
            .get_block_header(block_hash)
            .expect("[ForkContext] blocks used for median time exist");
        (
            header.timestamp(),
            header.number(),
            header.data().raw().parent_hash(),
        )
    }
}

impl<'a, CS: ChainStore<'a>> HeaderChecker for VerifyContext<'a, CS> {
    fn check_valid(&self, block_hash: &Byte32) -> Result<(), UnresolvableError> {
        match self.store.get_block_number(block_hash) {
            Some(block_number) => {
                let tip_header = self.store.get_tip_header().expect("tip should exist");
                let tip_block_number = tip_header.number();
                if tip_block_number < block_number + self.consensus.cellbase_maturity() {
                    Err(UnresolvableError::ImmatureHeader(block_hash.clone()))
                } else {
                    Ok(())
                }
            }
            None => Err(UnresolvableError::InvalidHeader(block_hash.clone())),
        }
    }
}

pub struct UncleVerifierContext<'a, 'b, CS> {
    epoch: &'b EpochExt,
    context: &'a VerifyContext<'a, CS>,
}

impl<'a, 'b, CS: ChainStore<'a>> UncleVerifierContext<'a, 'b, CS> {
    pub(crate) fn new(context: &'a VerifyContext<'a, CS>, epoch: &'b EpochExt) -> Self {
        UncleVerifierContext { epoch, context }
    }
}

impl<'a, 'b, CS: ChainStore<'a>> UncleProvider for UncleVerifierContext<'a, 'b, CS> {
    fn double_inclusion(&self, hash: &Byte32) -> bool {
        self.context.store.get_block_number(hash).is_some() || self.context.store.is_uncle(hash)
    }

    fn descendant(&self, uncle: &HeaderView) -> bool {
        let parent_hash = uncle.data().raw().parent_hash();
        let uncle_number = uncle.number();
        let store = self.context.store;

        if store.get_block_number(&parent_hash).is_some() {
            return store
                .get_block_header(&parent_hash)
                .map(|parent| (parent.number() + 1) == uncle_number)
                .unwrap_or(false);
        }

        if let Some(uncle_parent) = store.get_uncle_header(&parent_hash) {
            return (uncle_parent.number() + 1) == uncle_number;
        }

        false
    }

    fn epoch(&self) -> &EpochExt {
        &self.epoch
    }

    fn consensus(&self) -> &Consensus {
        self.context.consensus
    }
}

pub struct CommitVerifier<'a, CS> {
    context: &'a VerifyContext<'a, CS>,
    block: &'a BlockView,
}

impl<'a, CS: ChainStore<'a>> CommitVerifier<'a, CS> {
    pub fn new(context: &'a VerifyContext<'a, CS>, block: &'a BlockView) -> Self {
        CommitVerifier { context, block }
    }

    pub fn verify(&self) -> Result<(), Error> {
        if self.block.is_genesis() {
            return Ok(());
        }
        let block_number = self.block.header().number();
        let proposal_window = self.context.consensus.tx_proposal_window();
        let proposal_start = block_number.saturating_sub(proposal_window.farthest());
        let mut proposal_end = block_number.saturating_sub(proposal_window.closest());

        let mut block_hash = self
            .context
            .store
            .get_block_hash(proposal_end)
            .ok_or_else(|| Error::Commit(CommitError::AncestorNotFound))?;

        let mut proposal_txs_ids = HashSet::new();

        while proposal_end >= proposal_start {
            let header = self
                .context
                .store
                .get_block_header(&block_hash)
                .ok_or_else(|| Error::Commit(CommitError::AncestorNotFound))?;
            if header.is_genesis() {
                break;
            }

            if let Some(ids) = self.context.store.get_block_proposal_txs_ids(&block_hash) {
                proposal_txs_ids.extend(ids);
            }
            if let Some(uncles) = self.context.store.get_block_uncles(&block_hash) {
                uncles
                    .data()
                    .into_iter()
                    .for_each(|uncle| proposal_txs_ids.extend(uncle.proposals()));
            }

            block_hash = header.data().raw().parent_hash();
            proposal_end -= 1;
        }

        let committed_ids: HashSet<_> = self
            .block
            .transactions()
            .iter()
            .skip(1)
            .map(TransactionView::proposal_short_id)
            .collect();

        let difference: Vec<_> = committed_ids.difference(&proposal_txs_ids).collect();

        if !difference.is_empty() {
            error_target!(
                crate::LOG_TARGET,
                "BlockView {} {}",
                self.block.number(),
                self.block.hash()
            );
            error_target!(crate::LOG_TARGET, "proposal_window {:?}", proposal_window);
            error_target!(crate::LOG_TARGET, "Committed Ids:");
            for committed_id in committed_ids.iter() {
                error_target!(crate::LOG_TARGET, "    {:?}", committed_id);
            }
            error_target!(crate::LOG_TARGET, "Proposal Txs Ids:");
            for proposal_txs_id in proposal_txs_ids.iter() {
                error_target!(crate::LOG_TARGET, "    {:?}", proposal_txs_id);
            }
            return Err(Error::Commit(CommitError::Invalid));
        }
        Ok(())
    }
}

pub struct RewardVerifier<'a, 'b, CS> {
    resolved: &'a [ResolvedTransaction],
    parent: &'b HeaderView,
    context: &'a VerifyContext<'a, CS>,
}

impl<'a, 'b, CS: ChainStore<'a>> RewardVerifier<'a, 'b, CS> {
    pub fn new(
        context: &'a VerifyContext<'a, CS>,
        resolved: &'a [ResolvedTransaction],
        parent: &'b HeaderView,
    ) -> Self {
        RewardVerifier {
            parent,
            context,
            resolved,
        }
    }

    pub fn verify(&self) -> Result<Vec<Capacity>, Error> {
        let cellbase = &self.resolved[0];
        let (target_lock, block_reward) = self.context.finalize_block_reward(self.parent)?;
        if cellbase.transaction.outputs_capacity()? != block_reward.total {
            return Err(Error::Cellbase(CellbaseError::InvalidRewardAmount));
        }
        if cellbase
            .transaction
            .outputs()
            .get(0)
            .expect("cellbase should have output")
            .lock()
            != target_lock
        {
            return Err(Error::Cellbase(CellbaseError::InvalidRewardTarget));
        }
        let txs_fees = self
            .resolved
            .iter()
            .skip(1)
            .map(|tx| {
                DaoCalculator::new(self.context.consensus, self.context.store)
                    .transaction_fee(&tx)
                    .map_err(|_| Error::FeeCalculation)
            })
            .collect::<Result<Vec<Capacity>, Error>>()?;

        Ok(txs_fees)
    }
}

struct DaoHeaderVerifier<'a, 'b, 'c, CS> {
    context: &'a VerifyContext<'a, CS>,
    resolved: &'a [ResolvedTransaction],
    parent: &'b HeaderView,
    header: &'c HeaderView,
}

impl<'a, 'b, 'c, CS: ChainStore<'a>> DaoHeaderVerifier<'a, 'b, 'c, CS> {
    pub fn new(
        context: &'a VerifyContext<'a, CS>,
        resolved: &'a [ResolvedTransaction],
        parent: &'b HeaderView,
        header: &'c HeaderView,
    ) -> Self {
        DaoHeaderVerifier {
            context,
            resolved,
            parent,
            header,
        }
    }

    pub fn verify(&self) -> Result<(), Error> {
        let dao = DaoCalculator::new(self.context.consensus, self.context.store)
            .dao_field(&self.resolved, self.parent)
            .map_err(|e| {
                error_target!(
                    crate::LOG_TARGET,
                    "Error generating dao data for block {}: {:?}",
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

struct BlockTxsVerifier<'a, CS> {
    context: &'a VerifyContext<'a, CS>,
    block_number: BlockNumber,
    epoch_number: EpochNumber,
    parent_hash: Byte32,
    resolved: &'a [ResolvedTransaction],
}

impl<'a, CS: ChainStore<'a>> BlockTxsVerifier<'a, CS> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        context: &'a VerifyContext<'a, CS>,
        block_number: BlockNumber,
        epoch_number: EpochNumber,
        parent_hash: Byte32,
        resolved: &'a [ResolvedTransaction],
    ) -> Self {
        BlockTxsVerifier {
            context,
            block_number,
            epoch_number,
            parent_hash,
            resolved,
        }
    }

    pub fn verify(self, txs_verify_cache: Lock<LruCache<Byte32, Cycle>>) -> Result<Cycle, Error> {
        let keys = self.resolved.iter().map(|rtx| rtx.transaction.hash());
        let fetched_cache = FetchCache::new(txs_verify_cache.clone(), keys)
            .wait()
            .expect("fetched cache no exception");

        // make verifiers orthogonal
        let ret = self
            .resolved
            .par_iter()
            .enumerate()
            .map(|(index, tx)| {
                let tx_hash = tx.transaction.hash();
                if let Some(cycles) = fetched_cache.get(&tx_hash) {
                    ContextualTransactionVerifier::new(
                        &tx,
                        self.context,
                        self.block_number,
                        self.epoch_number,
                        self.parent_hash.clone(),
                        self.context.consensus,
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
                        self.parent_hash.clone(),
                        self.context.consensus,
                        self.context.script_config,
                        self.context.store,
                    )
                    .verify(self.context.consensus.max_block_cycles())
                    .map_err(|e| Error::Transactions((index, e)))
                    .map(|cycles| (tx_hash, cycles))
                }
            })
            .collect::<Result<HashMap<Byte32, Cycle>, _>>()?;

        let sum: Cycle = ret.iter().map(|(_, cycles)| cycles).sum();
        let update = UpdateCache::new(txs_verify_cache.clone(), ret);
        update.wait().expect("update cache no exception");

        if sum > self.context.consensus.max_block_cycles() {
            Err(Error::ExceededMaximumCycles)
        } else {
            Ok(sum)
        }
    }
}

fn prepare_epoch_ext<'a, CS: ChainStore<'a>>(
    context: &VerifyContext<'a, CS>,
    parent: &HeaderView,
) -> Result<EpochExt, Error> {
    let parent_ext = context
        .store
        .get_block_epoch(&parent.hash())
        .ok_or_else(|| Error::UnknownParent(parent.hash()))?;
    Ok(context
        .next_epoch_ext(&parent_ext, parent)
        .unwrap_or(parent_ext))
}

pub struct ContextualBlockVerifier<'a, CS> {
    context: &'a VerifyContext<'a, CS>,
}

impl<'a, CS: ChainStore<'a>> ContextualBlockVerifier<'a, CS> {
    pub fn new(context: &'a VerifyContext<'a, CS>) -> Self {
        ContextualBlockVerifier { context }
    }

    pub fn verify(
        &'a self,
        resolved: &'a [ResolvedTransaction],
        block: &'a BlockView,
        txs_verify_cache: Lock<LruCache<Byte32, Cycle>>,
    ) -> Result<(Cycle, Vec<Capacity>), Error> {
        let parent_hash = block.data().header().raw().parent_hash();
        let parent = self
            .context
            .store
            .get_block_header(&parent_hash)
            .ok_or_else(|| Error::UnknownParent(parent_hash.clone()))?;

        let epoch_ext = if block.is_genesis() {
            self.context.consensus.genesis_epoch_ext().to_owned()
        } else {
            prepare_epoch_ext(&self.context, &parent)?
        };

        let uncle_verifier_context = UncleVerifierContext::new(&self.context, &epoch_ext);
        UnclesVerifier::new(uncle_verifier_context, block).verify()?;

        CommitVerifier::new(&self.context, block).verify()?;
        DaoHeaderVerifier::new(&self.context, resolved, &parent, &block.header()).verify()?;
        let txs_fees = RewardVerifier::new(&self.context, resolved, &parent).verify()?;

        let cycles = BlockTxsVerifier::new(
            &self.context,
            block.number(),
            block.epoch(),
            parent_hash,
            resolved,
        )
        .verify(txs_verify_cache)?;

        Ok((cycles, txs_fees))
    }
}
