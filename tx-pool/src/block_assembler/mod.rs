mod candidate_uncles;

use crate::component::entry::TxEntry;
use crate::error::BlockAssemblerError as Error;
pub use candidate_uncles::CandidateUncles;
use ckb_app_config::BlockAssemblerConfig;
use ckb_chain_spec::consensus::Consensus;
use ckb_jsonrpc_types::{BlockTemplate, CellbaseTemplate, TransactionTemplate, UncleTemplate};
use ckb_reward_calculator::RewardCalculator;
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_types::{
    bytes::Bytes,
    core::{
        BlockNumber, Capacity, Cycle, EpochExt, HeaderView, TransactionBuilder, TransactionView,
        UncleBlockView, Version,
    },
    packed::{self, Byte32, CellInput, CellOutput, CellbaseWitness, ProposalShortId, Transaction},
    prelude::*,
};
use failure::Error as FailureError;
use lru::LruCache;
use std::collections::HashSet;
use std::sync::{atomic::AtomicU64, Arc};
use tokio::sync::Mutex;

const BLOCK_TEMPLATE_TIMEOUT: u64 = 3000;
const TEMPLATE_CACHE_SIZE: usize = 10;

pub struct TemplateCache {
    pub time: u64,
    pub uncles_updated_at: u64,
    pub txs_updated_at: u64,
    pub template: BlockTemplate,
}

impl TemplateCache {
    pub fn is_outdate(&self, current_time: u64) -> bool {
        current_time.saturating_sub(self.time) > BLOCK_TEMPLATE_TIMEOUT
    }

    pub fn is_modified(&self, last_uncles_updated_at: u64, last_txs_updated_at: u64) -> bool {
        last_uncles_updated_at != self.uncles_updated_at
            || last_txs_updated_at != self.txs_updated_at
    }
}

pub type BlockTemplateCacheKey = (Byte32, Cycle, u64, Version);

#[derive(Clone)]
pub struct BlockAssembler {
    pub(crate) config: Arc<BlockAssemblerConfig>,
    pub(crate) work_id: Arc<AtomicU64>,
    pub(crate) last_uncles_updated_at: Arc<AtomicU64>,
    pub(crate) template_caches: Arc<Mutex<LruCache<BlockTemplateCacheKey, TemplateCache>>>,
    pub(crate) candidate_uncles: Arc<Mutex<CandidateUncles>>,
}

impl BlockAssembler {
    pub fn new(config: BlockAssemblerConfig) -> Self {
        Self {
            config: Arc::new(config),
            work_id: Arc::new(AtomicU64::new(0)),
            last_uncles_updated_at: Arc::new(AtomicU64::new(0)),
            template_caches: Arc::new(Mutex::new(LruCache::new(TEMPLATE_CACHE_SIZE))),
            candidate_uncles: Arc::new(Mutex::new(CandidateUncles::new())),
        }
    }

    pub(crate) fn transform_params(
        consensus: &Consensus,
        bytes_limit: Option<u64>,
        proposals_limit: Option<u64>,
        max_version: Option<Version>,
    ) -> (u64, u64, Version) {
        let bytes_limit = bytes_limit
            .min(Some(consensus.max_block_bytes()))
            .unwrap_or_else(|| consensus.max_block_bytes());
        let proposals_limit = proposals_limit
            .min(Some(consensus.max_block_proposals_limit()))
            .unwrap_or_else(|| consensus.max_block_proposals_limit());
        let version = max_version
            .min(Some(consensus.block_version()))
            .unwrap_or_else(|| consensus.block_version());

        (bytes_limit, proposals_limit, version)
    }

    pub(crate) fn transform_uncle(uncle: &UncleBlockView) -> UncleTemplate {
        UncleTemplate {
            hash: uncle.hash().unpack(),
            required: false,
            proposals: uncle
                .data()
                .proposals()
                .into_iter()
                .map(Into::into)
                .collect(),
            header: uncle.data().header().into(),
        }
    }

    pub(crate) fn transform_cellbase(
        tx: &TransactionView,
        cycles: Option<Cycle>,
    ) -> CellbaseTemplate {
        CellbaseTemplate {
            hash: tx.hash().unpack(),
            cycles: cycles.map(Into::into),
            data: tx.data().into(),
        }
    }

