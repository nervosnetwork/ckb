//! One template driver builds outside Store guards and publishes only while
//! its lifecycle, selected owners and uncle source remain current.

use super::{
    model::Error,
    packing::{Selection, TemplatePackingLimits},
    store::{ReadSet, Store},
};
use crate::{
    block_assembler::{
        BlockAssembler, BlockTemplate, CandidateUnclePrune, CandidateUncleSourceReceipt,
        CurrentTemplate,
    },
    error::BlockAssemblerError,
    util::block_offload,
};
use ckb_error::AnyError;
use ckb_jsonrpc_types::BlockTemplate as JsonBlockTemplate;
use ckb_store::ChainStore;
use ckb_systemtime::unix_time_as_millis;
use std::{
    collections::{BTreeMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::Notify;

// Limit a reader's cooperative wait for the shared driver. This does not
// cancel that driver or promise to interrupt synchronous storage operations.
const TEMPLATE_REFRESH_WAIT: Duration = Duration::from_secs(30);

/// Validity follows the lifecycle and selected transaction/proposal owners.
/// Unrelated admissions may trigger a refresh without invalidating this template.
/// Weak identities detect owner replacement without retaining retired entries.
pub(crate) struct TemplateSource {
    view: u64,
    reads: ReadSet,
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
    pub(super) async fn read(&self) -> Result<JsonBlockTemplate, Error> {
        let deadline = tokio::time::Instant::now()
            .checked_add(TEMPLATE_REFRESH_WAIT)
            .ok_or(Error::Full("template refresh deadline".into()))?;
        let mut attempted = false;
        loop {
            let updated = self.updated.notified();
            if self.store.is_faulted() {
                return Err(Error::Fault("template generation"));
            }
            if self.store.is_stopped() {
                return Err(Error::Closed);
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
            // A ready notification can win timeout_at's first poll. Repeated
            // stale builds must still respect this reader's original deadline.
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Full("template refresh timeout".into()));
            }
            self.requested.notify_one();
            attempted = true;
            tokio::time::timeout_at(deadline, updated)
                .await
                .map_err(|_| Error::Full("template refresh timeout".into()))?;
        }
    }
    fn prepare(
        &self,
    ) -> Result<
        (
            Arc<CurrentTemplate>,
            CandidateUnclePrune,
            CandidateUncleSourceReceipt,
        ),
        AnyError,
    > {
        let (view, snapshot, owners, _) = self.store.capture(true);
        let original: BTreeMap<_, _> = owners
            .iter()
            .map(|entry| (entry.hash(), Arc::clone(entry)))
            .collect();
        let selection = Selection::new(owners, &snapshot, self.max_ancestors)?;
        let epoch = snapshot
            .consensus()
            .next_epoch_ext(snapshot.tip_header(), &snapshot.borrow_as_data_loader())
            .ok_or(BlockAssemblerError::MissingTipEpoch)?
            .epoch();
        let (prepared, prune, uncle_source) = self
            .assembler
            .prepare_uncles(&snapshot, &epoch)
            .map_err(|error| ckb_error::OtherError::new(format!("uncle preparation: {error:?}")))?
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
        let mut reads = ReadSet::default();
        for transaction in &transactions {
            let hash = transaction.transaction().hash();
            reads.owner(&hash, Some(original.get(&hash).ok_or(Error::Stale)?))?;
        }
        let proposals: HashSet<_> = optional.proposals.iter().cloned().collect();
        for (hash, entry) in &original {
            if proposals.contains(&entry.proposal()) {
                reads.owner(hash, Some(entry))?;
            }
        }
        // Compute DAO only for the final selected contents. Work IDs and time
        // belong to this publication attempt, not the reused mandatory parts.
        let mut template = BlockTemplate::new(
            &snapshot,
            &epoch,
            cellbase,
            BlockAssembler::take_counter(&self.assembler.work_id, "work id")?,
            dao,
            unix_time_as_millis().max(
                snapshot
                    .tip_header()
                    .timestamp()
                    .checked_add(1)
                    .ok_or(BlockAssemblerError::Overflow)?,
            ),
        )?;
        template.extension = extension;
        template.transactions = transactions;
        template.proposals = optional.proposals;
        template.uncles = optional.uncles;
        Ok((
            Arc::new(CurrentTemplate {
                template,
                source: Some(TemplateSource { view, reads }),
            }),
            prune,
            uncle_source,
        ))
    }
    fn rebuild(&self) -> Result<(), AnyError> {
        let (current, prune, uncle_source) = self.prepare()?;
        let source = current
            .source
            .as_ref()
            .ok_or(Error::Fault("template source"))?;
        let retired = self.store.read_selected(source.view, &source.reads, || {
            let mut uncles = self.assembler.candidate_uncles.lock();
            if uncles.source_receipt() != uncle_source {
                return Err(Error::Stale);
            }
            // Cache pruning and output replacement are both internal bounded
            // synchronous mutations. Payload destruction follows guard release.
            let pruned = uncles.prune(prune);
            Ok((
                pruned,
                std::mem::replace(&mut *self.assembler.current.write(), Arc::clone(&current)),
            ))
        })??;
        drop(retired);
        self.notification.notify_one();
        Ok(())
    }
    pub(super) async fn run(self: Arc<Self>) -> Result<(), Error> {
        let mut first = true;
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
            let outcome = block_offload(|| self.rebuild());
            let stale = outcome
                .as_ref()
                .err()
                .and_then(|error| error.downcast_ref::<Error>())
                .is_some_and(|error| matches!(error, Error::Stale));
            self.failed
                .store(outcome.is_err() && !stale, Ordering::Release);
            if outcome
                .as_ref()
                .err()
                .and_then(|error| error.downcast_ref::<Error>())
                .is_some_and(|error| matches!(error, Error::Fault(_)))
            {
                self.store.fault();
            }
            if let Err(error) = &outcome
                && !stale
            {
                ckb_logger::error!("template build failed: {error}");
            }
            self.updated.notify_waiters();
            if stale {
                continue;
            }
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
