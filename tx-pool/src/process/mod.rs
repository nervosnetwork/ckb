use crate::component::pool_map::{PoolMap, Status};
use crate::component::pre_pool::{PrePoolError, PrePoolGeneration, PrePoolKernel};
use crate::constants::{GAP_PROPOSAL_INDEX, PROPOSED_PROPOSAL_INDEX};
use crate::error::Reject;
use crate::pool::TxPool;
use crate::service::effects::{EffectBatch, EffectBuildError, EffectJournalError, TxPoolEffect};
use crate::service::{BlockAssemblerMessage, TxPoolService, TxVerificationResult};
use crate::tx_source::TxSource;
use crate::util::non_contextual_verify;
use ckb_error::{AnyError, InternalErrorKind};
use ckb_fee_estimator::FeeEstimator;
use ckb_jsonrpc_types::BlockTemplate;
use ckb_logger::{debug, error, info};
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_types::packed::OutPoint;
use ckb_types::{
    core::{
        BlockView, Capacity, EstimateMode, FeeRate, HeaderView, TransactionView, UncleBlockView,
        cell::ResolvedTransaction,
    },
    packed::{Byte32, ProposalShortId},
};
use ckb_util::LinkedHashSet;
use ckb_verification::{TxVerifyEnv, cache::Completed};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tokio::sync::mpsc;

mod classify;
mod post_process;
pub(crate) mod reorg;
pub(crate) mod submit;

pub(crate) enum TxPoolGenerationFault {
    PrePool(crate::component::pre_pool::PrePoolFault),
    AcceptedPool(crate::component::pool_map::PoolMutationFault),
    #[cfg(feature = "internal")]
    Selection(crate::component::tx_selector::TxSelectionError),
    Reorg(ReorgUpdateError),
    Commit(submit::PipelineCommitFault),
    Epoch(crate::service::PipelineEpochExhausted),
    Effect(EffectJournalError),
}

impl std::fmt::Debug for TxPoolGenerationFault {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PrePool(error) => formatter.debug_tuple("PrePool").field(error).finish(),
            Self::AcceptedPool(error) => {
                formatter.debug_tuple("AcceptedPool").field(error).finish()
            }
            #[cfg(feature = "internal")]
            Self::Selection(error) => formatter.debug_tuple("Selection").field(error).finish(),
            Self::Reorg(error) => formatter.debug_tuple("Reorg").field(error).finish(),
            Self::Commit(error) => formatter.debug_tuple("Commit").field(error).finish(),
            Self::Epoch(error) => formatter.debug_tuple("Epoch").field(error).finish(),
            Self::Effect(error) => formatter.debug_tuple("Effect").field(error).finish(),
        }
    }
}

/// A list for plug target for `plug_entry` method
pub enum PlugTarget {
    /// Pending pool
    Pending,
    /// Proposed pool
    Proposed,
}

enum ReorgDraft {
    Queued,
    Reset(Vec<TransactionView>),
    Fault(ReorgMutationFault),
}

struct RetiredReorgGeneration {
    accepted: PoolMap,
    kernel: PrePoolGeneration,
}

impl RetiredReorgGeneration {
    fn dispose(self) {
        drop((self.accepted, self.kernel));
    }
}

enum ReorgMutation {
    Queued,
    Reset(RetiredReorgGeneration),
    Fault {
        error: ReorgMutationFault,
        retired: RetiredReorgGeneration,
    },
}

enum ReorgFallback {
    Retain(Vec<TransactionView>),
    Empty,
}

enum ReorgMutationFault {
    Sort(DependencySortError),
    Effect(EffectBuildError),
    Kernel(crate::component::pre_pool::PrePoolFault),
}

impl From<DependencySortError> for ReorgMutationFault {
    fn from(error: DependencySortError) -> Self {
        Self::Sort(error)
    }
}

impl From<EffectBuildError> for ReorgMutationFault {
    fn from(error: EffectBuildError) -> Self {
        Self::Effect(error)
    }
}

impl From<crate::component::pre_pool::PrePoolFault> for ReorgMutationFault {
    fn from(error: crate::component::pre_pool::PrePoolFault) -> Self {
        Self::Kernel(error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DependencySortError {
    Arithmetic(&'static str),
    Projection(&'static str),
}

impl std::fmt::Display for DependencySortError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Arithmetic(context) => {
                write!(formatter, "dependency-sort arithmetic overflow: {context}")
            }
            Self::Projection(context) => {
                write!(formatter, "dependency-sort projection drift: {context}")
            }
        }
    }
}

impl std::error::Error for DependencySortError {}

#[derive(Debug)]
pub(crate) enum ReorgUpdateError {
    Effect(EffectJournalError),
    Epoch(crate::service::PipelineEpochExhausted),
    Sort(DependencySortError),
    Kernel(crate::component::pre_pool::PrePoolFault),
    EffectBuild(EffectBuildError),
}

impl std::fmt::Display for ReorgUpdateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Effect(error) => write!(formatter, "reorg effect journal failed: {error:?}"),
            Self::Epoch(error) => write!(formatter, "reorg epoch unavailable: {error}"),
            Self::Sort(error) => write!(formatter, "reorg dependency sorting failed: {error}"),
            Self::Kernel(error) => write!(formatter, "reorg kernel reset failed: {error:?}"),
            Self::EffectBuild(error) => {
                write!(formatter, "reorg effect construction failed: {error:?}")
            }
        }
    }
}

