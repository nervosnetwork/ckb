//! Generate a new block

mod builder;
mod candidate_uncles;
mod cell_liveness;
mod dao;
mod json;
mod notify;
mod state;

#[cfg(test)]
mod tests;

use crate::component::entry::TxEntry;
use crate::error::BlockAssemblerError;
use crate::util::block_offload;
pub(crate) use candidate_uncles::CandidateUncleSourceReceipt;
pub use candidate_uncles::CandidateUncles;
use candidate_uncles::PreparedUncles;
pub(crate) use candidate_uncles::{CandidateUncleMutationError, CandidateUnclePrune};
use cell_liveness::CellLivenessMemo;
use ckb_app_config::BlockAssemblerConfig;
use ckb_error::{AnyError, InternalErrorKind};
use ckb_jsonrpc_types::BlockTemplate as JsonBlockTemplate;
use ckb_reward_calculator::RewardCalculator;
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_systemtime::unix_time_as_millis;
use ckb_types::{
    bytes,
    core::{
        Capacity, EpochExt, ScriptHashType, TransactionBuilder, TransactionView, UncleBlockView,
    },
    packed::{
        self, Bytes, CellInput, CellOutput, CellbaseWitness, ProposalShortId, Script, Transaction,
    },
    prelude::*,
};
use ckb_util::Mutex as StdMutex;
use http_body_util::Full;
use hyper_util::client::legacy::{Client, connect::HttpConnector};
use std::collections::HashSet;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::{cmp, iter};
use tokio::sync::RwLock;

pub(crate) use builder::{BlockTemplateBuilder, BlockTemplateDraft, TemplateContentUpdate};
pub(crate) use state::{CurrentTemplate, ResetEpoch, TemplateRevision, TemplateSize};

/// Deterministic optional-content prefix compiled against one exact block-byte
/// budget. Proposals retain score order, uncles retain candidate order, and
/// only proposals that actually fit may exclude a conflicting uncle.
pub(crate) struct FittedOptionalContent {
    pub(crate) proposals: Vec<ProposalShortId>,
    pub(crate) uncles: Vec<UncleBlockView>,
    pub(crate) proposals_size: usize,
    pub(crate) uncles_size: usize,
    pub(crate) total_size: usize,
}

/// Block generator
#[derive(Clone)]
pub struct BlockAssembler {
    pub(crate) config: Arc<BlockAssemblerConfig>,
    pub(crate) work_id: Arc<AtomicU64>,
    /// Bounded optional uncle cache. Preparation clones its at-most-128-entry
    /// snapshot under this short synchronous lock and performs chain lookups
    /// after releasing it. Pruning occurs only inside a successful template
    /// publication Apply.
    pub(crate) candidate_uncles: Arc<StdMutex<CandidateUncles>>,
    /// Current template snapshot. Readers clone the inner `Arc` under a read
    /// lock; updaters build a new `CurrentTemplate` without holding the lock,
    /// then swap the `Arc` under a write lock.
    pub(crate) current: Arc<RwLock<Arc<CurrentTemplate>>>,
    /// Shared per-tip memo of chain-cell liveness for `calc_dao`. Uses a
    /// `std::sync::Mutex` because the critical sections are short and never
    /// cross `.await`.
    pub(crate) cell_liveness_memo: Arc<StdMutex<CellLivenessMemo>>,
    pub(crate) poster: Arc<Client<HttpConnector, Full<bytes::Bytes>>>,
    /// Bounded process owner for configured template-notification scripts.
    /// Each configured command has at most one live child; timeout drops and
    /// kills that child before its slot can be reused.
    script_notifier: Arc<notify::NotifyScriptRunner>,
    /// Test-only observation point for the external miner-notification
    /// boundary. Production builds pay no field or atomic-operation cost.
    #[cfg(test)]
    pub(crate) notify_count: Arc<AtomicU64>,
}

