//! One template driver builds outside Store guards and publishes only while
//! its lifecycle, selected owners and uncle source remain current.

use super::{
    model::Error,
    packing::{Cache, Selection, TemplatePackingLimits},
    store::{Captured, ReadSet, Store},
};
use crate::{
    block_assembler::{BlockAssembler, BlockTemplate, CandidateUnclePrune, CurrentTemplate},
    component::entry::TxEntry,
    error::BlockAssemblerError,
    util::block_offload,
};
use ckb_error::{AnyError, prelude::thiserror};
use ckb_jsonrpc_types::BlockTemplate as JsonBlockTemplate;
use ckb_store::ChainStore;
use ckb_systemtime::unix_time_as_millis;
use ckb_types::{
    packed::ProposalShortId,
    prelude::{Entity, Reader},
};
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::Notify;

/// Validity follows the lifecycle and selected transaction/proposal owners.
/// Unrelated admissions may trigger a refresh without invalidating this template.
/// Weak identities detect owner replacement without retaining retired entries.
pub(crate) struct TemplateSource {
    view: u64,
    reads: ReadSet,
}

impl TemplateSource {
    /// Only final DAO transactions and selected proposals constrain publication.
    /// A proposal short-ID collision observes every matching captured owner.
    fn from_content(
        view: u64,
        selection: &Selection<'_>,
        transactions: &[TxEntry],
        proposals: &[ProposalShortId],
    ) -> Result<Self, Error> {
        let mut reads = ReadSet::default();
        let mut selected: HashSet<_> = transactions
            .iter()
            .map(|transaction| transaction.transaction().hash())
            .collect();
        let proposals: HashSet<_> = proposals.iter().map(Entity::as_slice).collect();
        for candidate in selection.candidates() {
            if selected.remove(candidate.hash())
                || proposals.contains(candidate.proposal_short_id().as_slice())
            {
                reads.observe_owner(candidate.hash(), Some(candidate.owner()))?;
            }
        }
        if let Some(hash) = selected.iter().min() {
            ckb_logger::error!("selected transaction {hash} is outside template candidates");
            return Err(Error::Fault(
                "selected transaction outside template candidates",
            ));
        }
        Ok(Self { view, reads })
    }
}

/// One build and the exact sources that authorize its publication and pruning.
struct PreparedTemplate {
    current: Arc<CurrentTemplate>,
    prune: CandidateUnclePrune,
}

/// Pool(Stale) yields and retries; Pool(Fault) faults the generation. Other
/// errors mark this build failed for readers waiting on a refresh.
/// Assembly errors retain their original diagnostic without controlling the pool.
#[derive(Debug, thiserror::Error)]
enum BuildError {
    #[error(transparent)]
    Pool(#[from] Error),
    #[error(transparent)]
    Assembly(#[from] AnyError),
}

impl From<BlockAssemblerError> for BuildError {
    fn from(error: BlockAssemblerError) -> Self {
        Self::Assembly(error.into())
    }
}

pub(super) struct Driver {
    store: Arc<Store>,
    assembler: BlockAssembler,
    max_ancestors: usize,
    pub(super) requested: Notify,
    updated: Notify,
    notification: Notify,
    failed: AtomicBool,
}
impl Driver {
    pub(super) fn new(
        store: Arc<Store>,
        assembler: BlockAssembler,
        max_ancestors: usize,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            assembler,
            max_ancestors,
            requested: Notify::new(),
            updated: Notify::new(),
            notification: Notify::new(),
            failed: AtomicBool::new(false),
        })
    }

    pub(super) fn uncle(&self, uncle: crate::block_assembler::BoundedCandidateUncle) {
        match self
            .assembler
            .candidate_uncles
            .lock()
            .try_insert_bounded(uncle)
        {
            Ok(true) => self.requested.notify_one(),
            Ok(false) => {}
            Err(error) => ckb_logger::warn!("candidate uncle unavailable: {error:?}"),
        }
    }

