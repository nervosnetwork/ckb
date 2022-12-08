use crate::uncles_verifier::{UncleProvider, UnclesVerifier};
use ckb_async_runtime::Handle;
use ckb_chain_spec::{
    consensus::{Consensus, ConsensusProvider},
    versionbits::{DeploymentPos, ThresholdState, VersionbitsIndexer},
};
use ckb_dao::DaoCalculator;
use ckb_dao_utils::DaoError;
use ckb_error::{Error, InternalErrorKind};
use ckb_logger::error_target;
use ckb_merkle_mountain_range::MMRStore;
use ckb_reward_calculator::RewardCalculator;
use ckb_store::{data_loader_wrapper::AsDataLoader, ChainStore};
use ckb_traits::HeaderProvider;
use ckb_types::{
    core::error::OutPointError,
    core::{
        cell::{HeaderChecker, ResolvedTransaction},
        BlockReward, BlockView, Capacity, Cycle, EpochExt, HeaderView, TransactionView,
    },
    packed::{Byte32, CellOutput, HeaderDigest, Script},
    prelude::*,
    utilities::merkle_mountain_range::ChainRootMMR,
};
use ckb_verification::cache::{
    TxVerificationCache, {CacheEntry, Completed},
};
use ckb_verification::{
    BlockErrorKind, CellbaseError, CommitError, ContextualTransactionVerifier,
    TimeRelativeTransactionVerifier, UnknownParentError,
};
use ckb_verification::{BlockTransactionsError, EpochError, TxVerifyEnv};
use ckb_verification_traits::Switch;
use rayon::iter::{IndexedParallelIterator, IntoParallelRefIterator, ParallelIterator};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{oneshot, RwLock};

/// Context for context-dependent block verification
pub struct VerifyContext<CS> {
    pub(crate) store: Arc<CS>,
    pub(crate) consensus: Arc<Consensus>,
}

impl<CS> Clone for VerifyContext<CS> {
    fn clone(&self) -> Self {
        VerifyContext {
            store: Arc::clone(&self.store),
            consensus: Arc::clone(&self.consensus),
        }
    }
}

impl<CS: ChainStore + VersionbitsIndexer> VerifyContext<CS> {
    /// Create new VerifyContext from `Store` and `Consensus`
    pub fn new(store: Arc<CS>, consensus: Arc<Consensus>) -> Self {
        VerifyContext { store, consensus }
    }

    fn finalize_block_reward(
        &self,
        parent: &HeaderView,
    ) -> Result<(Script, BlockReward), DaoError> {
        RewardCalculator::new(&self.consensus, self.store.as_ref()).block_reward_to_finalize(parent)
    }

    fn versionbits_active(&self, pos: DeploymentPos, header: &HeaderView) -> bool {
        self.consensus
            .versionbits_state(pos, header, self.store.as_ref())
            .map(|state| state == ThresholdState::Active)
            .unwrap_or(false)
    }
}

impl<CS: ChainStore> HeaderProvider for VerifyContext<CS> {
    fn get_header(&self, hash: &Byte32) -> Option<HeaderView> {
        self.store.get_block_header(hash)
    }
}

impl<CS: ChainStore> HeaderChecker for VerifyContext<CS> {
    fn check_valid(&self, block_hash: &Byte32) -> Result<(), OutPointError> {
        if !self.store.is_main_chain(block_hash) {
            return Err(OutPointError::InvalidHeader(block_hash.clone()));
        }
        self.store
            .get_block_header(block_hash)
            .ok_or_else(|| OutPointError::InvalidHeader(block_hash.clone()))?;
        Ok(())
    }
}

impl<CS: ChainStore> ConsensusProvider for VerifyContext<CS> {
    fn get_consensus(&self) -> &Consensus {
        &self.consensus
    }
}

pub struct UncleVerifierContext<'a, 'b, CS> {
    epoch: &'b EpochExt,
    context: &'a VerifyContext<CS>,
}

impl<'a, 'b, CS: ChainStore> UncleVerifierContext<'a, 'b, CS> {
    pub(crate) fn new(context: &'a VerifyContext<CS>, epoch: &'b EpochExt) -> Self {
        UncleVerifierContext { epoch, context }
    }
}

