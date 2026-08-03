//! Production-shaped bridge from immutable UAK template receipts to the
//! rebuildable block-assembler output.
//!
//! The adapter owns no transaction state. One ordered replacement lane and
//! three optimistic component lanes construct outside both authorities and
//! meet only at the existing short current-template publication boundary.

use super::{
    packing::TemplatePackingLimits,
    runtime::AuthorityRuntime,
    template::{
        AuthorityTemplateInput, FullTemplateBuild, PartialTemplateBuild, ResetTemplateBuild,
        TemplateComponent, TemplateConvergence, TemplateConvergenceError, TemplatePublication,
        TemplateReadError,
    },
};
use crate::{
    block_assembler::{
        BlockAssembler, BlockTemplateBuilder, CandidateUncleMutationError, CandidateUnclePrune,
        CurrentTemplate, TemplateContentUpdate, TemplateSize,
    },
    error::BlockAssemblerError,
};
use ckb_async_runtime::Handle;
use ckb_error::AnyError;
use ckb_snapshot::Snapshot;
use ckb_stop_handler::CancellationToken;
use ckb_store::ChainStore;
use ckb_systemtime::unix_time_as_millis;
use ckb_types::{
    core::{EpochExt, UncleBlockView},
    packed::ProposalShortId,
};
use ckb_util::Mutex;
use std::{cmp, collections::HashSet, sync::Arc, time::Duration};
use tokio::sync::Notify;

const TEMPLATE_ALLOCATION_RETRY: Duration = Duration::from_millis(1);

#[derive(Debug)]
pub(in crate::authority) enum AuthorityTemplateDriverFault {
    Read(TemplateReadError),
    Convergence(TemplateConvergenceError),
    Block(AnyError),
    Candidate(CandidateUncleMutationError),
}

impl From<TemplateReadError> for AuthorityTemplateDriverFault {
    fn from(error: TemplateReadError) -> Self {
        Self::Read(error)
    }
}

impl From<TemplateConvergenceError> for AuthorityTemplateDriverFault {
    fn from(error: TemplateConvergenceError) -> Self {
        Self::Convergence(error)
    }
}

impl From<BlockAssemblerError> for AuthorityTemplateDriverFault {
    fn from(error: BlockAssemblerError) -> Self {
        Self::Block(error.into())
    }
}

impl From<AnyError> for AuthorityTemplateDriverFault {
    fn from(error: AnyError) -> Self {
        Self::Block(error)
    }
}