impl std::error::Error for ReorgUpdateError {}

impl From<EffectJournalError> for ReorgUpdateError {
    fn from(error: EffectJournalError) -> Self {
        Self::Effect(error)
    }
}

impl From<crate::service::PipelineEpochExhausted> for ReorgUpdateError {
    fn from(error: crate::service::PipelineEpochExhausted) -> Self {
        Self::Epoch(error)
    }
}

impl From<DependencySortError> for ReorgUpdateError {
    fn from(error: DependencySortError) -> Self {
        Self::Sort(error)
    }
}

impl From<crate::component::pre_pool::PrePoolFault> for ReorgUpdateError {
    fn from(error: crate::component::pre_pool::PrePoolFault) -> Self {
        Self::Kernel(error)
    }
}

impl From<EffectBuildError> for ReorgUpdateError {
    fn from(error: EffectBuildError) -> Self {
        Self::EffectBuild(error)
    }
}

impl From<ReorgMutationFault> for ReorgUpdateError {
    fn from(error: ReorgMutationFault) -> Self {
        match error {
            ReorgMutationFault::Sort(error) => Self::Sort(error),
            ReorgMutationFault::Effect(error) => Self::EffectBuild(error),
            ReorgMutationFault::Kernel(error) => Self::Kernel(error),
        }
    }
}

/// Map a pool status to the verification environment used for contextual checks.
fn status_to_verify_env(status: Status, header: &HeaderView) -> TxVerifyEnv {
    match status {
        Status::Pending => TxVerifyEnv::new_submit(header),
        Status::Gap => TxVerifyEnv::new_proposed(header, GAP_PROPOSAL_INDEX),
        Status::Proposed => TxVerifyEnv::new_proposed(header, PROPOSED_PROPOSAL_INDEX),
    }
}

/// Map a pool status to the block-assembler notification that should be sent
/// when a transaction reaches this status.
fn status_to_block_assembler_message(status: Status) -> Option<BlockAssemblerMessage> {
    match status {
        Status::Pending => Some(BlockAssemblerMessage::Pending),
        Status::Proposed => Some(BlockAssemblerMessage::Proposed),
        Status::Gap => None,
    }
}

impl TxPoolService {
    /// The reject reason used when an in-flight candidate is superseded by a
    /// higher-fee-rate one. One constant so the register-time and
    /// submit-time paths produce the identical message.
    pub(crate) const SUPERSEDED_BY_HIGHER_FEE_CANDIDATE: &'static str =
        "superseded by higher-fee-rate in-flight candidate";

    /// Converge every externally inferred relay state before stopping a
    /// generation whose typed authority proof failed. The reset register is
    /// allocation-free and capacity-independent; installing it before the
    /// cancellation edge avoids a shutdown race in which an internal fault is
    /// accidentally published as a transaction/peer rejection.
    pub(crate) fn fail_tx_pool_generation(
        &self,
        context: &'static str,
        fault: &TxPoolGenerationFault,
    ) {
        if let Err(reset_error) = self.relay.effects.install_generation_reset()
            && reset_error != crate::service::effects::EffectJournalError::Closed
        {
            ckb_logger::error!(
                "failed to install relayer reset before pipeline fault: {reset_error:?}"
            );
        }
        self.pipeline.kernel.report_fault(context, fault);
    }

    fn wake_block_assembler(&self, message: BlockAssemblerMessage, saturated: &'static str) {
        if let Err(error) = self.relay.block_assembler_sender.try_send(message) {
            match error {
                mpsc::error::TrySendError::Full(_) => debug!("{saturated}"),
                mpsc::error::TrySendError::Closed(_) => {
                    error!("block_assembler receiver dropped")
                }
            }
        }
    }

    pub(crate) async fn get_block_template(&self) -> Result<BlockTemplate, AnyError> {
        if let Some(ref block_assembler) = self.block_assembler {
            Ok(block_assembler.get_current().await)
        } else {
            Err(InternalErrorKind::Config
                .other("BlockAssembler disabled")
                .into())
        }
    }

    /// Record the assembler delta synchronously at the same linearization
    /// point as the pool mutation. The channel is only a wake edge; the dirty
    /// bit is the level-triggered authority and therefore cannot be left to a
    /// cancellable post-commit future.
    pub(crate) fn journal_block_assembler_update(&self, status: Status) {
        if let Some(message) = status_to_block_assembler_message(status) {
            self.journal_block_assembler_message(message);
        }
    }

