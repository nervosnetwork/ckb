use crate::cache::{CacheEntry, TxVerifyCache};
use crate::error::{BlockTransactionsError, EpochError};
use crate::uncles_verifier::{UncleProvider, UnclesVerifier};
use crate::{
    BlockErrorKind, CellbaseError, CommitError, ContextualTransactionVerifier,
    TimeRelativeTransactionVerifier, UnknownParentError,
};
use ckb_async_runtime::Handle;
use ckb_chain_spec::consensus::Consensus;
use ckb_dao::DaoCalculator;
use ckb_error::Error;
use ckb_logger::error_target;
use ckb_reward_calculator::RewardCalculator;
use ckb_store::ChainStore;
use ckb_traits::{BlockMedianTimeContext, HeaderProvider};
use ckb_types::{
    core::error::OutPointError,
    core::{
        cell::{HeaderChecker, ResolvedTransaction},
        BlockNumber, BlockReward, BlockView, Capacity, Cycle, EpochExt, EpochNumberWithFraction,
        HeaderView, TransactionView,
    },
    packed::{Byte32, CellOutput, Script},
    prelude::*,
};
use rayon::iter::{IndexedParallelIterator, IntoParallelRefIterator, ParallelIterator};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{oneshot, RwLock};

/// TODO(doc): @zhangsoledad
pub struct VerifyContext<'a, CS> {
    pub(crate) store: &'a CS,
    pub(crate) consensus: &'a Consensus,
}

/// TODO(doc): @zhangsoledad
pub trait Switch {
    /// TODO(doc): @zhangsoledad
    fn disable_epoch(&self) -> bool;
    /// TODO(doc): @zhangsoledad
    fn disable_uncles(&self) -> bool;
    /// TODO(doc): @zhangsoledad
    fn disable_two_phase_commit(&self) -> bool;
    /// TODO(doc): @zhangsoledad
    fn disable_daoheader(&self) -> bool;
    /// TODO(doc): @zhangsoledad
    fn disable_reward(&self) -> bool;
}

impl<'a, CS: ChainStore<'a>> VerifyContext<'a, CS> {
    /// TODO(doc): @zhangsoledad
    pub fn new(store: &'a CS, consensus: &'a Consensus) -> Self {
        VerifyContext { store, consensus }
    }

    fn finalize_block_reward(&self, parent: &HeaderView) -> Result<(Script, BlockReward), Error> {
        RewardCalculator::new(self.consensus, self.store).block_reward_to_finalize(parent)
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
}

impl<'a, CS: ChainStore<'a>> HeaderProvider for VerifyContext<'a, CS> {
    fn get_header(&self, hash: &Byte32) -> Option<HeaderView> {
        self.store.get_block_header(hash)
    }
}

impl<'a, CS: ChainStore<'a>> HeaderChecker for VerifyContext<'a, CS> {
    fn check_valid(&self, block_hash: &Byte32) -> Result<(), Error> {
        match self.store.get_block_header(block_hash) {
            Some(header) => {
                let tip_header = self.store.get_tip_header().expect("tip should exist");
                let threshold =
                    self.consensus.cellbase_maturity().to_rational() + header.epoch().to_rational();
                let current = tip_header.epoch().to_rational();
                if current < threshold {
                    Err(OutPointError::ImmatureHeader(block_hash.clone()).into())
                } else {
                    Ok(())
                }
            }
            None => Err(OutPointError::InvalidHeader(block_hash.clone()).into()),
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

pub struct TwoPhaseCommitVerifier<'a, CS> {
    context: &'a VerifyContext<'a, CS>,
    block: &'a BlockView,
}

impl<'a, CS: ChainStore<'a>> TwoPhaseCommitVerifier<'a, CS> {
    pub fn new(context: &'a VerifyContext<'a, CS>, block: &'a BlockView) -> Self {
        TwoPhaseCommitVerifier { context, block }
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
            .ok_or_else(|| CommitError::AncestorNotFound)?;

        let mut proposal_txs_ids = HashSet::new();

        while proposal_end >= proposal_start {
            let header = self
                .context
                .store
                .get_block_header(&block_hash)
                .ok_or_else(|| CommitError::AncestorNotFound)?;
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
            return Err((CommitError::Invalid).into());
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

    #[allow(clippy::int_plus_one)]
    pub fn verify(&self) -> Result<(), Error> {
        let cellbase = &self.resolved[0];
        let no_finalization_target =
            (self.parent.number() + 1) <= self.context.consensus.finalization_delay_length();

        let (target_lock, block_reward) = self.context.finalize_block_reward(self.parent)?;
        let output = CellOutput::new_builder()
            .capacity(block_reward.total.pack())
            .lock(target_lock.clone())
            .build();
        let insufficient_reward_to_create_cell = output.is_lack_of_capacity(Capacity::zero())?;

        if no_finalization_target || insufficient_reward_to_create_cell {
            let ret = if cellbase.transaction.outputs().is_empty() {
                Ok(())
            } else {
                Err((CellbaseError::InvalidRewardTarget).into())
            };
            return ret;
        }

        if !insufficient_reward_to_create_cell {
            if cellbase.transaction.outputs_capacity()? != block_reward.total {
                return Err((CellbaseError::InvalidRewardAmount).into());
            }
            if cellbase
                .transaction
                .outputs()
                .get(0)
                .expect("cellbase should have output")
                .lock()
                != target_lock
            {
                return Err((CellbaseError::InvalidRewardTarget).into());
            }
        }

        Ok(())
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
                e
            })?;

        if dao != self.header.dao() {
            return Err((BlockErrorKind::InvalidDAO).into());
        }
        Ok(())
    }
}