    pub(super) async fn read(
        &self,
        deadline: tokio::time::Instant,
    ) -> Result<JsonBlockTemplate, Error> {
        let mut attempted = false;
        loop {
            let updated = self.updated.notified();
            if self.store.is_faulted() {
                return Err(Error::Fault("template generation"));
            }
            if self.store.is_stopped() {
                return Err(Error::Closed);
            }
            // Queueing and previous stale attempts already used this request's
            // time. An expired reader cannot start another refresh wait.
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Full("template refresh timeout".into()));
            }
            let current = Arc::clone(&self.assembler.current.read());
            if let Some(source) = &current.source {
                match self.store.read_selected(source.view, &source.reads, || ()) {
                    Ok(()) => {
                        let mut output: JsonBlockTemplate = (&current.template).into();
                        output.current_time = unix_time_as_millis()
                            .max(current.template.current_time)
                            .into();
                        return Ok(output);
                    }
                    Err(Error::Stale) => {}
                    Err(error) => return Err(error),
                }
            }
            // Refresh waiters retain no obsolete template payload while the
            // sole driver prepares its next publication.
            drop(current);
            if attempted && self.failed.load(Ordering::Acquire) {
                return Err(Error::Full("template build".into()));
            }
            self.requested.notify_one();
            attempted = true;
            tokio::time::timeout_at(deadline, updated)
                .await
                .map_err(|_| Error::Full("template refresh timeout".into()))?;
        }
    }

    fn prepare(&self, packing: &mut Cache) -> Result<PreparedTemplate, BuildError> {
        let Captured {
            view,
            snapshot,
            owners,
            ..
        } = self.store.capture_accepted();
        let selection = packing.selection(&owners, &snapshot, self.max_ancestors)?;
        let epoch = snapshot
            .consensus()
            .next_epoch_ext(snapshot.tip_header(), &snapshot.borrow_as_data_loader())
            .ok_or(BlockAssemblerError::MissingTipEpoch)?
            .epoch();
        let (prepared, prune) = self
            .assembler
            .prepare_uncles(&snapshot, &epoch)
            .map_err(|error| {
                BuildError::Assembly(
                    ckb_error::OtherError::new(format!("uncle preparation: {error:?}")).into(),
                )
            })?
            .into_parts();
        // Mandatory parts depend on this lifecycle view and the assembler's
        // immutable config, even when selected owners have since changed.
        // Clone only their compact payloads; do not retain the old template's
        // transaction, proposal or uncle collections through this build.
        let reused = {
            let current = self.assembler.current.read();
            current
                .source
                .as_ref()
                .filter(|source| source.view == view)
                .map(|_| {
                    (
                        current.template.cellbase.clone(),
                        current.template.extension.clone(),
                    )
                })
        };
        let (cellbase, extension) = match reused {
            Some(parts) => parts,
            None => (
                BlockAssembler::build_cellbase(&self.assembler.config, &snapshot)?,
                BlockAssembler::build_extension(&snapshot)?,
            ),
        };
        let maximum = BlockAssembler::max_block_bytes(&snapshot)?;
        let fixed_size = BlockAssembler::basic_block_size(
            cellbase.data(),
            &[],
            std::iter::empty(),
            extension.clone(),
        );
        if fixed_size > maximum {
            return Err(BlockAssemblerError::Overflow.into());
        }
        let proposals =
            selection.proposal_short_ids(snapshot.consensus().max_block_proposals_limit());
        let optional = BlockAssembler::fit_optional_content(
            &snapshot, proposals, &prepared, fixed_size, maximum,
        )
        .ok_or(BlockAssemblerError::Overflow)?;
        let selected = selection
            .pack_transactions(TemplatePackingLimits::new(
                maximum
                    .checked_sub(optional.total_size)
                    .ok_or(BlockAssemblerError::Overflow)?,
                snapshot.consensus().max_block_cycles(),
            ))
            .map_err(Error::from)?;
        let (dao, transactions) = BlockAssembler::calc_dao(
            &snapshot,
            &epoch,
            cellbase.clone(),
            selected,
            &self.assembler.cell_liveness_memo,
        )?;
        let source =
            TemplateSource::from_content(view, &selection, &transactions, &optional.proposals)?;
        // Compute DAO only for the final selected contents. Work IDs and time
        // belong to this publication attempt, not the reused mandatory parts.
        let mut template = BlockTemplate::new(
            &snapshot,
            &epoch,
            cellbase,
            BlockAssembler::take_counter(&self.assembler.work_id, "work id")?,
            dao,
            unix_time_as_millis(),
        )?;
        template.extension = extension;
        template.transactions = transactions;
        template.proposals = optional.proposals;
        template.uncles = optional.uncles;
        Ok(PreparedTemplate {
            current: Arc::new(CurrentTemplate {
                template,
                source: Some(source),
            }),
            prune,
        })
    }

    fn rebuild(&self, packing: &mut Cache) -> Result<(), BuildError> {
        self.publish(self.prepare(packing)?)?;
        Ok(())
    }

    fn publish(&self, prepared: PreparedTemplate) -> Result<(), Error> {
        let PreparedTemplate { current, prune } = prepared;
        let source = current
            .source
            .as_ref()
            .ok_or(Error::Fault("template source"))?;
        let retired: Result<_, CandidateUnclePrune> =
            self.store.read_selected(source.view, &source.reads, || {
                let mut uncles = self.assembler.candidate_uncles.lock();
                let pruned = uncles.try_prune(prune)?;
                // Cache pruning and output replacement are both internal bounded
                // synchronous mutations. Payload destruction follows guard release.
                Ok((
                    pruned,
                    std::mem::replace(&mut *self.assembler.current.write(), Arc::clone(&current)),
                ))
            })?;
        // A stale plan also owns uncle payloads; discard it only after the
        // selected-source guards have opened.
        let retired = retired.map_err(|_stale| Error::Stale)?;
        drop(retired);
        self.notification.notify_one();
        Ok(())
    }

    pub(super) async fn run(self: Arc<Self>) -> Result<(), Error> {
        let mut first = true;
        let mut packing = Cache::default();
        loop {
            let changed = self.store.template_changed.notified();
            let requested = self.requested.notified();
            tokio::pin!(changed, requested);
            requested.as_mut().enable();
            if self.store.is_faulted() {
                self.updated.notify_waiters();
                return Err(Error::Fault("template generation"));
            }
            if self.store.is_stopped() {
                self.updated.notify_waiters();
                return Ok(());
            }
            let view = self.store.snapshot().0;
            let current_view = self
                .assembler
                .current
                .read()
                .source
                .as_ref()
                .map(|source| source.view);
            if !first && current_view == Some(view) {
                // Coalesce same-chain membership bursts. A new chain view
                // needs fresh mining work immediately; readers can also wake
                // an invalid selected-owner template without a timer delay.
                tokio::select! {
                    _ = &mut requested => {},
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {},
                }
            }
            first = false;
            let outcome = block_offload(|| self.rebuild(&mut packing));
            let stale = matches!(outcome, Err(BuildError::Pool(Error::Stale)));
            self.failed
                .store(outcome.is_err() && !stale, Ordering::Release);
            match outcome {
                Ok(()) => {}
                Err(BuildError::Pool(Error::Stale)) => {
                    self.updated.notify_waiters();
                    // Only publication can become stale after a coherent capture.
                    // Let concurrent commits finish before recapturing.
                    tokio::task::yield_now().await;
                    continue;
                }
                Err(error) => {
                    if matches!(error, BuildError::Pool(Error::Fault(_))) {
                        self.store.fault();
                    }
                    ckb_logger::error!("template build failed: {error}");
                }
            }
            self.updated.notify_waiters();
            tokio::select! { _ = &mut changed => {}, _ = &mut requested => {} }
        }
    }

    pub(super) async fn notify(self: Arc<Self>) -> Result<(), Error> {
        loop {
            let changed = self.store.template_changed.notified();
            let notification = self.notification.notified();
            tokio::pin!(changed, notification);
            notification.as_mut().enable();
            if self.store.is_stopped() {
                return Ok(());
            }
            if self.store.is_faulted() {
                return Err(Error::Fault("miner notification generation"));
            }
            tokio::select! {
                _ = &mut notification => self.assembler.notify().await,
                _ = &mut changed => {},
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/template_driver.rs"]
pub(super) mod tests;