impl From<CandidateUncleMutationError> for AuthorityTemplateDriverFault {
    fn from(error: CandidateUncleMutationError) -> Self {
        Self::Candidate(error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use = "a template step determines whether its level should run again"]
pub(in crate::authority) enum AuthorityTemplateStep {
    Idle,
    Published,
    Stale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TemplateRetryWake {
    Cancelled,
    Retry,
}

pub(in crate::authority) struct AuthorityTemplateDriverHandles {
    pub(in crate::authority) replacement:
        tokio::task::JoinHandle<Result<(), AuthorityTemplateDriverFault>>,
    pub(in crate::authority) proposals:
        tokio::task::JoinHandle<Result<(), AuthorityTemplateDriverFault>>,
    pub(in crate::authority) transactions:
        tokio::task::JoinHandle<Result<(), AuthorityTemplateDriverFault>>,
    pub(in crate::authority) uncles:
        tokio::task::JoinHandle<Result<(), AuthorityTemplateDriverFault>>,
}

/// The sole UAK adapter for one block-assembler output generation.
///
/// `convergence` is rebuildable output coverage, not transaction membership.
/// It is updated together with `BlockAssembler::current` by the publication
/// methods below. `wake` is a lossy hint; each lane re-reads the exact level.
#[derive(Clone)]
pub(in crate::authority) struct AuthorityBlockAssembler {
    runtime: AuthorityRuntime,
    assembler: BlockAssembler,
    convergence: Arc<Mutex<TemplateConvergence>>,
    wake: Arc<Notify>,
}

pub(in crate::authority) struct PreparedFull {
    build: FullTemplateBuild,
    current: CurrentTemplate,
    prune: CandidateUnclePrune,
}

pub(in crate::authority) struct PreparedPartial {
    build: PartialTemplateBuild,
    current: CurrentTemplate,
    prune: Option<CandidateUnclePrune>,
}

pub(in crate::authority) struct PreparedReset {
    build: ResetTemplateBuild,
    current: CurrentTemplate,
    prune: CandidateUnclePrune,
}

impl AuthorityBlockAssembler {
    pub(in crate::authority) async fn new(
        runtime: AuthorityRuntime,
        assembler: BlockAssembler,
    ) -> Result<Self, AuthorityTemplateDriverFault> {
        let input = runtime.template_input()?;
        let current = assembler.current.read().await;
        let reset_epoch = current.reset_epoch;
        drop(current);
        let epoch = next_epoch(input.snapshot())?;
        let (_, _, uncle_source) = assembler
            .prepare_uncles(input.snapshot(), &epoch)
            .into_parts();
        let convergence = TemplateConvergence::new(input.source_cut(uncle_source), reset_epoch);
        Ok(Self {
            runtime,
            assembler,
            convergence: Arc::new(Mutex::new(convergence)),
            wake: Arc::new(Notify::new()),
        })
    }

    /// Insert through the typed candidate source and wake the uncle level only
    /// after its version changed. The adapter never exposes a raw mutation
    /// handle to production callers.
    pub(in crate::authority) fn receive_candidate_uncle(
        &self,
        uncle: UncleBlockView,
    ) -> Result<bool, AuthorityTemplateDriverFault> {
        let inserted = self.assembler.candidate_uncles.lock().try_insert(uncle)?;
        if inserted {
            self.wake.notify_waiters();
        }
        Ok(inserted)
    }

    pub(in crate::authority) fn spawn_drivers(
        &self,
        handle: &Handle,
        cancel: CancellationToken,
    ) -> AuthorityTemplateDriverHandles {
        let replacement = {
            let driver = self.clone();
            let cancel = cancel.child_token();
            handle.spawn(async move { driver.run_replacement_lane(cancel).await })
        };
        let proposals =
            self.spawn_component_lane(handle, cancel.child_token(), TemplateComponent::Proposals);
        let transactions = self.spawn_component_lane(
            handle,
            cancel.child_token(),
            TemplateComponent::Transactions,
        );
        let uncles =
            self.spawn_component_lane(handle, cancel.child_token(), TemplateComponent::Uncles);
        AuthorityTemplateDriverHandles {
            replacement,
            proposals,
            transactions,
            uncles,
        }
    }

    fn spawn_component_lane(
        &self,
        handle: &Handle,
        cancel: CancellationToken,
        component: TemplateComponent,
    ) -> tokio::task::JoinHandle<Result<(), AuthorityTemplateDriverFault>> {
        let driver = self.clone();
        handle.spawn(async move { driver.run_component_lane(component, cancel).await })
    }

    async fn run_replacement_lane(
        self,
        cancel: CancellationToken,
    ) -> Result<(), AuthorityTemplateDriverFault> {
        loop {
            if cancel.is_cancelled() {
                return Ok(());
            }
            let authority_signal = self.runtime.mutation_signal();
            let authority_notified = authority_signal.notified();
            let local_notified = self.wake.notified();
            match self.drive_replacement_once().await {
                Ok(AuthorityTemplateStep::Idle) => {
                    tokio::select! {
                        _ = cancel.cancelled() => return Ok(()),
                        _ = authority_notified => {},
                        _ = local_notified => {},
                    }
                }
                Ok(AuthorityTemplateStep::Published | AuthorityTemplateStep::Stale) => {
                    tokio::task::yield_now().await;
                }
                Err(AuthorityTemplateDriverFault::Read(TemplateReadError::Allocation)) => {
                    if wait_template_retry(&cancel, authority_notified, local_notified).await
                        == TemplateRetryWake::Cancelled
                    {
                        return Ok(());
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }

    async fn run_component_lane(
        self,
        component: TemplateComponent,
        cancel: CancellationToken,
    ) -> Result<(), AuthorityTemplateDriverFault> {
        loop {
            if cancel.is_cancelled() {
                return Ok(());
            }
            let authority_signal = self.runtime.mutation_signal();
            let authority_notified = authority_signal.notified();
            let local_notified = self.wake.notified();
            match self.drive_component_once(component).await {
                Ok(AuthorityTemplateStep::Idle) => {
                    tokio::select! {
                        _ = cancel.cancelled() => return Ok(()),
                        _ = authority_notified => {},
                        _ = local_notified => {},
                    }
                }
                Ok(AuthorityTemplateStep::Published | AuthorityTemplateStep::Stale) => {
                    tokio::task::yield_now().await;
                }
                Err(AuthorityTemplateDriverFault::Read(TemplateReadError::Allocation)) => {
                    if wait_template_retry(&cancel, authority_notified, local_notified).await
                        == TemplateRetryWake::Cancelled
                    {
                        return Ok(());
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }

    pub(in crate::authority) async fn drive_replacement_once(
        &self,
    ) -> Result<AuthorityTemplateStep, AuthorityTemplateDriverFault> {
        let input = self.runtime.template_input()?;
        let current = self.assembler.current.read().await.clone();
        if current.snapshot.tip_hash() != input.snapshot().tip_hash() {
            let Some(prepared) = self.prepare_reset(input, current.reset_epoch)? else {
                return Ok(AuthorityTemplateStep::Stale);
            };
            return self.publish_reset(prepared).await;
        }
        let Some(prepared) = self.prepare_full(input, current)? else {
            return Ok(AuthorityTemplateStep::Idle);
        };
        self.publish_full(prepared).await
    }

    pub(in crate::authority) async fn drive_component_once(
        &self,
        component: TemplateComponent,
    ) -> Result<AuthorityTemplateStep, AuthorityTemplateDriverFault> {
        let input = self.runtime.template_input()?;
        let current = self.assembler.current.read().await.clone();
        if current.snapshot.tip_hash() != input.snapshot().tip_hash() {
            return Ok(AuthorityTemplateStep::Idle);
        }
        let prepared = match component {
            TemplateComponent::Proposals => self.prepare_proposals(&input, current)?,
            TemplateComponent::Transactions => self.prepare_transactions(&input, current)?,
            TemplateComponent::Uncles => self.prepare_uncles(&input, current)?,
        };
        let Some(prepared) = prepared else {
            return Ok(AuthorityTemplateStep::Idle);
        };
        self.publish_partial(prepared).await
    }

    #[cfg(test)]
    pub(in crate::authority) async fn prepare_full_for_foundation(
        &self,
    ) -> Result<Option<PreparedFull>, AuthorityTemplateDriverFault> {
        let input = self.runtime.template_input()?;
        let current = self.assembler.current.read().await.clone();
        if current.snapshot.tip_hash() != input.snapshot().tip_hash() {
            return Ok(None);
        }
        self.prepare_full(input, current)
    }

    #[cfg(test)]
    pub(in crate::authority) async fn prepare_component_for_foundation(
        &self,
        component: TemplateComponent,
    ) -> Result<Option<PreparedPartial>, AuthorityTemplateDriverFault> {
        let input = self.runtime.template_input()?;
        let current = self.assembler.current.read().await.clone();
        if current.snapshot.tip_hash() != input.snapshot().tip_hash() {
            return Ok(None);
        }
        match component {
            TemplateComponent::Proposals => self.prepare_proposals(&input, current),
            TemplateComponent::Transactions => self.prepare_transactions(&input, current),
            TemplateComponent::Uncles => self.prepare_uncles(&input, current),
        }
    }

    #[cfg(test)]
    pub(in crate::authority) async fn prepare_reset_for_foundation(
        &self,
    ) -> Result<Option<PreparedReset>, AuthorityTemplateDriverFault> {
        let input = self.runtime.template_input()?;
        let current = self.assembler.current.read().await.clone();
        if current.snapshot.tip_hash() == input.snapshot().tip_hash() {
            return Ok(None);
        }
        self.prepare_reset(input, current.reset_epoch)
    }

    #[cfg(test)]
    pub(in crate::authority) async fn publish_full_for_foundation(
        &self,
        prepared: PreparedFull,
    ) -> Result<AuthorityTemplateStep, AuthorityTemplateDriverFault> {
        self.publish_full(prepared).await
    }

    #[cfg(test)]
    pub(in crate::authority) async fn publish_component_for_foundation(
        &self,
        prepared: PreparedPartial,
    ) -> Result<AuthorityTemplateStep, AuthorityTemplateDriverFault> {
        self.publish_partial(prepared).await
    }

    #[cfg(test)]
    pub(in crate::authority) async fn publish_reset_for_foundation(
        &self,
        prepared: PreparedReset,
    ) -> Result<AuthorityTemplateStep, AuthorityTemplateDriverFault> {
        self.publish_reset(prepared).await
    }

    #[cfg(test)]
    pub(in crate::authority) async fn is_converged_for_foundation(&self) -> bool {
        let published_reset = self.assembler.current.read().await.reset_epoch;
        self.convergence.lock().is_converged(published_reset)
    }

    fn prepare_full(
        &self,
        input: AuthorityTemplateInput,
        current: Arc<CurrentTemplate>,
    ) -> Result<Option<PreparedFull>, AuthorityTemplateDriverFault> {
        let epoch = next_epoch(input.snapshot())?;
        let (prepared_uncles, prune, uncle_source) = self
            .assembler
            .prepare_uncles(input.snapshot(), &epoch)
            .into_parts();
        let sources = input.source_cut(uncle_source);
        let Some(build) = self.convergence.lock().begin_pending_full(sources) else {
            return Ok(None);
        };

        let consensus = input.snapshot().consensus();
        let proposals = input
            .selection()
            .proposal_short_ids(consensus.max_block_proposals_limit())?;
        let proposal_set = proposals.iter().cloned().collect::<HashSet<_>>();
        let uncles = BlockAssembler::filter_uncles_conflicting_with_proposals(
            input.snapshot(),
            &prepared_uncles,
            &proposal_set,
        );
        let basic_size = BlockAssembler::basic_block_size(
            current.template.cellbase.data(),
            &uncles,
            proposals.iter(),
            current.template.extension.clone(),
        );
        let tx_bytes = (consensus.max_block_bytes() as usize)
            .checked_sub(basic_size)
            .ok_or(BlockAssemblerError::Overflow)?;
        let packed = input
            .selection()
            .pack_transactions(TemplatePackingLimits::new(
                tx_bytes,
                consensus.max_block_cycles(),
            ))?;
        let txs = packed.into_tx_entries();
        let (dao, checked_txs, _failed) = BlockAssembler::calc_dao(
            input.snapshot(),
            &epoch,
            current.template.cellbase.clone(),
            txs,
            &self.assembler.cell_liveness_memo,
        )?;
        // Final template liveness is a defensive projection check, not an
        // authority transition. A coherent UAK receipt should retain every
        // selected transaction, but an unexpected miss must degrade to the
        // checked subset rather than terminate a miner-facing worker or
        // mutate authoritative membership from incomplete overlay evidence.

        let txs_size = BlockAssembler::checked_entries_size(&checked_txs)?;
        let proposals_size = proposals
            .len()
            .checked_mul(ProposalShortId::serialized_size())
            .ok_or(BlockAssemblerError::Overflow)?;
        let uncles_size = BlockAssembler::uncles_size(&uncles)?;
        let total = basic_size
            .checked_add(txs_size)
            .ok_or(BlockAssemblerError::Overflow)?;
        let current = self.updated_current(
            &current,
            TemplateContentUpdate::Full {
                uncles,
                transactions: checked_txs,
                proposals,
                dao,
            },
            TemplateSize {
                txs: txs_size,
                proposals: proposals_size,
                uncles: uncles_size,
                total,
            },
        )?;
        Ok(Some(PreparedFull {
            build,
            current,
            prune,
        }))
    }

    fn prepare_reset(
        &self,
        input: AuthorityTemplateInput,
        published_reset: crate::block_assembler::ResetEpoch,
    ) -> Result<Option<PreparedReset>, AuthorityTemplateDriverFault> {
        let epoch = next_epoch(input.snapshot())?;
        let (uncles, prune, uncle_source) = self
            .assembler
            .prepare_uncles(input.snapshot(), &epoch)
            .into_parts();
        let sources = input.source_cut(uncle_source);
        let Some(build) = self
            .convergence
            .lock()
            .ensure_reset(sources, published_reset)?
        else {
            return Ok(None);
        };
        let current = BlockAssembler::build_base_template(
            &self.assembler.config,
            &self.assembler.work_id,
            Arc::clone(input.snapshot()),
            &epoch,
            uncles,
            &self.assembler.cell_liveness_memo,
        )?;
        Ok(Some(PreparedReset {
            build,
            current,
            prune,
        }))
    }

    fn prepare_proposals(
        &self,
        input: &AuthorityTemplateInput,
        current: Arc<CurrentTemplate>,
    ) -> Result<Option<PreparedPartial>, AuthorityTemplateDriverFault> {
        let sources = input.pool_source_cut();
        let Some(build) = self
            .convergence
            .lock()
            .begin_pending_proposals(sources, current.revision)
        else {
            return Ok(None);
        };
        let consensus = input.snapshot().consensus();
        let proposals = input
            .selection()
            .proposal_short_ids(consensus.max_block_proposals_limit())?;
        let proposal_set = proposals.iter().cloned().collect::<HashSet<_>>();
        let uncles = BlockAssembler::filter_uncles_conflicting_with_proposals(
            input.snapshot(),
            &current.template.uncles,
            &proposal_set,
        );
        let uncles_size = BlockAssembler::uncles_size(&uncles)?;
        let proposals_size = proposals
            .len()
            .checked_mul(ProposalShortId::serialized_size())
            .ok_or(BlockAssemblerError::Overflow)?;
        let Some(total) = current
            .size
            .calc_total_by_uncles_and_proposals(uncles_size, proposals_size)
        else {
            return Err(BlockAssemblerError::Overflow.into());
        };
        if total > consensus.max_block_bytes() as usize {
            self.require_full();
            return Ok(None);
        }
        let size = TemplateSize {
            uncles: uncles_size,
            proposals: proposals_size,
            total,
            ..current.size
        };
        let updated = self.updated_current(
            &current,
            TemplateContentUpdate::Proposals { uncles, proposals },
            size,
        )?;
        Ok(Some(PreparedPartial {
            build,
            current: updated,
            prune: None,
        }))
    }

    fn prepare_transactions(
        &self,
        input: &AuthorityTemplateInput,
        current: Arc<CurrentTemplate>,
    ) -> Result<Option<PreparedPartial>, AuthorityTemplateDriverFault> {
        let sources = input.pool_source_cut();
        let Some(build) = self
            .convergence
            .lock()
            .begin_pending_transactions(sources, current.revision)
        else {
            return Ok(None);
        };
        let consensus = input.snapshot().consensus();
        let basic_size = BlockAssembler::basic_block_size(
            current.template.cellbase.data(),
            &current.template.uncles,
            current.template.proposals.iter(),
            current.template.extension.clone(),
        );
        let tx_bytes = (consensus.max_block_bytes() as usize)
            .checked_sub(basic_size)
            .ok_or(BlockAssemblerError::Overflow)?;
        let packed = input
            .selection()
            .pack_transactions(TemplatePackingLimits::new(
                tx_bytes,
                consensus.max_block_cycles(),
            ))?;
        let txs = packed.into_tx_entries();
        let epoch = next_epoch(input.snapshot())?;
        let (dao, checked_txs, _failed) = BlockAssembler::calc_dao(
            input.snapshot(),
            &epoch,
            current.template.cellbase.clone(),
            txs,
            &self.assembler.cell_liveness_memo,
        )?;
        // See `prepare_full`: the checked subset is a valid, deterministic
        // projection for this source cut and prevents both fail-stop and a
        // same-source retry loop.
        let txs_size = BlockAssembler::checked_entries_size(&checked_txs)?;
        let total = current
            .size
            .calc_total_by_txs(txs_size)
            .ok_or(BlockAssemblerError::Overflow)?;
        let size = TemplateSize {
            txs: txs_size,
            total,
            ..current.size
        };
        let updated = self.updated_current(
            &current,
            TemplateContentUpdate::Transactions {
                transactions: checked_txs,
                dao,
            },
            size,
        )?;
        Ok(Some(PreparedPartial {
            build,
            current: updated,
            prune: None,
        }))
    }

    fn prepare_uncles(
        &self,
        input: &AuthorityTemplateInput,
        current: Arc<CurrentTemplate>,
    ) -> Result<Option<PreparedPartial>, AuthorityTemplateDriverFault> {
        let epoch = next_epoch(input.snapshot())?;
        let (uncles, prune, uncle_source) = self
            .assembler
            .prepare_uncles(input.snapshot(), &epoch)
            .into_parts();
        let sources = input.source_cut(uncle_source);
        let Some(build) = self
            .convergence
            .lock()
            .begin_pending_uncles(sources, current.revision)
        else {
            return Ok(None);
        };
        let consensus = input.snapshot().consensus();
        let proposals = input
            .selection()
            .proposal_short_ids(consensus.max_block_proposals_limit())?;
        let proposal_set = proposals.iter().cloned().collect::<HashSet<_>>();
        let uncles = BlockAssembler::filter_uncles_conflicting_with_proposals(
            input.snapshot(),
            &uncles,
            &proposal_set,
        );
        let uncles_size = BlockAssembler::uncles_size(&uncles)?;
        let proposals_size = proposals
            .len()
            .checked_mul(ProposalShortId::serialized_size())
            .ok_or(BlockAssemblerError::Overflow)?;
        let Some(total) = current
            .size
            .calc_total_by_uncles_and_proposals(uncles_size, proposals_size)
        else {
            return Err(BlockAssemblerError::Overflow.into());
        };
        if total > consensus.max_block_bytes() as usize {
            self.require_full();
            return Ok(None);
        }
        let size = TemplateSize {
            uncles: uncles_size,
            proposals: proposals_size,
            total,
            ..current.size
        };
        let updated = self.updated_current(
            &current,
            TemplateContentUpdate::Proposals { uncles, proposals },
            size,
        )?;
        Ok(Some(PreparedPartial {
            build,
            current: updated,
            prune: Some(prune),
        }))
    }

    fn require_full(&self) {
        if self.convergence.lock().require_full() {
            self.wake.notify_waiters();
        }
    }

    fn updated_current(
        &self,
        current: &CurrentTemplate,
        update: TemplateContentUpdate,
        size: TemplateSize,
    ) -> Result<CurrentTemplate, BlockAssemblerError> {
        let mut builder = BlockTemplateBuilder::for_update(&current.template, update);
        builder
            .work_id(BlockAssembler::take_counter(
                &self.assembler.work_id,
                "work id",
            )?)
            .current_time(cmp::max(
                unix_time_as_millis(),
                current.template.current_time,
            ));
        Ok(current.with_content(builder.build(), size))
    }

    async fn publish_full(
        &self,
        prepared: PreparedFull,
    ) -> Result<AuthorityTemplateStep, AuthorityTemplateDriverFault> {
        let PreparedFull {
            build,
            mut current,
            prune,
        } = prepared;
        let mut published = self.assembler.current.write().await;
        // The current-template guard is acquired first, then the synchronous
        // authority read proves this build's chain capability is still exact.
        // No production path holds the authority guard while awaiting this
        // output lock, so the short Apply has no inverse lock order.
        if build.chain_source() != self.runtime.template_chain_source() {
            return Ok(AuthorityTemplateStep::Stale);
        }
        let next_revision = published
            .revision
            .next()
            .ok_or(BlockAssemblerError::CounterExhausted("template revision"))?;
        let mut convergence = self.convergence.lock();
        if convergence.publish_full(build, published.reset_epoch)? == TemplatePublication::Stale {
            return Ok(AuthorityTemplateStep::Stale);
        }
        self.assembler.candidate_uncles.lock().prune(prune);
        current.revision = next_revision;
        current.reset_epoch = published.reset_epoch;
        *published = Arc::new(current);
        drop(convergence);
        drop(published);
        self.wake.notify_waiters();
        Ok(AuthorityTemplateStep::Published)
    }

    async fn publish_partial(
        &self,
        prepared: PreparedPartial,
    ) -> Result<AuthorityTemplateStep, AuthorityTemplateDriverFault> {
        let PreparedPartial {
            build,
            mut current,
            prune,
        } = prepared;
        let mut published = self.assembler.current.write().await;
        if build.chain_source() != self.runtime.template_chain_source() {
            return Ok(AuthorityTemplateStep::Stale);
        }
        let next_revision = published
            .revision
            .next()
            .ok_or(BlockAssemblerError::CounterExhausted("template revision"))?;
        let mut convergence = self.convergence.lock();
        if convergence.publish_partial(build, published.revision)? == TemplatePublication::Stale {
            return Ok(AuthorityTemplateStep::Stale);
        }
        if let Some(prune) = prune {
            self.assembler.candidate_uncles.lock().prune(prune);
        }
        current.revision = next_revision;
        current.reset_epoch = published.reset_epoch;
        *published = Arc::new(current);
        drop(convergence);
        drop(published);
        self.wake.notify_waiters();
        Ok(AuthorityTemplateStep::Published)
    }

    async fn publish_reset(
        &self,
        prepared: PreparedReset,
    ) -> Result<AuthorityTemplateStep, AuthorityTemplateDriverFault> {
        let PreparedReset {
            build,
            mut current,
            prune,
        } = prepared;
        let reset_epoch = build.epoch();
        let mut published = self.assembler.current.write().await;
        if build.chain_source() != self.runtime.template_chain_source() {
            return Ok(AuthorityTemplateStep::Stale);
        }
        let next_revision = published
            .revision
            .next()
            .ok_or(BlockAssemblerError::CounterExhausted("template revision"))?;
        let mut convergence = self.convergence.lock();
        if convergence.publish_reset(build, published.reset_epoch)? == TemplatePublication::Stale {
            return Ok(AuthorityTemplateStep::Stale);
        }
        self.assembler.candidate_uncles.lock().prune(prune);
        current.revision = next_revision;
        current.reset_epoch = reset_epoch;
        *published = Arc::new(current);
        drop(convergence);
        drop(published);
        self.wake.notify_waiters();
        Ok(AuthorityTemplateStep::Published)
    }
}

async fn wait_template_retry(
    cancel: &CancellationToken,
    authority_notified: tokio::sync::futures::Notified<'_>,
    local_notified: tokio::sync::futures::Notified<'_>,
) -> TemplateRetryWake {
    tokio::select! {
        _ = cancel.cancelled() => TemplateRetryWake::Cancelled,
        _ = authority_notified => TemplateRetryWake::Retry,
        _ = local_notified => TemplateRetryWake::Retry,
        _ = tokio::time::sleep(TEMPLATE_ALLOCATION_RETRY) => TemplateRetryWake::Retry,
    }
}

fn next_epoch(snapshot: &Snapshot) -> Result<EpochExt, BlockAssemblerError> {
    snapshot
        .consensus()
        .next_epoch_ext(snapshot.tip_header(), &snapshot.borrow_as_data_loader())
        .map(|epoch| epoch.epoch())
        .ok_or(BlockAssemblerError::MissingTipEpoch)
}