struct BlockTxsVerifier<'a, CS> {
    context: &'a VerifyContext<'a, CS>,
    block_number: BlockNumber,
    epoch_number_with_fraction: EpochNumberWithFraction,
    parent_hash: Byte32,
    resolved: &'a [ResolvedTransaction],
}

impl<'a, CS: ChainStore<'a>> BlockTxsVerifier<'a, CS> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        context: &'a VerifyContext<'a, CS>,
        block_number: BlockNumber,
        epoch_number_with_fraction: EpochNumberWithFraction,
        parent_hash: Byte32,
        resolved: &'a [ResolvedTransaction],
    ) -> Self {
        BlockTxsVerifier {
            context,
            block_number,
            epoch_number_with_fraction,
            parent_hash,
            resolved,
        }
    }

    fn fetched_cache<K: IntoIterator<Item = Byte32> + Send + 'static>(
        &self,
        txs_verify_cache: Arc<RwLock<TxVerifyCache>>,
        keys: K,
        handle: &Handle,
    ) -> HashMap<Byte32, CacheEntry> {
        let (sender, receiver) = oneshot::channel();
        handle.spawn(async move {
            let guard = txs_verify_cache.read().await;
            let ret = keys
                .into_iter()
                .filter_map(|hash| guard.peek(&hash).cloned().map(|value| (hash, value)))
                .collect();

            if let Err(e) = sender.send(ret) {
                error_target!(crate::LOG_TARGET, "TxsVerifier fetched_cache error {:?}", e);
            };
        });
        handle
            .block_on(receiver)
            .expect("fetched cache no exception")
    }

    pub fn verify(
        &self,
        txs_verify_cache: Arc<RwLock<TxVerifyCache>>,
        handle: &Handle,
    ) -> Result<(Cycle, Vec<CacheEntry>), Error> {
        let keys: Vec<Byte32> = self
            .resolved
            .iter()
            .map(|rtx| rtx.transaction.hash())
            .collect();
        let fetched_cache = self.fetched_cache(Arc::clone(&txs_verify_cache), keys, handle);

        // make verifiers orthogonal
        let ret = self
            .resolved
            .par_iter()
            .enumerate()
            .map(|(index, tx)| {
                let tx_hash = tx.transaction.hash();
                if let Some(cache_entry) = fetched_cache.get(&tx_hash) {
                    TimeRelativeTransactionVerifier::new(
                        &tx,
                        self.context,
                        self.block_number,
                        self.epoch_number_with_fraction,
                        self.parent_hash.clone(),
                        self.context.consensus,
                    )
                    .verify()
                    .map_err(|error| {
                        BlockTransactionsError {
                            index: index as u32,
                            error,
                        }
                        .into()
                    })
                    .map(|_| (tx_hash, *cache_entry))
                } else {
                    ContextualTransactionVerifier::new(
                        &tx,
                        self.context,
                        self.block_number,
                        self.epoch_number_with_fraction,
                        self.parent_hash.clone(),
                        self.context.consensus,
                        self.context.store,
                    )
                    .verify(self.context.consensus.max_block_cycles())
                    .map_err(|error| {
                        BlockTransactionsError {
                            index: index as u32,
                            error,
                        }
                        .into()
                    })
                    .map(|cache_entry| (tx_hash, cache_entry))
                }
            })
            .collect::<Result<Vec<(Byte32, CacheEntry)>, Error>>()?;

        let sum: Cycle = ret.iter().map(|(_, cache_entry)| cache_entry.cycles).sum();
        let cache_entires = ret
            .iter()
            .map(|(_, cache_entry)| cache_entry)
            .cloned()
            .collect();
        handle.spawn(async move {
            let mut guard = txs_verify_cache.write().await;
            for (k, v) in ret {
                guard.put(k, v);
            }
        });

        if sum > self.context.consensus.max_block_cycles() {
            Err(BlockErrorKind::ExceededMaximumCycles.into())
        } else {
            Ok((sum, cache_entires))
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
        .ok_or_else(|| UnknownParentError {
            parent_hash: parent.hash(),
        })?;
    Ok(context
        .next_epoch_ext(&parent_ext, parent)
        .unwrap_or(parent_ext))
}

pub struct EpochVerifier<'a> {
    epoch: &'a EpochExt,
    block: &'a BlockView,
}