    /// Record a level-triggered assembler delta before issuing a best-effort
    /// wake edge. This is shared by pool status and candidate-uncle changes;
    /// neither producer may await bounded channel capacity after mutation.
    pub(crate) fn journal_block_assembler_message(&self, message: BlockAssemblerMessage) {
        if !self.should_notify_block_assembler() {
            return;
        }
        if let Err(error) = self.relay.mark_block_assembler_dirty(&message) {
            // This path can run inside an effect-journal Apply closure. Do not
            // call `fail_tx_pool_generation` here: installing another relay
            // reset would re-enter that journal lock.
            self.pipeline
                .kernel
                .report_fault("block-assembler dirty generation exhausted", &error);
            return;
        }
        self.wake_block_assembler(
            message,
            "block_assembler channel full; dirty update retained",
        );
    }

    /// Preserve the original block-assembler priority model: `update_full`
    /// wins unconditionally, while proposal/transaction deltas remain
    /// optimistic. A successful full swap reissues both delta generations so
    /// work acknowledged just before the swap is reconciled once more.
    pub(crate) fn journal_block_assembler_full_reconcile(&self) {
        if !self.should_notify_block_assembler() {
            return;
        }
        if let Err(error) = self.relay.mark_block_assembler_full_reconcile() {
            // May be called while the effect journal is exclusively borrowed.
            self.pipeline
                .kernel
                .report_fault("block-assembler reconcile generation exhausted", &error);
            return;
        }
        for message in [
            BlockAssemblerMessage::Pending,
            BlockAssemblerMessage::Proposed,
        ] {
            self.wake_block_assembler(
                message,
                "block_assembler channel full; post-full reconcile retained",
            );
        }
    }

    /// Journal a management reset at the pool-mutation linearization point.
    /// The bounded channel carries only a wake token; the latest snapshot is
    /// retained here and therefore survives channel saturation/cancellation.
    pub(crate) fn journal_block_assembler_reset(&self, snapshot: Arc<Snapshot>) {
        if !self.should_notify_block_assembler() {
            return;
        }
        if let Err(error) = self.relay.mark_block_assembler_reset(snapshot) {
            // Reorg/clear invoke this from their effect-journal Apply closure.
            self.pipeline
                .kernel
                .report_fault("block-assembler reset generation exhausted", &error);
            return;
        }
        self.wake_block_assembler(
            BlockAssemblerMessage::Reset,
            "block_assembler channel full; reset retained",
        );
    }

    /// Read-lock the pool and return the result along with a cloned snapshot.
    pub(crate) async fn read_tx_pool_with_snapshot<U, F: FnMut(&TxPool, Arc<Snapshot>) -> U>(
        &self,
        mut f: F,
    ) -> (U, Arc<Snapshot>) {
        let tx_pool = self.pool.tx_pool.read().await;
        let snapshot = tx_pool.cloned_snapshot();

        let ret = f(&tx_pool, Arc::clone(&snapshot));
        (ret, snapshot)
    }

    pub(crate) async fn non_contextual_verify(&self, tx: &TransactionView) -> Result<(), Reject> {
        // Malformed peer banning lives in the after_process remote-reject
        // path (`handle_remote_reject`); keeping it out of this shared
        // check keeps the ban decision in exactly one place.
        non_contextual_verify(&self.pool.consensus, tx)
    }

    /// Common pre-flight checks shared by all transaction submission paths.
    ///
    /// Runs non-contextual verification and rejects duplicates already owned
    /// by either the accepted pool or any pre-pool location.
    pub(crate) async fn check_tx_basic_validity(&self, tx: &TransactionView) -> Result<(), Reject> {
        self.non_contextual_verify(tx).await?;

        let dup = || Reject::Duplicated(tx.hash());
        // Membership and pre-pool ownership are checked in the universal
        // TxPool -> kernel order. This rejects replay of already accepted
        // transactions before it can consume bounded pipeline residency/CPU,
        // while remaining gap-free across a concurrent commit handoff.
        let duplicate = {
            let pool = self.pool.tx_pool.read().await;
            let accepted = pool.get_tx_from_pool_by_hash(&tx.hash()).is_some();
            accepted
                || self
                    .pipeline
                    .kernel
                    .read(|kernel| kernel.contains_hash(&tx.hash()))
        };
        if duplicate {
            return Err(dup());
        }

        Ok(())
    }