impl BlockAssembler {
    /// Construct new block generator
    pub fn new(config: BlockAssemblerConfig, snapshot: Arc<Snapshot>) -> Result<Self, AnyError> {
        let consensus = snapshot.consensus();
        let tip_header = snapshot.tip_header();
        let current_epoch = consensus
            .next_epoch_ext(tip_header, &snapshot.borrow_as_data_loader())
            .ok_or(BlockAssemblerError::MissingTipEpoch)?
            .epoch();

        let work_id = AtomicU64::new(0);
        let cell_liveness_memo = Arc::new(StdMutex::new(CellLivenessMemo::for_block_bytes(
            snapshot.consensus().max_block_bytes() as usize,
        )));
        let current = Self::build_base_template(
            &config,
            &work_id,
            snapshot,
            &current_epoch,
            vec![],
            &cell_liveness_memo,
        )?;
        let script_notifier = Arc::new(notify::NotifyScriptRunner::new(&config.notify_scripts));

        Ok(Self {
            config: Arc::new(config),
            work_id: Arc::new(work_id),
            candidate_uncles: Arc::new(StdMutex::new(CandidateUncles::new())),
            current: Arc::new(RwLock::new(Arc::new(current))),
            cell_liveness_memo,
            poster: Arc::new(
                Client::builder(hyper_util::rt::TokioExecutor::new())
                    .build::<_, Full<bytes::Bytes>>(HttpConnector::new()),
            ),
            script_notifier,
            #[cfg(test)]
            notify_count: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Build a fresh base template from `snapshot`.
    ///
    /// The returned template contains no transactions or proposals; it only has
    /// the cellbase, DAO, extension and (optionally) uncles. `uncles` is empty
    /// during initial construction and is populated by `reset_template` after a
    /// reorg. Sharing this path avoids duplicating the cellbase, extension, DAO
    /// and size calculations between `new` and `reset_template`.
    pub(crate) fn build_base_template(
        config: &BlockAssemblerConfig,
        work_id: &AtomicU64,
        snapshot: Arc<Snapshot>,
        current_epoch: &EpochExt,
        uncles: Vec<UncleBlockView>,
        memo: &StdMutex<CellLivenessMemo>,
    ) -> Result<CurrentTemplate, AnyError> {
        let tip_header = snapshot.tip_header();
        let mut draft = BlockTemplateDraft::new(&snapshot, current_epoch)?;

        let cellbase = Self::build_cellbase(config, &snapshot)?;
        let extension = Self::build_extension(&snapshot)?;
        let fixed_size =
            Self::basic_block_size(cellbase.data(), &[], iter::empty(), extension.clone());
        let optional = Self::fit_optional_content(
            &snapshot,
            Vec::new(),
            &uncles,
            fixed_size,
            snapshot.consensus().max_block_bytes() as usize,
        )?
        .ok_or(BlockAssemblerError::Overflow)?;
        let uncles = optional.uncles;
        let uncles_size = optional.uncles_size;
        let basic_block_size = optional.total_size;

        let (dao, _checked_txs, _failed_txs) =
            Self::calc_dao(&snapshot, current_epoch, cellbase.clone(), vec![], memo)?;

        draft.uncles(uncles);
        if let Some(data) = extension {
            draft.extension(data);
        }
        let template = draft.build(
            cellbase,
            Self::take_counter(work_id, "work id")?,
            dao,
            cmp::max(
                unix_time_as_millis(),
                tip_header
                    .timestamp()
                    .checked_add(1)
                    .ok_or(BlockAssemblerError::Overflow)?,
            ),
        );

        let size = TemplateSize {
            txs: 0,
            proposals: 0,
            uncles: uncles_size,
            total: basic_block_size,
        };

        Ok(CurrentTemplate {
            template,
            size,
            snapshot,
            epoch: current_epoch.clone(),
            revision: TemplateRevision::INITIAL,
            reset_epoch: ResetEpoch::INITIAL,
        })
    }

    pub(crate) fn take_counter(
        counter: &AtomicU64,
        label: &'static str,
    ) -> Result<u64, BlockAssemblerError> {
        counter
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            })
            .map_err(|_| BlockAssemblerError::CounterExhausted(label))
    }

    /// Compile the one optional-content policy shared by reset, full and both
    /// optimistic component lanes. Proposal liveness has byte priority; only
    /// the selected prefix participates in uncle-conflict filtering, then the
    /// ordered compatible uncle prefix consumes the remainder. Returning
    /// `None` means the mandatory cellbase/extension/transaction base already
    /// exceeds the consensus byte limit.
    pub(crate) fn fit_optional_content(
        snapshot: &Snapshot,
        mut proposals: Vec<ProposalShortId>,
        prepared_uncles: &[UncleBlockView],
        base_total_size: usize,
        max_block_bytes: usize,
    ) -> Result<Option<FittedOptionalContent>, BlockAssemblerError> {
        let Some((proposals_size, proposals_total)) =
            Self::fit_proposal_prefix(&mut proposals, base_total_size, max_block_bytes)
        else {
            return Ok(None);
        };
        let proposal_set = proposals.iter().cloned().collect::<HashSet<_>>();
        let mut uncles = Self::filter_uncles_conflicting_with_proposals(
            snapshot,
            prepared_uncles,
            &proposal_set,
        );
        let Some((uncles_size, total_size)) =
            Self::fit_uncle_prefix_after_base(&mut uncles, proposals_total, max_block_bytes)
        else {
            return Ok(None);
        };
        Ok(Some(FittedOptionalContent {
            proposals,
            uncles,
            proposals_size,
            uncles_size,
            total_size,
        }))
    }

    /// Keep the highest-scored proposal prefix that fits the remaining block
    /// bytes. Returning `None` means the non-proposal template already exceeds
    /// the limit. Exact fits are valid.
    pub(crate) fn fit_proposal_prefix(
        proposals: &mut Vec<ProposalShortId>,
        base_total_size: usize,
        max_block_bytes: usize,
    ) -> Option<(usize, usize)> {
        let available = max_block_bytes.checked_sub(base_total_size)?;
        let id_size = ProposalShortId::serialized_size();
        let fit_count = available.checked_div(id_size)?.min(proposals.len());
        proposals.truncate(fit_count);
        let proposals_size = fit_count.checked_mul(id_size)?;
        Some((proposals_size, base_total_size.checked_add(proposals_size)?))
    }

    fn fit_uncle_prefix_after_base(
        uncles: &mut Vec<UncleBlockView>,
        base_total_size: usize,
        max_block_bytes: usize,
    ) -> Option<(usize, usize)> {
        let available = max_block_bytes.checked_sub(base_total_size)?;
        let mut fit_count = 0usize;
        let mut uncles_size = 0usize;
        for uncle in uncles.iter() {
            let next_size = uncles_size.checked_add(Self::uncle_size(uncle).ok()?)?;
            if next_size > available {
                break;
            }
            uncles_size = next_size;
            fit_count = fit_count.checked_add(1)?;
        }
        uncles.truncate(fit_count);
        Some((uncles_size, base_total_size.checked_add(uncles_size)?))
    }

    pub(crate) async fn get_current(&self) -> JsonBlockTemplate {
        // Only clone the inner Arc while holding the read lock; the lock is
        // released immediately after this statement.
        let current = self.current.read().await.clone();
        (&current.template).into()
    }

    pub(crate) fn build_cellbase_witness(
        config: &BlockAssemblerConfig,
        snapshot: &Snapshot,
    ) -> CellbaseWitness {
        let hash_type: ScriptHashType = config.hash_type.into();
        let cellbase_lock = Script::new_builder()
            .args(config.args.as_bytes())
            .code_hash(&config.code_hash)
            .hash_type(hash_type)
            .build();
        let tip = snapshot.tip_header();

        let mut message = vec![];
        if let Some(version) = snapshot.compute_versionbits(tip) {
            message.extend_from_slice(&version.to_le_bytes());
            message.extend_from_slice(b" ");
        }
        if config.use_binary_version_as_message_prefix {
            message.extend_from_slice(config.binary_version.as_bytes());
        }
        if !config.message.is_empty() {
            message.extend_from_slice(b" ");
            message.extend_from_slice(config.message.as_bytes());
        }

        CellbaseWitness::new_builder()
            .lock(cellbase_lock)
            .message(message)
            .build()
    }

    /// Miner mined block H(c), the block reward will be finalized at H(c + w_far + 1).
    /// Miner specify own lock in cellbase witness.
    /// The cellbase have only one output,
    /// miner should collect the block reward for finalize target H(max(0, c - w_far - 1))
    pub(crate) fn build_cellbase(
        config: &BlockAssemblerConfig,
        snapshot: &Snapshot,
    ) -> Result<TransactionView, AnyError> {
        let tip = snapshot.tip_header();
        let candidate_number = tip
            .number()
            .checked_add(1)
            .ok_or(BlockAssemblerError::Overflow)?;
        let cellbase_witness = Self::build_cellbase_witness(config, snapshot);

        let tx = {
            let (target_lock, block_reward) = block_offload(|| {
                RewardCalculator::new(snapshot.consensus(), snapshot).block_reward_to_finalize(tip)
            })?;
            let input = CellInput::new_cellbase_input(candidate_number);
            let output = CellOutput::new_builder()
                .capacity(block_reward.total)
                .lock(target_lock)
                .build();

            let witness = cellbase_witness.as_bytes();
            let no_finalization_target =
                candidate_number <= snapshot.consensus().finalization_delay_length();
            let tx_builder = TransactionBuilder::default().input(input).witness(witness);
            let insufficient_reward_to_create_cell = output.is_lack_of_capacity(Capacity::zero())?;
            if no_finalization_target || insufficient_reward_to_create_cell {
                tx_builder.build()
            } else {
                tx_builder
                    .output(output)
                    .output_data(Bytes::default())
                    .build()
            }
        };

        Ok(tx)
    }

    pub(crate) fn build_extension(snapshot: &Snapshot) -> Result<Option<packed::Bytes>, AnyError> {
        let tip_header = snapshot.tip_header();
        // The use of the epoch number of the tip here leads to an off-by-one bug,
        // so be careful, it needs to be preserved for consistency reasons and not fixed directly.
        let mmr_activate = snapshot
            .consensus()
            .rfc0044_active(tip_header.epoch().number());
        if mmr_activate {
            let chain_root = snapshot
                .chain_root_mmr(tip_header.number())
                .get_root()
                .map_err(|e| InternalErrorKind::MMR.other(e))?;
            let bytes = chain_root.calc_mmr_hash().as_bytes().into();
            Ok(Some(bytes))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn prepare_uncles(
        &self,
        snapshot: &Snapshot,
        current_epoch: &EpochExt,
    ) -> PreparedUncles {
        // Chain lookups run on a detached bounded copy. A stale optimistic
        // build therefore cannot mutate the live candidate cache.
        let candidates = self.candidate_uncles.lock().clone();
        candidates.prepare_uncles(snapshot, current_epoch)
    }

    /// Keep proposal selection live even when miners omit optional uncles.
    ///
    /// Pending transactions are selected as top-level proposals first. Any
    /// template uncle carrying one of those short ids is omitted from this
    /// template, because otherwise `package_proposals` would have to suppress
    /// the id and a miner that drops the uncle could repeat that suppression
    /// indefinitely. Descendants of an omitted uncle are also omitted unless
    /// their parent is independently known on the main chain or as an already
    /// embedded uncle.
    pub(crate) fn filter_uncles_conflicting_with_proposals(
        snapshot: &Snapshot,
        uncles: &[UncleBlockView],
        proposals: &HashSet<ProposalShortId>,
    ) -> Vec<UncleBlockView> {
        let mut included_hashes = HashSet::with_capacity(uncles.len());
        let mut compatible = Vec::with_capacity(uncles.len());

        for uncle in uncles {
            let conflicts = uncle
                .data()
                .proposals()
                .into_iter()
                .any(|id| proposals.contains(&id));
            if conflicts {
                continue;
            }

            let parent_hash = uncle.header().parent_hash();
            if snapshot.is_main_chain(&parent_hash)
                || snapshot.is_uncle(&parent_hash)
                || included_hashes.contains(&parent_hash)
            {
                included_hashes.insert(uncle.hash());
                compatible.push(uncle.clone());
            }
        }

        compatible
    }

    /// The byte contribution of one uncle on the template's accounting
    /// basis (`serialized_size_without_uncle_proposals`, the same basis as
    /// `basic_block_size` and the consensus block-bytes limit): the packed
    /// in-block size minus its proposal ids.
    fn uncle_size(uncle: &UncleBlockView) -> Result<usize, BlockAssemblerError> {
        let proposal_bytes = uncle
            .data()
            .proposals()
            .len()
            .checked_mul(ProposalShortId::serialized_size())
            .ok_or(BlockAssemblerError::Overflow)?;
        UncleBlockView::serialized_size_in_block()
            .checked_sub(proposal_bytes)
            .ok_or(BlockAssemblerError::Overflow)
    }

    pub(crate) fn basic_block_size<'a>(
        cellbase: Transaction,
        uncles: &[UncleBlockView],
        proposals: impl Iterator<Item = &'a ProposalShortId>,
        extension_opt: Option<packed::Bytes>,
    ) -> usize {
        let empty_dao = packed::Byte32::default();
        let raw_header = packed::RawHeader::new_builder().dao(empty_dao).build();
        let header = packed::Header::new_builder().raw(raw_header).build();
        let block = if let Some(extension) = extension_opt {
            packed::BlockV1::new_builder()
                .header(header)
                .transactions(vec![cellbase])
                .uncles(uncles.iter().map(|u| u.data()).collect::<Vec<_>>())
                .proposals(proposals.cloned().collect::<Vec<_>>())
                .extension(extension)
                .build()
                .as_v0()
        } else {
            packed::Block::new_builder()
                .header(header)
                .transactions(vec![cellbase])
                .uncles(uncles.iter().map(|u| u.data()).collect::<Vec<_>>())
                .proposals(proposals.cloned().collect::<Vec<_>>())
                .build()
        };
        block.serialized_size_without_uncle_proposals()
    }

    pub(crate) fn checked_entries_size(entries: &[TxEntry]) -> Result<usize, BlockAssemblerError> {
        entries.iter().try_fold(0usize, |sum, tx| {
            sum.checked_add(tx.size)
                .ok_or(BlockAssemblerError::Overflow)
        })
    }
}