impl<'a> EpochVerifier<'a> {
    pub fn new(epoch: &'a EpochExt, block: &'a BlockView) -> Self {
        EpochVerifier { epoch, block }
    }

    pub fn verify(&self) -> Result<(), Error> {
        let header = self.block.header();
        let actual_epoch_with_fraction = header.epoch();
        let block_number = header.number();
        let epoch_with_fraction = self.epoch.number_with_fraction(block_number);
        if actual_epoch_with_fraction != epoch_with_fraction {
            return Err(EpochError::NumberMismatch {
                expected: epoch_with_fraction.full_value(),
                actual: actual_epoch_with_fraction.full_value(),
            }
            .into());
        }
        let actual_compact_target = header.compact_target();
        if self.epoch.compact_target() != actual_compact_target {
            return Err(EpochError::TargetMismatch {
                expected: self.epoch.compact_target(),
                actual: actual_compact_target,
            }
            .into());
        }
        Ok(())
    }
}

/// TODO(doc): @zhangsoledad
pub struct ContextualBlockVerifier<'a, CS> {
    context: &'a VerifyContext<'a, CS>,
}

impl<'a, CS: ChainStore<'a>> ContextualBlockVerifier<'a, CS> {
    /// TODO(doc): @zhangsoledad
    pub fn new(context: &'a VerifyContext<'a, CS>) -> Self {
        ContextualBlockVerifier { context }
    }

    /// TODO(doc): @zhangsoledad
    pub fn verify<SW: Switch>(
        &'a self,
        resolved: &'a [ResolvedTransaction],
        block: &'a BlockView,
        txs_verify_cache: Arc<RwLock<TxVerifyCache>>,
        handle: &Handle,
        switch: SW,
    ) -> Result<(Cycle, Vec<CacheEntry>), Error> {
        let parent_hash = block.data().header().raw().parent_hash();
        let parent = self
            .context
            .store
            .get_block_header(&parent_hash)
            .ok_or_else(|| UnknownParentError {
                parent_hash: parent_hash.clone(),
            })?;

        let epoch_ext = if block.is_genesis() {
            self.context.consensus.genesis_epoch_ext().to_owned()
        } else {
            prepare_epoch_ext(&self.context, &parent)?
        };

        if !switch.disable_epoch() {
            EpochVerifier::new(&epoch_ext, block).verify()?;
        }

        if !switch.disable_uncles() {
            let uncle_verifier_context = UncleVerifierContext::new(&self.context, &epoch_ext);
            UnclesVerifier::new(uncle_verifier_context, block).verify()?;
        }

        if !switch.disable_two_phase_commit() {
            TwoPhaseCommitVerifier::new(&self.context, block).verify()?;
        }

        if !switch.disable_daoheader() {
            DaoHeaderVerifier::new(&self.context, resolved, &parent, &block.header()).verify()?;
        }

        if !switch.disable_reward() {
            RewardVerifier::new(&self.context, resolved, &parent).verify()?;
        }

        BlockTxsVerifier::new(
            &self.context,
            block.number(),
            block.epoch(),
            parent_hash,
            resolved,
        )
        .verify(txs_verify_cache, handle)
    }
}