    pub(crate) async fn submit_remote_tx(
        &self,
        tx: TransactionView,
        source: TxSource,
    ) -> Result<bool, Reject> {
        // Preflight failures go through after_process too: the remote terminal
        // policy (malformed-peer ban, eligible relayer cleanup, and
        // recent-reject history) must apply here exactly like it does to
        // failures deeper in the pipeline. Malformed rejects are intentionally
        // not retryable relayer events, but they still ban and record.
        if let Err(reject) = self.check_tx_basic_validity(&tx).await {
            // `Duplicated` covers both an already-accepted pool entry and an
            // unverified kernel owner. Only the former is a definitive
            // success; acknowledging the latter before verification would let
            // an invalid first witness make another peer accept the raw hash.
            if matches!(reject, Reject::Duplicated(_))
                && let Some(peer) = source.peer()
            {
                match self
                    .publish_accepted_relay_result(tx.hash(), Some(peer))
                    .await
                {
                    Ok(true) => return Err(reject),
                    Ok(false) => {}
                    Err(EffectJournalError::Closed) => {
                        return Err(Self::stale_pipeline_reject());
                    }
                    Err(error) => {
                        self.fail_tx_pool_generation(
                            "accepted duplicate relay publication failed",
                            &TxPoolGenerationFault::Effect(error),
                        );
                        return Err(Reject::Internal(
                            "tx-pool relay publication generation failed".to_owned(),
                        ));
                    }
                }
            }
            self.reject_with_after_process(tx, source, reject.clone())
                .await;
            return Err(reject);
        }
        self.classify_and_enqueue_tx_spawn(tx, source).await
    }

    pub(crate) async fn notify_tx(&self, tx: TransactionView) -> Result<bool, Reject> {
        // Proposal is a trusted source promotion, not an ordinary duplicate.
        // Admission must reach the kernel so an existing Remote owner is
        // upgraded in place (priority and peer budget) while immutable ingress
        // attribution remains available for later peer revocation and any
        // active versioned lease remains valid.
        if let Err(reject) = self.non_contextual_verify(&tx).await {
            self.reject_with_after_process(tx, TxSource::Proposal, reject.clone())
                .await;
            return Err(reject);
        }
        // If the same raw hash is already owned with another witness, the
        // admission adapter atomically installs this trusted payload, expires
        // the old worker lease and restarts normal bounded processing. Script
        // verification therefore never runs serially on this dispatcher.
        self.classify_and_enqueue_tx_spawn(tx, TxSource::Proposal)
            .await
    }

    pub(crate) async fn process_tx(
        &self,
        tx: TransactionView,
        source: TxSource,
    ) -> Result<Completed, Reject> {
        // Local RPC is deliberately synchronous. A matching remote/proposal
        // kernel entry must not turn it into an asynchronous duplicate:
        // run the authoritative checks directly and let a successful pool
        // commit invalidate the older kernel lease.
        if let Err(reject) = self.non_contextual_verify(&tx).await {
            self.reject_with_after_process(tx, source, reject.clone())
                .await;
            return Err(reject);
        }

        match self
            .process_tx_direct_outcome(tx.clone(), source, None)
            .await
        {
            Ok(submit::VerifySubmitOutcome::Committed(completed)) => Ok(completed),
            Ok(submit::VerifySubmitOutcome::Cleared) => Err(Self::stale_pipeline_reject()),
            Err(submit::SubmissionError::Rejected(reject)) => {
                // A fresh commit journals its Ok relay result inside the
                // authoritative pool transaction. Only typed pre-commit
                // transaction failures enter after-process policy.
                self.after_process(tx, source, &reject).await;
                Err(reject)
            }
            Err(submit::SubmissionError::Fault(fault)) => {
                self.fail_tx_pool_generation(
                    "direct transaction processing failed",
                    &TxPoolGenerationFault::Commit(fault),
                );
                Err(Reject::Internal(
                    "tx-pool transaction processing generation failed".to_owned(),
                ))
            }
        }
    }

    pub(crate) async fn send_result_to_relayer(&self, result: TxVerificationResult) {
        self.publish_relay_result(result).await;
    }
    pub(crate) fn sort_txs_by_dependencies(
        txs: &mut Vec<TransactionView>,
    ) -> Result<(), DependencySortError> {
        Self::sort_by_dependencies(txs, |tx| tx)
    }