impl<'a, 'b, CS: ChainStore> UncleProvider for UncleVerifierContext<'a, 'b, CS> {
    fn double_inclusion(&self, hash: &Byte32) -> bool {
        self.context.store.get_block_number(hash).is_some() || self.context.store.is_uncle(hash)
    }

    fn descendant(&self, uncle: &HeaderView) -> bool {
        let parent_hash = uncle.data().raw().parent_hash();
        let uncle_number = uncle.number();
        let store = &self.context.store;

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
        self.epoch
    }

    fn consensus(&self) -> &Consensus {
        &self.context.consensus
    }
}

pub struct TwoPhaseCommitVerifier<'a, CS> {
    context: &'a VerifyContext<CS>,
    block: &'a BlockView,
}

impl<'a, CS: ChainStore + VersionbitsIndexer> TwoPhaseCommitVerifier<'a, CS> {
    pub fn new(context: &'a VerifyContext<CS>, block: &'a BlockView) -> Self {
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
            .ok_or(CommitError::AncestorNotFound)?;

        let mut proposal_txs_ids = HashSet::new();

        while proposal_end >= proposal_start {
            let header = self
                .context
                .store
                .get_block_header(&block_hash)
                .ok_or(CommitError::AncestorNotFound)?;
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

        if committed_ids.difference(&proposal_txs_ids).next().is_some() {
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
    resolved: &'a [Arc<ResolvedTransaction>],
    parent: &'b HeaderView,
    context: &'a VerifyContext<CS>,
}

impl<'a, 'b, CS: ChainStore + VersionbitsIndexer> RewardVerifier<'a, 'b, CS> {
    pub fn new(
        context: &'a VerifyContext<CS>,
        resolved: &'a [Arc<ResolvedTransaction>],
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
    context: &'a VerifyContext<CS>,
    resolved: &'a [Arc<ResolvedTransaction>],
    parent: &'b HeaderView,
    header: &'c HeaderView,
}

impl<'a, 'b, 'c, CS: ChainStore + VersionbitsIndexer> DaoHeaderVerifier<'a, 'b, 'c, CS> {
    pub fn new(
        context: &'a VerifyContext<CS>,
        resolved: &'a [Arc<ResolvedTransaction>],
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
        let dao = DaoCalculator::new(
            &self.context.consensus,
            &self.context.store.as_data_provider(),
        )
        .dao_field(self.resolved.iter().map(AsRef::as_ref), self.parent)
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
    context: VerifyContext<CS>,
    header: HeaderView,
    handle: &'a Handle,
    txs_verify_cache: &'a Arc<RwLock<TxVerificationCache>>,
}

impl<'a, CS: ChainStore + VersionbitsIndexer + 'static> BlockTxsVerifier<'a, CS> {
    pub fn new(
        context: VerifyContext<CS>,
        header: HeaderView,
        handle: &'a Handle,
        txs_verify_cache: &'a Arc<RwLock<TxVerificationCache>>,
    ) -> Self {
        BlockTxsVerifier {
            context,
            header,
            handle,
            txs_verify_cache,
        }
    }

    fn fetched_cache<K: IntoIterator<Item = Byte32> + Send + 'static>(
        &self,
        keys: K,
    ) -> HashMap<Byte32, CacheEntry> {
        let (sender, receiver) = oneshot::channel();
        let txs_verify_cache = Arc::clone(self.txs_verify_cache);
        self.handle.spawn(async move {
            let guard = txs_verify_cache.read().await;
            let ret = keys
                .into_iter()
                .filter_map(|hash| guard.peek(&hash).cloned().map(|value| (hash, value)))
                .collect();

            if let Err(e) = sender.send(ret) {
                error_target!(crate::LOG_TARGET, "TxsVerifier fetched_cache error {:?}", e);
            };
        });
        self.handle
            .block_on(receiver)
            .expect("fetched cache no exception")
    }

    fn update_cache(&self, ret: Vec<(Byte32, Completed)>) {
        let txs_verify_cache = Arc::clone(self.txs_verify_cache);
        self.handle.spawn(async move {
            let mut guard = txs_verify_cache.write().await;
            for (k, v) in ret {
                guard.put(k, CacheEntry::Completed(v));
            }
        });
    }

    pub fn verify(
        &self,
        resolved: &'a [Arc<ResolvedTransaction>],
        skip_script_verify: bool,
    ) -> Result<(Cycle, Vec<Completed>), Error> {
        // We should skip updating tx_verify_cache about the cellbase tx,
        // putting it in cache that will never be used until lru cache expires.
        let fetched_cache = if resolved.len() > 1 {
            let keys: Vec<Byte32> = resolved
                .iter()
                .skip(1)
                .map(|rtx| rtx.transaction.hash())
                .collect();

            self.fetched_cache(keys)
        } else {
            HashMap::new()
        };

        // make verifiers orthogonal
        let ret = resolved
            .par_iter()
            .enumerate()
            .map(|(index, tx)| {
                let tx_hash = tx.transaction.hash();
                let tx_env = TxVerifyEnv::new_commit(&self.header);
                if let Some(cache_entry) = fetched_cache.get(&tx_hash) {
                    match cache_entry {
                        CacheEntry::Completed(completed) => TimeRelativeTransactionVerifier::new(
                            Arc::clone(tx),
                            &self.context.consensus,
                            self.context.store.as_data_loader(),
                            &tx_env,
                        )
                        .verify()
                        .map_err(|error| {
                            BlockTransactionsError {
                                index: index as u32,
                                error,
                            }
                            .into()
                        })
                        .map(|_| (tx_hash, *completed)),
                        CacheEntry::Suspended(suspended) => ContextualTransactionVerifier::new(
                            Arc::clone(tx),
                            &self.context.consensus,
                            self.context.store.as_data_loader(),
                            &tx_env,
                        )
                        .complete(
                            self.context.consensus.max_block_cycles(),
                            skip_script_verify,
                            &suspended.snap,
                        )
                        .map_err(|error| {
                            BlockTransactionsError {
                                index: index as u32,
                                error,
                            }
                            .into()
                        })
                        .map(|completed| (tx_hash, completed)),
                    }
                } else {
                    ContextualTransactionVerifier::new(
                        Arc::clone(tx),
                        &self.context.consensus,
                        self.context.store.as_data_loader(),
                        &tx_env,
                    )
                    .verify(
                        self.context.consensus.max_block_cycles(),
                        skip_script_verify,
                    )
                    .map_err(|error| {
                        BlockTransactionsError {
                            index: index as u32,
                            error,
                        }
                        .into()
                    })
                    .map(|completed| (tx_hash, completed))
                }
            })
            .skip(1)
            .collect::<Result<Vec<(Byte32, Completed)>, Error>>()?;

        let sum: Cycle = ret.iter().map(|(_, cache_entry)| cache_entry.cycles).sum();
        let cache_entires = ret
            .iter()
            .map(|(_, completed)| completed)
            .cloned()
            .collect();
        if !ret.is_empty() {
            self.update_cache(ret);
        }

        if sum > self.context.consensus.max_block_cycles() {
            Err(BlockErrorKind::ExceededMaximumCycles.into())
        } else {
            Ok((sum, cache_entires))
        }
    }
}
/// EpochVerifier
///
/// Check for block epoch
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

/// BlockExtensionVerifier.
///
/// Check block extension.
#[derive(Clone)]
pub struct BlockExtensionVerifier<'a, 'b, CS, MS> {
    context: &'a VerifyContext<CS>,
    chain_root_mmr: &'a ChainRootMMR<MS>,
    parent: &'b HeaderView,
}

impl<'a, 'b, CS: ChainStore + VersionbitsIndexer, MS: MMRStore<HeaderDigest>>
    BlockExtensionVerifier<'a, 'b, CS, MS>
{
    pub fn new(
        context: &'a VerifyContext<CS>,
        chain_root_mmr: &'a ChainRootMMR<MS>,
        parent: &'b HeaderView,
    ) -> Self {
        BlockExtensionVerifier {
            context,
            chain_root_mmr,
            parent,
        }
    }

    pub fn verify(&self, block: &BlockView) -> Result<(), Error> {
        let extra_fields_count = block.data().count_extra_fields();

        let mmr_active = self
            .context
            .versionbits_active(DeploymentPos::LightClient, self.parent);

        match extra_fields_count {
            0 => {
                if mmr_active {
                    return Err(BlockErrorKind::NoBlockExtension.into());
                }
            }
            1 => {
                let extension = if let Some(data) = block.extension() {
                    data
                } else {
                    return Err(BlockErrorKind::UnknownFields.into());
                };
                if extension.is_empty() {
                    return Err(BlockErrorKind::EmptyBlockExtension.into());
                }
                if extension.len() > 96 {
                    return Err(BlockErrorKind::ExceededMaximumBlockExtensionBytes.into());
                }
                if mmr_active {
                    if extension.len() < 32 {
                        return Err(BlockErrorKind::InvalidBlockExtension.into());
                    }

                    let chain_root = self
                        .chain_root_mmr
                        .get_root()
                        .map_err(|e| InternalErrorKind::MMR.other(e))?;
                    let actual_root_hash = chain_root.calc_mmr_hash();
                    let expected_root_hash =
                        Byte32::new_unchecked(extension.raw_data().slice(..32));
                    if actual_root_hash != expected_root_hash {
                        return Err(BlockErrorKind::InvalidChainRoot.into());
                    }
                }
            }
            _ => {
                return Err(BlockErrorKind::UnknownFields.into());
            }
        }

        let actual_extra_hash = block.calc_extra_hash().extra_hash();
        if actual_extra_hash != block.extra_hash() {
            return Err(BlockErrorKind::InvalidExtraHash.into());
        }
        Ok(())
    }
}

/// Context-dependent verification checks for block
///
/// Contains:
/// - [`EpochVerifier`](./struct.EpochVerifier.html)
/// - [`UnclesVerifier`](./struct.UnclesVerifier.html)
/// - [`TwoPhaseCommitVerifier`](./struct.TwoPhaseCommitVerifier.html)
/// - [`DaoHeaderVerifier`](./struct.DaoHeaderVerifier.html)
/// - [`RewardVerifier`](./struct.RewardVerifier.html)
/// - [`BlockTxsVerifier`](./struct.BlockTxsVerifier.html)
pub struct ContextualBlockVerifier<'a, CS, MS> {
    context: VerifyContext<CS>,
    switch: Switch,
    handle: &'a Handle,
    txs_verify_cache: Arc<RwLock<TxVerificationCache>>,
    chain_root_mmr: &'a ChainRootMMR<MS>,
}

impl<'a, CS: ChainStore + VersionbitsIndexer + 'static, MS: MMRStore<HeaderDigest>>
    ContextualBlockVerifier<'a, CS, MS>
{
    /// Create new ContextualBlockVerifier
    pub fn new(
        context: VerifyContext<CS>,
        handle: &'a Handle,
        switch: Switch,
        txs_verify_cache: Arc<RwLock<TxVerificationCache>>,
        chain_root_mmr: &'a ChainRootMMR<MS>,
    ) -> Self {
        ContextualBlockVerifier {
            context,
            handle,
            switch,
            txs_verify_cache,
            chain_root_mmr,
        }
    }

    /// Perform context-dependent verification checks for block
    pub fn verify(
        &'a self,
        resolved: &'a [Arc<ResolvedTransaction>],
        block: &'a BlockView,
    ) -> Result<(Cycle, Vec<Completed>), Error> {
        let parent_hash = block.data().header().raw().parent_hash();
        let header = block.header();
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
            self.context
                .consensus
                .next_epoch_ext(&parent, &self.context.store.as_data_provider())
                .ok_or_else(|| UnknownParentError {
                    parent_hash: parent.hash(),
                })?
                .epoch()
        };

        if !self.switch.disable_epoch() {
            EpochVerifier::new(&epoch_ext, block).verify()?;
        }

        if !self.switch.disable_uncles() {
            let uncle_verifier_context = UncleVerifierContext::new(&self.context, &epoch_ext);
            UnclesVerifier::new(uncle_verifier_context, block).verify()?;
        }

        if !self.switch.disable_two_phase_commit() {
            TwoPhaseCommitVerifier::new(&self.context, block).verify()?;
        }

        if !self.switch.disable_daoheader() {
            DaoHeaderVerifier::new(&self.context, resolved, &parent, &block.header()).verify()?;
        }

        if !self.switch.disable_reward() {
            RewardVerifier::new(&self.context, resolved, &parent).verify()?;
        }

        BlockExtensionVerifier::new(&self.context, self.chain_root_mmr, &parent).verify(block)?;

        let ret = BlockTxsVerifier::new(
            self.context.clone(),
            header,
            self.handle,
            &self.txs_verify_cache,
        )
        .verify(resolved, self.switch.disable_script())?;
        Ok(ret)
    }
}
