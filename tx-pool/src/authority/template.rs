//! One template driver builds outside Store guards and publishes only while
//! its lifecycle, selected owners and uncle source remain current.

use super::{
    model::{Entry, Error, hash_key},
    packing::{Cache, Selection, TemplatePackingLimits},
    state::{FastHashSet, HashKeyBuildHasher},
    store::{Captured, Store, read_rwlock_until},
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
use ckb_types::{packed::ProposalShortId, prelude::Entity};
use ckb_verification::cache::{ScriptVerificationProof, TxVerificationCache};
use std::{
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::{Notify, RwLock},
    task::JoinSet,
};

/// Validity follows the lifecycle and selected transaction/proposal owners.
/// When `removal_revision` matches `AuthorityState::accepted_removal_revision`,
/// no previously accepted owner could have been removed or replaced, enabling
/// O(1) validation without scanning `selected_owners`.
pub(crate) struct TemplateSource {
    pub(super) view: u64,
    pub(super) removal_revision: u64,
    pub(super) selected_owners: Vec<([u8; 32], Weak<Entry>)>,
}

impl TemplateSource {
    /// Only final DAO transactions and selected proposals constrain publication.
    /// A proposal short-ID collision observes every matching captured owner.
    fn from_content(
        view: u64,
        removal_revision: u64,
        selection: &Selection<'_>,
        transactions: &[TxEntry],
        proposals: &[ProposalShortId],
    ) -> Result<(Self, Vec<ScriptVerificationProof>), Error> {
        let mut selected_owners =
            Vec::with_capacity(transactions.len().saturating_add(proposals.len()));
        let mut proofs = Vec::with_capacity(transactions.len());
        let mut selected: FastHashSet<[u8; 32]> =
            FastHashSet::with_capacity_and_hasher(transactions.len(), HashKeyBuildHasher);
        selected.extend(
            transactions
                .iter()
                .map(|transaction| hash_key(transaction.transaction().hash_ref())),
        );
        let mut proposals_set: FastHashSet<[u8; ProposalShortId::TOTAL_SIZE]> =
            FastHashSet::with_capacity_and_hasher(proposals.len(), HashKeyBuildHasher);
        for proposal in proposals {
            if let Some(bytes) = proposal
                .as_slice()
                .first_chunk::<{ ProposalShortId::TOTAL_SIZE }>()
            {
                proposals_set.insert(*bytes);
            }
        }
        for candidate in selection.candidates() {
            let key = candidate.owner().hash_bytes();
            let transaction = selected.remove(&key);
            if transaction && let Some(proof) = candidate.accepted.scripts.proof() {
                proofs.push(proof);
            }
            if transaction || proposals_set.contains(&super::model::proposal_key(&key)) {
                selected_owners.push((key, Arc::downgrade(candidate.owner())));
            }
        }
        if let Some(hash) = selected.iter().min() {
            let hash = ckb_types::packed::Byte32::new(*hash);
            ckb_logger::error!("selected transaction {hash} is outside template candidates");
            return Err(Error::Fault(
                "selected transaction outside template candidates",
            ));
        }
        Ok((
            Self {
                view,
                removal_revision,
                selected_owners,
            },
            proofs,
        ))
    }
}

/// One build and the exact sources that authorize its publication and pruning.
struct PreparedTemplate {
    current: Arc<CurrentTemplate>,
    prune: CandidateUnclePrune,
    proofs: Vec<ScriptVerificationProof>,
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
    cache: Arc<RwLock<TxVerificationCache>>,
    max_ancestors: usize,
    pub(super) requested: Notify,
    updated: Notify,
    notification: Notify,
    failed: AtomicBool,
    #[cfg(test)]
    build_response_hook: ckb_util::parking_lot::Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}
impl Driver {
    pub(super) fn new(
        store: Arc<Store>,
        assembler: BlockAssembler,
        cache: Arc<RwLock<TxVerificationCache>>,
        max_ancestors: usize,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            assembler,
            cache,
            max_ancestors,
            requested: Notify::new(),
            updated: Notify::new(),
            notification: Notify::new(),
            failed: AtomicBool::new(false),
            #[cfg(test)]
            build_response_hook: ckb_util::parking_lot::Mutex::new(None),
        })
    }

    #[cfg(test)]
    pub(super) fn set_build_response_hook(&self, hook: Option<Arc<dyn Fn() + Send + Sync>>) {
        *self.build_response_hook.lock() = hook;
    }

    pub(super) fn start(self: &Arc<Self>, tasks: &mut JoinSet<Result<(), Error>>) {
        tasks.spawn(Arc::clone(self).run());
        if self.assembler.notifications_enabled() {
            tasks.spawn(Arc::clone(self).notify());
        }
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

    /// Validate only the currently published template handle against the store,
    /// without cloning its JSON payload yet.
    fn current_validated(
        &self,
        deadline: tokio::time::Instant,
    ) -> Result<Option<Arc<CurrentTemplate>>, Error> {
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
        let current = read_rwlock_until(&self.assembler.current, deadline, || {
            if self.store.is_faulted() {
                Err(Error::Fault("template generation"))
            } else if self.store.is_stopped() {
                Err(Error::Closed)
            } else {
                Ok(())
            }
        })?
        .clone();
        let Some(current) = current else {
            return Ok(None);
        };
        match self
            .store
            .read_template_until(&current.source, deadline, || ())
        {
            Ok(()) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(Error::Full("template refresh timeout".into()));
                }
                Ok(Some(current))
            }
            Err(Error::Stale) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(Error::Full("template refresh timeout".into()));
                }
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// Deep-clone a validated template into an RPC response, enforcing the
    /// same absolute `deadline` both before and immediately after construction.
    fn materialize_response(
        &self,
        current: &CurrentTemplate,
        deadline: tokio::time::Instant,
    ) -> Result<JsonBlockTemplate, Error> {
        if tokio::time::Instant::now() >= deadline {
            return Err(Error::Full("template refresh timeout".into()));
        }
        #[cfg(test)]
        {
            let hook = self.build_response_hook.lock().clone();
            if let Some(hook) = hook {
                hook();
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(Error::Full("template refresh timeout".into()));
        }
        let mut output = current.json.clone();
        output.current_time = unix_time_as_millis()
            .max(current.template.current_time)
            .into();
        if tokio::time::Instant::now() >= deadline {
            return Err(Error::Full("template refresh timeout".into()));
        }
        Ok(output)
    }

    /// Validate only the currently published work and materialize its response
    /// within `deadline`. A stale or absent source leaves refresh scheduling
    /// to the waiting reader.
    #[cfg(test)]
    pub(super) fn current(
        &self,
        deadline: tokio::time::Instant,
    ) -> Result<Option<JsonBlockTemplate>, Error> {
        let Some(current) = self.current_validated(deadline)? else {
            return Ok(None);
        };
        self.materialize_response(&current, deadline).map(Some)
    }

    pub(super) async fn read(
        &self,
        deadline: tokio::time::Instant,
    ) -> Result<JsonBlockTemplate, Error> {
        let mut attempted = false;
        loop {
            let updated = self.updated.notified();
            if let Some(current) = self.current_validated(deadline)? {
                return block_offload(|| self.materialize_response(&current, deadline));
            }
            if attempted && self.failed.load(Ordering::SeqCst) {
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
        let captured = self.store.capture_accepted();
        let selection = packing.selection_captured(&captured, self.max_ancestors)?;
        let Captured {
            view,
            ref snapshot,
            accepted_removal_revision,
            ..
        } = captured;
        let epoch = snapshot
            .consensus()
            .next_epoch_ext(snapshot.tip_header(), &snapshot.borrow_as_data_loader())
            .ok_or(BlockAssemblerError::MissingTipEpoch)?
            .epoch();
        let (prepared, prune) = self
            .assembler
            .prepare_uncles(snapshot, &epoch)
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
                .as_ref()
                .filter(|current| current.source.view == view)
                .map(|current| {
                    (
                        current.template.cellbase.clone(),
                        current.template.extension.clone(),
                    )
                })
        };
        let (cellbase, extension) = match reused {
            Some(parts) => parts,
            None => (
                BlockAssembler::build_cellbase(&self.assembler.config, snapshot)?,
                BlockAssembler::build_extension(snapshot)?,
            ),
        };
        let maximum = BlockAssembler::max_block_bytes(snapshot)?;
        let fixed_size = BlockAssembler::basic_block_size(
            cellbase.data(),
            &[],
            std::iter::empty(),
            extension.clone(),
        );
        if fixed_size > maximum {
            return Err(BlockAssemblerError::Overflow.into());
        }
        let block = selection.for_block().map_err(Error::from)?;
        let proposals = block.proposal_short_ids(snapshot.consensus().max_block_proposals_limit());
        let optional = BlockAssembler::fit_optional_content(
            snapshot, proposals, prepared, fixed_size, maximum,
        )
        .ok_or(BlockAssemblerError::Overflow)?;
        let selected = block
            .pack_transactions(TemplatePackingLimits::new(
                maximum
                    .checked_sub(optional.total_size)
                    .ok_or(BlockAssemblerError::Overflow)?,
                snapshot.consensus().max_block_cycles(),
            ))
            .map_err(Error::from)?;
        let (dao, transactions) = BlockAssembler::calc_dao(
            snapshot,
            &epoch,
            cellbase.clone(),
            selected,
            &self.assembler.cell_liveness_memo,
        )?;
        let (source, proofs) = TemplateSource::from_content(
            view,
            accepted_removal_revision,
            &selection,
            &transactions,
            &optional.proposals,
        )?;
        // Compute DAO only for the final selected contents. Work IDs and time
        // belong to this publication attempt, not the reused mandatory parts.
        let mut template = BlockTemplate::new(
            snapshot,
            &epoch,
            cellbase,
            BlockAssembler::take_counter(&self.assembler.work_id, "work id")?,
            dao,
            unix_time_as_millis(),
        )?;
        template.extension = extension;
        template.transactions = transactions
            .into_iter()
            .map(|entry| (entry.transaction().clone(), entry.cycles))
            .collect();
        template.proposals = optional.proposals;
        template.uncles = optional.uncles;
        let json: JsonBlockTemplate = (&template).into();
        Ok(PreparedTemplate {
            current: Arc::new(CurrentTemplate {
                template,
                json,
                source,
            }),
            prune,
            proofs,
        })
    }

    fn rebuild(&self, packing: &mut Cache) -> Result<(), BuildError> {
        block_offload(|| {
            self.publish(self.prepare(packing)?)?;
            Ok(())
        })
    }

    fn publish(&self, prepared: PreparedTemplate) -> Result<(), Error> {
        let PreparedTemplate {
            current,
            prune,
            proofs,
        } = prepared;
        // An accepted owner keeps its sealed proof even after shared-cache
        // eviction. Refresh only the final selected transactions before miners
        // can submit this template. Cache contention must not delay publication;
        // a miss still takes the canonical VM path. No Store guard is held here.
        if !proofs.is_empty()
            && let Ok(cache) = self.cache.try_read()
        {
            for proof in proofs {
                cache.insert_concurrent(proof);
            }
        }
        let retired: Result<_, CandidateUnclePrune> =
            self.store.read_template(&current.source, || {
                let mut uncles = self.assembler.candidate_uncles.lock();
                let pruned = uncles.try_prune(prune)?;
                // Cache pruning and output replacement are both internal bounded
                // synchronous mutations. Payload destruction follows guard release.
                Ok((
                    pruned,
                    self.assembler.current.write().replace(Arc::clone(&current)),
                ))
            })?;
        // A stale plan also owns uncle payloads; discard it only after the
        // selected-source guards have opened.
        let retired = retired.map_err(|_stale| Error::Stale)?;
        drop(retired);
        if self.assembler.notifications_enabled() {
            self.notification.notify_one();
        }
        Ok(())
    }

    async fn coalesce(&self, view: u64) {
        let delay = tokio::time::sleep(Duration::from_millis(100));
        let requested = self.requested.notified();
        tokio::pin!(delay, requested);
        requested.as_mut().enable();
        loop {
            // Lifecycle and terminal changes share this state wake with
            // maintenance. Recheck after subscribing; an unrelated state wake
            // must neither end the wait nor renew its deadline.
            let changed = self.store.changed.notified();
            if self.store.is_faulted() || self.store.is_stopped() || self.store.snapshot().0 != view
            {
                return;
            }
            tokio::select! {
                _ = &mut requested => return,
                _ = &mut delay => return,
                _ = changed => {},
            }
        }
    }

    pub(super) async fn run(self: Arc<Self>) -> Result<(), Error> {
        let mut coalesce = false;
        let mut packing = Cache::default();
        loop {
            if coalesce {
                let view = self.store.snapshot().0;
                let current_view = self
                    .assembler
                    .current
                    .read()
                    .as_ref()
                    .map(|current| current.source.view);
                if current_view == Some(view) {
                    self.coalesce(view).await;
                }
            }
            // The wait's subscriptions have been dropped. Subscribe again
            // before capture so changes during construction remain observable.
            let changed = self.store.template_changed.notified();
            let requested = self.requested.notified();
            tokio::pin!(changed, requested);
            if requested.as_mut().enable() {
                // This pending request is covered by the imminent capture.
                // Rearm once; later requests must not postpone that capture.
                requested.set(self.requested.notified());
                requested.as_mut().enable();
            }
            if self.store.is_faulted() {
                self.updated.notify_waiters();
                return Err(Error::Fault("template generation"));
            }
            if self.store.is_stopped() {
                self.updated.notify_waiters();
                return Ok(());
            }
            let outcome = self.rebuild(&mut packing);
            let stale = matches!(outcome, Err(BuildError::Pool(Error::Stale)));
            self.failed
                .store(outcome.is_err() && !stale, Ordering::SeqCst);
            match outcome {
                Ok(()) => {}
                Err(BuildError::Pool(Error::Stale)) => {
                    self.updated.notify_waiters();
                    // Only publication can become stale after a coherent capture.
                    // Let concurrent commits finish before recapturing, retaining
                    // the same-view coalescing policy for stale attempts.
                    coalesce = true;
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
            coalesce = tokio::select! {
                biased;
                _ = &mut requested => false,
                _ = &mut changed => true,
            };
        }
    }

    async fn notify(self: Arc<Self>) -> Result<(), Error> {
        loop {
            // Stop and fault wake state waiters. Only a published template
            // needs to wake this task for ordinary membership changes.
            let changed = self.store.changed.notified();
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