    pub(crate) fn transform_tx(
        tx: &TxEntry,
        required: bool,
        depends: Option<Vec<u32>>,
    ) -> TransactionTemplate {
        TransactionTemplate {
            hash: tx.transaction.hash().unpack(),
            required,
            cycles: Some(tx.cycles.into()),
            depends: depends.map(|deps| deps.into_iter().map(|x| u64::from(x).into()).collect()),
            data: tx.transaction.data().into(),
        }
    }

    pub(crate) fn calculate_txs_size_limit(
        bytes_limit: u64,
        cellbase: Transaction,
        uncles: &[UncleBlockView],
        proposals: &HashSet<ProposalShortId>,
    ) -> Result<usize, FailureError> {
        let empty_dao = packed::Byte32::default();
        let raw_header = packed::RawHeader::new_builder().dao(empty_dao).build();
        let header = packed::Header::new_builder().raw(raw_header).build();
        let block = packed::Block::new_builder()
            .header(header)
            .transactions(vec![cellbase].pack())
            .uncles(uncles.iter().map(|u| u.data()).pack())
            .proposals(
                proposals
                    .iter()
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
                    .pack(),
            )
            .build();
        let serialized_size = block.serialized_size_without_uncle_proposals();
        let bytes_limit = bytes_limit as usize;
        bytes_limit
            .checked_sub(serialized_size)
            .ok_or_else(|| Error::InvalidParams(format!("bytes_limit {}", bytes_limit)).into())
    }

    /// Miner mined block H(c), the block reward will be finalized at H(c + w_far + 1).
    /// Miner specify own lock in cellbase witness.
    /// The cellbase have only one output,
    /// miner should collect the block reward for finalize target H(max(0, c - w_far - 1))
    pub(crate) fn build_cellbase(
        snapshot: &Snapshot,
        tip: &HeaderView,
        cellbase_witness: CellbaseWitness,
    ) -> Result<TransactionView, FailureError> {
        let candidate_number = tip.number() + 1;

        let tx = {
            let (target_lock, block_reward) = RewardCalculator::new(snapshot.consensus(), snapshot)
                .block_reward_to_finalize(tip)?;
            let input = CellInput::new_cellbase_input(candidate_number);
            let output = CellOutput::new_builder()
                .capacity(block_reward.total.pack())
                .lock(target_lock)
                .build();

            let witness = cellbase_witness.as_bytes().pack();
            let no_finalization_target =
                candidate_number <= snapshot.consensus().finalization_delay_length();
            let tx_builder = TransactionBuilder::default().input(input).witness(witness);
            let insufficient_reward_to_create_cell = output.is_lack_of_capacity(Capacity::zero())?;
            if no_finalization_target || insufficient_reward_to_create_cell {
                tx_builder.build()
            } else {
                tx_builder
                    .output(output)
                    .output_data(Bytes::default().pack())
                    .build()
            }
        };

        Ok(tx)
    }

    // A block B1 is considered to be the uncle of another block B2 if all of the following conditions are met:
    // (1) they are in the same epoch, sharing the same difficulty;
    // (2) height(B2) > height(B1);
    // (3) B1's parent is either B2's ancestor or embedded in B2 or its ancestors as an uncle;
    // and (4) B2 is the first block in its chain to refer to B1.
    pub(crate) fn prepare_uncles(
        snapshot: &Snapshot,
        candidate_number: BlockNumber,
        current_epoch_ext: &EpochExt,
        candidate_uncles: &mut CandidateUncles,
    ) -> Vec<UncleBlockView> {
        let epoch_number = current_epoch_ext.number();
        let max_uncles_num = snapshot.consensus().max_uncles_num();
        let mut uncles: Vec<UncleBlockView> = Vec::with_capacity(max_uncles_num);
        let mut removed = Vec::new();

        for uncle in candidate_uncles.values() {
            if uncles.len() == max_uncles_num {
                break;
            }
            let parent_hash = uncle.header().parent_hash();
            if uncle.compact_target() != current_epoch_ext.compact_target()
                || uncle.epoch().number() != epoch_number
                || snapshot.get_block_number(&uncle.hash()).is_some()
                || snapshot.is_uncle(&uncle.hash())
                || !(uncles.iter().any(|u| u.hash() == parent_hash)
                    || snapshot.get_block_number(&parent_hash).is_some()
                    || snapshot.is_uncle(&parent_hash))
                || uncle.number() >= candidate_number
            {
                removed.push(uncle.clone());
            } else {
                uncles.push(uncle.clone());
            }
        }

        for r in removed {
            candidate_uncles.remove(&r);
        }
        uncles
    }
}