    /// Topologically sort a list of items that wrap transactions so that
    /// parents are placed before their children. `tx_of` extracts the
    /// transaction reference from each item.
    pub(crate) fn sort_by_dependencies<T>(
        items: &mut Vec<T>,
        tx_of: impl Fn(&T) -> &TransactionView,
    ) -> Result<(), DependencySortError> {
        if items.len() <= 1 {
            return Ok(());
        }

        let mut output_to_index: HashMap<OutPoint, usize> =
            HashMap::with_capacity(items.len().saturating_mul(2));
        for (i, item) in items.iter().enumerate() {
            let tx_hash = tx_of(item).hash();
            for idx in 0..tx_of(item).outputs().len() {
                let output_index = u32::try_from(idx)
                    .map_err(|_| DependencySortError::Arithmetic("transaction output index"))?;
                output_to_index.insert(OutPoint::new(tx_hash.clone(), output_index), i);
            }
        }

        let mut in_degree = vec![0usize; items.len()];
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); items.len()];
        for (i, item) in items.iter().enumerate() {
            let tx = tx_of(item);
            for input in tx.input_pts_iter() {
                if let Some(&parent) = output_to_index.get(&input)
                    && parent != i
                {
                    let degree = in_degree
                        .get_mut(i)
                        .ok_or(DependencySortError::Projection("child indegree index"))?;
                    *degree = degree
                        .checked_add(1)
                        .ok_or(DependencySortError::Arithmetic("child indegree"))?;
                    children
                        .get_mut(parent)
                        .ok_or(DependencySortError::Projection("parent child-list index"))?
                        .push(i);
                }
            }
            for dep in tx.cell_deps_iter() {
                let out_point = dep.out_point();
                if let Some(&parent) = output_to_index.get(&out_point)
                    && parent != i
                {
                    let degree = in_degree
                        .get_mut(i)
                        .ok_or(DependencySortError::Projection("dep child indegree index"))?;
                    *degree = degree
                        .checked_add(1)
                        .ok_or(DependencySortError::Arithmetic("dep child indegree"))?;
                    children
                        .get_mut(parent)
                        .ok_or(DependencySortError::Projection(
                            "dep parent child-list index",
                        ))?
                        .push(i);
                }
            }
        }

        let mut ready: VecDeque<usize> = (0..items.len())
            .filter(|&index| in_degree.get(index).is_some_and(|degree| *degree == 0))
            .collect();
        let mut sorted = Vec::with_capacity(items.len());
        while let Some(i) = ready.pop_front() {
            sorted.push(i);
            let planned_children = children
                .get(i)
                .ok_or(DependencySortError::Projection("ready child-list index"))?;
            for &child in planned_children {
                let degree = in_degree
                    .get_mut(child)
                    .ok_or(DependencySortError::Projection(
                        "ready child indegree index",
                    ))?;
                *degree = degree
                    .checked_sub(1)
                    .ok_or(DependencySortError::Projection(
                        "dependency indegree underflow",
                    ))?;
                if *degree == 0 {
                    ready.push_back(child);
                }
            }
        }

        if sorted.len() != items.len() {
            // A cycle should never happen in valid detached blocks, but if it
            // does we keep the original order rather than losing transactions.
            return Ok(());
        }

        let mut remaining: Vec<Option<T>> = items.drain(..).map(Some).collect();
        let mut reordered = Vec::with_capacity(remaining.len());
        for index in sorted {
            let Some(item) = remaining.get_mut(index).and_then(Option::take) else {
                reordered.extend(
                    remaining
                        .into_iter()
                        .enumerate()
                        .filter_map(|(index, item)| item.map(|item| (index, item))),
                );
                reordered.sort_unstable_by_key(|(index, _)| *index);
                items.extend(reordered.into_iter().map(|(_, item)| item));
                return Err(DependencySortError::Projection(
                    "topological permutation index",
                ));
            };
            reordered.push((index, item));
        }
        items.extend(reordered.into_iter().map(|(_, item)| item));
        Ok(())
    }

    /// Converge both executable authorities while their universal lock order
    /// is still held. This is the only failure exit for an in-progress reorg
    /// draft: no worker can observe a partially reconciled kernel generation,
    /// and retired payload graphs are returned for destruction after both
    /// authority guards open.
    fn converge_reorg_fallback(
        &self,
        tx_pool: &mut TxPool,
        kernel: &mut PrePoolKernel,
        fallback: ReorgFallback,
        recovery_epoch: u64,
        snapshot: Arc<Snapshot>,
    ) -> (
        RetiredReorgGeneration,
        Option<crate::component::pre_pool::PrePoolFault>,
    ) {
        let (kernel, error) = match fallback {
            ReorgFallback::Retain(txs) => {
                match kernel.replace_generation_for_chain(|fresh| {
                    fresh.retain_recovery_prefix_after_clear(txs, recovery_epoch)
                }) {
                    Ok((_retained, retired)) => (retired, None),
                    Err(error) => (
                        kernel.replace_empty_generation(),
                        Some(error.into_unexpected_fault()),
                    ),
                }
            }
            ReorgFallback::Empty => (kernel.replace_empty_generation(), None),
        };
        let accepted = tx_pool.reset_generation(Arc::clone(&snapshot));
        self.journal_block_assembler_reset(snapshot);
        (RetiredReorgGeneration { accepted, kernel }, error)
    }

    pub(crate) async fn update_tx_pool_for_reorg(
        &self,
        detached_blocks: VecDeque<BlockView>,
        attached_blocks: VecDeque<BlockView>,
        detached_proposal_id: HashSet<ProposalShortId>,
        snapshot: Arc<Snapshot>,
    ) -> Result<(Vec<UncleBlockView>, Arc<Snapshot>), ReorgUpdateError> {
        let effect_bound = self.max_reorg_effect_bytes();
        let recovery_epoch = self.current_pipeline_epoch()?;

        let mining = if self.block_assembler.is_some() {
            reorg::MiningMode::Package
        } else {
            reorg::MiningMode::ObserveOnly
        };
        let mut detached = LinkedHashSet::default();
        let mut attached = LinkedHashSet::default();

        let detached_headers: HashSet<Byte32> = detached_blocks
            .iter()
            .map(|blk| blk.header().hash())
            .collect();
        // Phase two needs only compact uncle candidates, never the full
        // transaction-bearing blocks. Bound the handoff through the same
        // candidate container used by the assembler so completing phase one
        // releases the original retained reorg message and all tx payloads.
        let candidate_uncles = if mining.packages() {
            let mut candidates = crate::block_assembler::CandidateUncles::new();
            for block in &detached_blocks {
                candidates.insert(block.as_uncle());
            }
            candidates.into_values()
        } else {
            Vec::new()
        };
        let attached_headers: HashSet<Byte32> = attached_blocks
            .iter()
            .map(|blk| blk.header().hash())
            .collect();

        for blk in detached_blocks {
            detached.extend(blk.transactions().into_iter().skip(1))
        }

        for blk in attached_blocks {
            self.aux.fee_estimator.commit_block(&blk);
            attached.extend(blk.transactions().into_iter().skip(1));
        }
        let mut retain = reorg::detached_not_attached(&detached, &attached);
        Self::sort_txs_by_dependencies(&mut retain)?;
        let mut retained_hashes = HashSet::new();
        retain.retain(|tx| retained_hashes.insert(tx.hash()));

        {
            // This closure is used to limit the lifetime of mutable tx_pool.
            let mut tx_pool = self.pool.tx_pool.write().await;
            // A later clear publishes its epoch barrier before waiting for
            // this guard. If it won while this reorg was queued, applying the
            // older chain cohort afterwards would resurrect cleared work.
            if !self.is_pipeline_epoch_current(recovery_epoch) {
                return Ok((Vec::new(), snapshot));
            }
            let transition = self.pipeline.kernel.mutate_authoritative(|kernel| {
                self.relay
                    .effects
                    .try_apply_authoritative(effect_bound, |capacity| {
                        let publish_detail = capacity.retains_detail();
                        let (draft, detail) = (|| {
                        let mut outcome = match reorg::begin_tx_pool_reorg(
                            &mut tx_pool,
                            &attached,
                            &detached_headers,
                            detached_proposal_id.clone(),
                            Arc::clone(&snapshot),
                            mining,
                        ) {
                            Ok(outcome) => outcome,
                            Err(error) => {
                                error!("reorg accepted-pool preparation failed; resetting generation: {error:?}");
                                return (ReorgDraft::Reset(retain.clone()), None);
                            }
                        };
                        let accepted_plan = match reorg::plan_accepted_recovery(
                            &mut tx_pool,
                            &retain,
                            crate::constants::MAX_POOL_MUTATION_CANDIDATES,
                        ) {
                            Ok(plan) => plan,
                            Err(error) => {
                                error!("reorg accepted recovery planning failed; resetting generation: {error:?}");
                                return (ReorgDraft::Reset(retain.clone()), None);
                            }
                        };
                        let accepted = match &accepted_plan {
                            reorg::AcceptedRecoveryPlan::OverBound => {
                                return (ReorgDraft::Reset(retain.clone()), None);
                            }
                            plan => plan.transactions_parent_first(),
                        };
                        let mut combined = retain.clone();
                        combined.extend(accepted);
                        if let Err(error) = Self::sort_txs_by_dependencies(&mut combined) {
                            return (ReorgDraft::Fault(error.into()), None);
                        }
                        let mut hashes = HashSet::new();
                        combined.retain(|tx| hashes.insert(tx.hash()));
                        match kernel.retain_recovery_batch(combined.clone(), recovery_epoch) {
                            Ok(_) => {}
                            Err(PrePoolError::Public(_)) => {
                                return (ReorgDraft::Reset(combined), None);
                            }
                            Err(error) => {
                                // The reorg boundary owns both executable authorities,
                                // so it can repair a structural kernel contradiction by
                                // replacing the whole generation before either lock opens.
                                // Keep this distinct from transaction/capacity fallback:
                                // if the fresh rebuild repeats the defect, it escapes as a
                                // typed fault and the generation is not persisted.
                                error!("reorg recovery retention invariant failed; rebuilding generation: {error:?}");
                                return (ReorgDraft::Reset(combined), None);
                            }
                        };
                        let Some(moved) = reorg::apply_accepted_recovery(accepted_plan) else {
                            return (ReorgDraft::Reset(combined), None);
                        };
                        outcome.recovery_removed.extend(moved);
                        if let Err(error) =
                            reorg::finish_tx_pool_reorg(&mut tx_pool, &mut outcome)
                        {
                            error!("reorg accepted-pool finalization failed; resetting generation: {error:?}");
                            return (ReorgDraft::Reset(combined), None);
                        }
                        // Apply the complete membership delta under the same pool write
                        // guard. Attached/on-chain parents remain available; every other
                        // physical removal demotes its already-resolved kernel
                        // consumers. An impossible projection defect unwinds to the
                        // typed internal defect boundary before either guard opens.
                        let mut committed: HashSet<_> =
                            attached.iter().map(TransactionView::hash).collect();
                        let mut unavailable = HashSet::new();
                        for entry in outcome
                            .reject_events
                            .iter()
                            .map(|(entry, _)| entry)
                            .chain(outcome.silently_removed.iter())
                            .chain(outcome.recovery_removed.iter())
                        {
                            let hash = entry.transaction().hash();
                            if tx_pool.snapshot().transaction_exists(&hash) {
                                committed.insert(hash);
                            } else {
                                unavailable.insert(hash);
                            }
                        }
                        let mut available_outpoints = tx_pool.released_inputs_from_removed_entries(
                            outcome
                                .reject_events
                                .iter()
                                .map(|(entry, _)| entry)
                                .chain(outcome.silently_removed.iter()),
                        );
                        // A historical candidate may have observed its conflicting
                        // input release before another required parent was mined.
                        // Attached outputs and headers are potential availability
                        // edges, but only the post-reorg overlay decides their
                        // level: an output created and consumed in the same
                        // attached branch is not available and must not wake a
                        // rejected conflict into a bogus second rejection.
                        available_outpoints
                            .extend(attached.iter().flat_map(TransactionView::output_pts));
                        let mut available_dependencies =
                            crate::service::pipeline_ops::available_cell_dependencies(
                                &tx_pool,
                                available_outpoints,
                            );
                        available_dependencies.extend(attached_headers.iter().filter_map(|hash| {
                            let key = crate::component::pre_pool::DependencyKey::Header(
                                crate::util::compact_packed(hash),
                            );
                            crate::service::pipeline_ops::dependency_is_available(&tx_pool, &key)
                                .then_some(key)
                        }));
                        let external_plan = match kernel.plan_external_commit(
                                &committed,
                                &unavailable,
                                available_dependencies,
                                Vec::new(),
                            ) {
                                Ok(plan) => plan,
                                Err(error) => {
                                    error!("reorg pre-pool membership planning failed; resetting generation: {error:?}");
                                    return (ReorgDraft::Reset(combined), None);
                                }
                            };
                        let externally_committed = external_plan.records().to_vec();
                        external_plan.apply();
                        let mut effects = Vec::new();
                        if publish_detail {
                            for (entry, reject) in &outcome.reject_events {
                                match self.rejected_effects(entry.clone(), reject.clone()) {
                                    Ok(rejected) => effects.extend(rejected),
                                    Err(error) => {
                                        return (ReorgDraft::Fault(error.into()), None);
                                    }
                                }
                            }
                            for (entry, status) in &outcome.notify_events {
                                if let Some(effect) = self.accepted_effect(entry.clone(), *status) {
                                    effects.push(effect);
                                }
                            }
                            for record in externally_committed {
                                if let Some(peer) = record.raw.ingress_peer() {
                                    effects.push(TxPoolEffect::Relay(
                                        crate::service::TxVerificationResult::Ok {
                                            original_peer: Some(peer),
                                            tx_hash: record.raw.tx.hash(),
                                        },
                                    ));
                                }
                            }
                        }
                        // Invalidate the old template at the same
                        // linearization point as the new snapshot
                        // and retained ownership. Full publication
                        // remains the second retained reorg phase.
                        self.journal_block_assembler_reset(Arc::clone(&snapshot));
                        (ReorgDraft::Queued, EffectBatch::new(effects))
                        })();

                        match draft {
                            ReorgDraft::Queued => capacity.detail(ReorgMutation::Queued, detail),
                            ReorgDraft::Reset(txs) => {
                                let (retired, error) = self.converge_reorg_fallback(
                                    &mut tx_pool,
                                    kernel,
                                    ReorgFallback::Retain(txs),
                                    recovery_epoch,
                                    Arc::clone(&snapshot),
                                );
                                let mutation = match error {
                                    None => ReorgMutation::Reset(retired),
                                    Some(error) => ReorgMutation::Fault {
                                        error: error.into(),
                                        retired,
                                    },
                                };
                                capacity.reset(mutation)
                            }
                            ReorgDraft::Fault(error) => {
                                let (retired, convergence_error) = self.converge_reorg_fallback(
                                    &mut tx_pool,
                                    kernel,
                                    ReorgFallback::Empty,
                                    recovery_epoch,
                                    Arc::clone(&snapshot),
                                );
                                let error = convergence_error.map_or(error, Into::into);
                                capacity.reset(ReorgMutation::Fault { error, retired })
                            }
                        }
                    })
            });
            match transition {
                Ok(ReorgMutation::Queued) => {}
                Ok(ReorgMutation::Reset(retired)) => {
                    drop(tx_pool);
                    retired.dispose();
                }
                Ok(ReorgMutation::Fault { error, retired }) => {
                    drop(tx_pool);
                    retired.dispose();
                    return Err(error.into());
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok((candidate_uncles, snapshot))
    }

    pub(crate) async fn clear_pool(&mut self, new_snapshot: Arc<Snapshot>) {
        self.advance_pipeline_epoch();
        let mut tx_pool = self.pool.tx_pool.write().await;
        let transition = self.pipeline.kernel.mutate_authoritative(|kernel| {
            self.relay.effects.apply_generation_reset(|| {
                let kernel = kernel.replace_empty_generation();
                let accepted = tx_pool.reset_generation(Arc::clone(&new_snapshot));
                self.journal_block_assembler_reset(new_snapshot);
                (accepted, kernel)
            })
        });
        drop(tx_pool);
        match transition {
            Ok(retired) => drop(retired),
            Err(error) => self.fail_tx_pool_generation(
                "clear-pool generation reset journal failed",
                &TxPoolGenerationFault::Effect(error),
            ),
        }
    }

    pub(crate) async fn save_pool(&self) {
        // Wait before copying either bounded authority. The unique lease is
        // then moved into the blocking closure, so this async task never owns
        // a lock/permit across its join await and concurrent save requests do
        // not accumulate full transaction snapshots.
        let writer = self.persistence_writer.acquire().await;
        let (base, accepted, recovery) = {
            // Universal cross-authority order. Copying compact transaction
            // views is bounded by the accepted + pre-pool residency limits;
            // no authority guard survives the blocking writer.
            let tx_pool = self.pool.tx_pool.read().await;
            let base = tx_pool.config.persisted_data.clone();
            let accepted = tx_pool.get_all_txs();
            let recovery = self.pipeline.kernel.recovery_snapshot();
            (base, accepted, recovery)
        };
        let snapshot = crate::persisted::PersistenceSnapshot { accepted, recovery };
        let result = tokio::task::spawn_blocking(move || writer.write(&base, snapshot)).await;
        match result {
            Ok(Ok(())) => info!("TxPool saved successfully"),
            Ok(Err(err)) => error!("failed to save pool, error: {:?}", err),
            Err(err) => error!("tx-pool persistence writer failed to join: {:?}", err),
        }
    }

    pub(crate) async fn update_ibd_state(&self, in_ibd: bool) {
        self.aux.fee_estimator.update_ibd_state(in_ibd);
    }

    pub(crate) async fn estimate_fee_rate(
        &self,
        estimate_mode: EstimateMode,
        enable_fallback: bool,
    ) -> Result<FeeRate, AnyError> {
        let all_entry_info = self.all_entry_info().await;
        match self
            .aux
            .fee_estimator
            .estimate_fee_rate(estimate_mode, all_entry_info)
        {
            Ok(fee_rate) => Ok(fee_rate),
            Err(err) => {
                if enable_fallback {
                    let target_blocks =
                        FeeEstimator::target_blocks_for_estimate_mode(estimate_mode);
                    self.pool
                        .tx_pool
                        .read()
                        .await
                        .estimate_fee_rate(target_blocks)
                        .map_err(Into::into)
                } else {
                    Err(err.into())
                }
            }
        }
    }
}

pub(crate) struct PreCheckedTx {
    /// Tip hash at the time the transaction was pre-checked.
    pub(crate) pre_resolve_tip: Byte32,
    /// Fully resolved transaction.
    pub(crate) rtx: Arc<ResolvedTransaction>,
    /// Current status (fresh / gap / proposed) relative to the proposal window.
    pub(crate) status: Status,
    /// Transaction fee.
    pub(crate) fee: Capacity,
    /// Transaction size in bytes as serialized in a block.
    pub(crate) tx_size: usize,
    /// Conservative resolved payload residency, computed once at resolve.
    pub(crate) resident_size: usize,
}

type ResolveResult = Result<(Arc<ResolvedTransaction>, Status), Reject>;

/// Assemble a [`PreCheckedTx`] from its parts; shared by the fast
/// (chain-only) and locked (pool-overlay) pre-check paths.
fn make_pre_checked_tx(
    snapshot: &Snapshot,
    rtx: Arc<ResolvedTransaction>,
    status: Status,
    fee: Capacity,
    tx_size: usize,
) -> PreCheckedTx {
    // Accept the resolution snapshot rather than an independently supplied
    // hash, so every production call site derives the provenance stamp from
    // the same capture it used for resolution. Final admission relies on this
    // pairing.
    let pre_resolve_tip = snapshot.tip_hash();
    let rtx = crate::resolved_tx::compact_resolved_transaction_for_residency(rtx);
    let resident_size = crate::component::entry::resolved_transaction_charge_bytes(tx_size, &rtx);
    PreCheckedTx {
        pre_resolve_tip,
        rtx,
        status,
        fee,
        tx_size,
        resident_size,
    }
}

fn get_tx_status(snapshot: &Snapshot, short_id: &ProposalShortId) -> Status {
    if snapshot.proposals().contains_proposed(short_id) {
        Status::Proposed
    } else if snapshot.proposals().contains_gap(short_id) {
        Status::Gap
    } else {
        Status::Pending
    }
}

fn resolve_tx(
    tx_pool: &TxPool,
    snapshot: &Snapshot,
    tx: TransactionView,
    rbf: bool,
) -> ResolveResult {
    let short_id = tx.proposal_short_id();
    let tx_status = get_tx_status(snapshot, &short_id);
    tx_pool
        .resolve_tx_from_pool(tx, rbf)
        .map(|rtx| (rtx, tx_status))
}
