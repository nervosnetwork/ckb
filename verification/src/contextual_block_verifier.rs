use crate::error::{CellbaseError, CommitError, Error};
use crate::uncles_verifier::{UncleProvider, UnclesVerifier};
use crate::{ContextualTransactionVerifier, TransactionVerifier};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::cell::{HeaderProvider, HeaderStatus, ResolvedTransaction};
use ckb_core::extras::EpochExt;
use ckb_core::header::Header;
use ckb_core::reward::BlockReward;
use ckb_core::script::Script;
use ckb_core::transaction::{OutPoint, Transaction};
use ckb_core::Cycle;
use ckb_core::{block::Block, BlockNumber, Capacity, EpochNumber};
use ckb_dao::DaoCalculator;
use ckb_logger::error_target;
use ckb_reward_calculator::RewardCalculator;
use ckb_script::ScriptConfig;
use ckb_store::ChainStore;
use ckb_traits::BlockMedianTimeContext;
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use rayon::iter::{IndexedParallelIterator, IntoParallelRefIterator, ParallelIterator};
use std::collections::HashSet;

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

    fn finalize_block_reward(&self, parent: &Header) -> Result<(Script, BlockReward), Error> {
        RewardCalculator::new(self.consensus, self.store)
            .block_reward(parent)
            .map_err(|_| Error::CannotFetchBlockReward)
    }

    fn next_epoch_ext(&self, last_epoch: &EpochExt, header: &Header) -> Option<EpochExt> {
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

    fn timestamp_and_parent(&self, block_hash: &H256) -> (u64, BlockNumber, H256) {
        let header = self
            .store
            .get_block_header(block_hash)
            .expect("[ForkContext] blocks used for median time exist");
        (
            header.timestamp(),
            header.number(),
            header.parent_hash().to_owned(),
        )
    }
}

impl<'a, CS: ChainStore<'a>> HeaderProvider for VerifyContext<'a, CS> {
    fn header(&self, out_point: &OutPoint) -> HeaderStatus {
        if let Some(block_hash) = &out_point.block_hash {
            match self.store.get_block_number(&block_hash) {
                Some(_) => HeaderStatus::live_header(
                    self.store
                        .get_block_header(&block_hash)
                        .expect("header index checked"),
                ),
                None => HeaderStatus::Unknown,
            }
        } else {
            HeaderStatus::Unspecified
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
    fn double_inclusion(&self, hash: &H256) -> bool {
        self.context.store.get_block_number(hash).is_some() || self.context.store.is_uncle(hash)
    }

    fn descendant(&self, uncle: &Header) -> bool {
        let parent_hash = uncle.parent_hash();
        let uncle_number = uncle.number();
        let store = self.context.store;

        let number_continuity = |parent_hash| {
            store
                .get_block_header(parent_hash)
                .map(|parent| (parent.number() + 1) == uncle_number)
                .unwrap_or(false)
        };

        if store.get_block_number(parent_hash).is_some() {
            return number_continuity(parent_hash);
        }

        if store.is_uncle(parent_hash) {
            return number_continuity(parent_hash);
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
    block: &'a Block,
}

impl<'a, CS: ChainStore<'a>> CommitVerifier<'a, CS> {
    pub fn new(context: &'a VerifyContext<'a, CS>, block: &'a Block) -> Self {
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

        let mut proposal_txs_ids = HashSet::default();

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
                    .iter()
                    .for_each(|uncle| proposal_txs_ids.extend(uncle.proposals()));
            }

            block_hash = header.parent_hash().to_owned();
            proposal_end -= 1;
        }

        let committed_ids: HashSet<_> = self
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

pub struct RewardVerifier<'a, 'b, CS> {
    resolved: &'a [ResolvedTransaction<'a>],
    parent: &'b Header,
    context: &'a VerifyContext<'a, CS>,
}

impl<'a, 'b, CS: ChainStore<'a>> RewardVerifier<'a, 'b, CS> {
    pub fn new(
        context: &'a VerifyContext<'a, CS>,
        resolved: &'a [ResolvedTransaction],
        parent: &'b Header,
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
        if cellbase.transaction.outputs()[0].lock != target_lock {
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

struct DaoHeaderVerifier<'a, 'b, CS> {
    context: &'a VerifyContext<'a, CS>,
    resolved: &'a [ResolvedTransaction<'a>],
    parent: &'b Header,
    header: &'a Header,
}

impl<'a, 'b, CS: ChainStore<'a>> DaoHeaderVerifier<'a, 'b, CS> {
    pub fn new(
        context: &'a VerifyContext<'a, CS>,
        resolved: &'a [ResolvedTransaction<'a>],
        parent: &'b Header,
        header: &'a Header,
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

struct BlockTxsVerifier<'a, CS> {
    context: &'a VerifyContext<'a, CS>,
    block_number: BlockNumber,
    epoch_number: EpochNumber,
    parent_hash: &'a H256,
    resolved: &'a [ResolvedTransaction<'a>],
}

impl<'a, CS: ChainStore<'a>> BlockTxsVerifier<'a, CS> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        context: &'a VerifyContext<'a, CS>,
        block_number: BlockNumber,
        epoch_number: EpochNumber,
        parent_hash: &'a H256,
        resolved: &'a [ResolvedTransaction<'a>],
    ) -> Self {
        BlockTxsVerifier {
            context,
            block_number,
            epoch_number,
            parent_hash,
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
                        self.parent_hash,
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
                        self.parent_hash,
                        self.context.consensus,
                        self.context.script_config,
                        self.context.store,
                    )
                    .verify(self.context.consensus.max_block_cycles())
                    .map_err(|e| Error::Transactions((index, e)))
                    .map(|cycles| (tx_hash, cycles))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;

        let sum: Cycle = ret_set.iter().map(|(_, cycles)| cycles).sum();

        for (hash, cycles) in ret_set {
            txs_verify_cache.insert(hash, cycles);
        }

        if sum > self.context.consensus.max_block_cycles() {
            Err(Error::ExceededMaximumCycles)
        } else {
            Ok(sum)
        }
    }
}

fn prepare_epoch_ext<'a, CS: ChainStore<'a>>(
    context: &VerifyContext<'a, CS>,
    parent: &Header,
) -> Result<EpochExt, Error> {
    let parent_ext = context
        .store
        .get_block_epoch(parent.hash())
        .ok_or_else(|| Error::UnknownParent(parent.hash().clone()))?;
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
        block: &'a Block,
        txs_verify_cache: &mut LruCache<H256, Cycle>,
    ) -> Result<(Cycle, Vec<Capacity>), Error> {
        let parent_hash = block.header().parent_hash();
        let parent = self
            .context
            .store
            .get_block_header(parent_hash)
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
            block.header().number(),
            block.header().epoch(),
            block.header().parent_hash(),
            resolved,
        )
        .verify(txs_verify_cache)?;

        Ok((cycles, txs_fees))
    }
}
