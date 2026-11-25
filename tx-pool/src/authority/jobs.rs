//! Queue selection gives one worker its exact owner and active reservation
//! through admission/discard. Selection does not rewrite the immutable owner.
use super::{
    budget::ActivePermit,
    model::{Entry, Error, Premises, Resolved, Source, Status, VerifiedScripts, status},
    service::{ComputePermit, ComputePool},
    store::Store,
};
use crate::{
    error::Reject,
    verification::{script_group_count, try_verify_rtx_sync, verify_rtx_with_compute},
};
use ckb_app_config::TxPoolConfig;
use ckb_script::ChunkCommand;
use ckb_snapshot::Snapshot;
#[cfg(any(test, feature = "internal"))]
use ckb_types::core::TransactionView;
use ckb_types::packed::OutPoint;
use ckb_verification::{TxVerifyEnv, cache::TxVerificationCache};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use tokio::sync::{RwLock, watch};

mod resolution;
pub(super) use resolution::{prefetch_points, resolve, resolve_for_view, resolve_prefetched};

pub(super) struct Job {
    pub(super) store: Arc<Store>,
    pub(super) entry: Arc<Entry>,
    pub(super) view: u64,
    _memory: ActivePermit,
    handled: bool,
}
impl Job {
    /// Store constructs this only from an item removed by its queue. The
    /// active reservation and non-Clone Job carry that selection to completion.
    pub(super) fn new(
        store: Arc<Store>,
        entry: Arc<Entry>,
        view: u64,
        memory: ActivePermit,
    ) -> Self {
        Self {
            store,
            entry,
            view,
            _memory: memory,
            handled: false,
        }
    }

    /// The worker acknowledges successful settlement before leaving its frame.
    /// The active reservation remains held until Job and its results drop.
    pub(super) fn mark_handled(&mut self) {
        self.handled = true;
    }

    pub(super) fn current(&self) -> bool {
        self.store.is_current(&self.entry)
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        // Selection removed the queue item. Abandoning its still-current owner
        // would strand that work; a successor already owns its own scheduling.
        if !self.handled && self.current() {
            self.store.fault();
        }
    }
}

pub(super) enum Resolution {
    Ready(Arc<Resolved>),
    Waiting(BTreeSet<OutPoint>, Premises),
    Rejected(Reject, Premises),
}

/// Only canonical contextual/VM verification constructs a successful result.
#[derive(Clone)]
pub(super) struct Verified {
    resolved: Arc<Resolved>,
    scripts: VerifiedScripts,
    timestamp: u64,
    spent_cpu: Duration,
    // Trusted internal fixtures supply status and serialized size before policy
    // planning; live verification derives both from the chain/transaction.
    #[cfg(any(test, feature = "internal"))]
    fixture: Option<(Status, usize)>,
}
impl Verified {
    pub(super) fn resolved(&self) -> &Arc<Resolved> {
        &self.resolved
    }

    pub(super) fn cycles(&self) -> u64 {
        self.scripts.cycles()
    }

    pub(super) fn scripts(&self) -> VerifiedScripts {
        self.scripts
    }

    pub(super) fn spent_cpu(&self) -> Duration {
        self.spent_cpu
    }

    #[cfg(any(test, feature = "internal"))]
    pub(super) fn forced_status(&self) -> Option<Status> {
        self.fixture.map(|(status, _)| status)
    }

    pub(super) fn serialized_size(&self) -> usize {
        #[cfg(any(test, feature = "internal"))]
        if let Some((_, size)) = self.fixture {
            return size;
        }
        self.resolved
            .transaction
            .transaction
            .data()
            .serialized_size_in_block()
    }

    pub(super) fn timestamp(&self) -> u64 {
        self.timestamp
    }
}

pub(super) fn environment(status: Status, snapshot: &Snapshot) -> TxVerifyEnv {
    match status {
        Status::Pending => TxVerifyEnv::new_submit(snapshot.tip_header()),
        Status::Gap => {
            TxVerifyEnv::new_proposed(snapshot.tip_header(), crate::constants::GAP_PROPOSAL_INDEX)
        }
        Status::Proposed => TxVerifyEnv::new_proposed(
            snapshot.tip_header(),
            snapshot
                .consensus()
                .tx_proposal_window()
                .closest()
                .saturating_sub(1),
        ),
    }
}

/// Synchronous local verification executed inside the caller's existing
/// `block_in_place` frame, avoiding a second runtime offload and redundant
/// `snapshot_checked` read lock. Returns `None` only if the outer cache RwLock
/// is write-locked.
pub(super) fn try_verify_local_sync(
    #[cfg(any(test, feature = "internal"))] store: &Store,
    snapshot: &Arc<Snapshot>,
    entry: &Entry,
    resolved: Arc<Resolved>,
    cache: &RwLock<TxVerificationCache>,
) -> Option<Result<Verified, Error>> {
    #[cfg(any(test, feature = "internal"))]
    store.run_verify_observer(entry.transaction.hash_ref());
    let environment = Arc::new(environment(status(snapshot, &entry.proposal()), snapshot));
    let max_cycles = snapshot.consensus().max_block_cycles();
    let result = try_verify_rtx_sync(
        snapshot,
        &resolved.transaction,
        environment,
        cache,
        max_cycles,
    )?;
    Some(match result {
        Ok((outcome, timestamp)) => Ok(Verified {
            resolved,
            scripts: VerifiedScripts::Vm(outcome.proof()),
            timestamp,
            spent_cpu: Duration::ZERO,
            #[cfg(any(test, feature = "internal"))]
            fixture: None,
        }),
        Err(reject) => Err(reject.into()),
    })
}

#[cfg(test)]
pub(super) async fn verify(
    store: &Store,
    entry: &Entry,
    resolved: Arc<Resolved>,
    config: &TxPoolConfig,
    cache: &RwLock<TxVerificationCache>,
    commands: &mut watch::Receiver<ChunkCommand>,
    // Retain admitted CPU capacity through canonical verification and its VM child.
    compute: &ComputePermit,
) -> Result<Verified, Error> {
    verify_with_compute(
        store, entry, resolved, config, cache, commands, compute, None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
#[cfg_attr(
    feature = "profiling",
    tracing::instrument(
        level = "trace",
        target = "ckb_tx_pool_profile",
        name = "tx_pool.stage.verify",
        skip_all
    )
)]
pub(super) async fn verify_with_compute(
    store: &Store,
    entry: &Entry,
    resolved: Arc<Resolved>,
    config: &TxPoolConfig,
    cache: &RwLock<TxVerificationCache>,
    commands: &mut watch::Receiver<ChunkCommand>,
    // Retain admitted CPU capacity through canonical verification and its VM child.
    compute: &ComputePermit,
    compute_pool: Option<&ComputePool>,
) -> Result<Verified, Error> {
    let snapshot = store.snapshot_checked(resolved.view, &resolved.premises)?;
    verify_with_snapshot(
        #[cfg(any(test, feature = "internal"))]
        store,
        snapshot,
        entry,
        resolved,
        config,
        cache,
        commands,
        compute,
        compute_pool,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn verify_with_snapshot(
    #[cfg(any(test, feature = "internal"))] store: &Store,
    snapshot: Arc<Snapshot>,
    entry: &Entry,
    resolved: Arc<Resolved>,
    config: &TxPoolConfig,
    cache: &RwLock<TxVerificationCache>,
    commands: &mut watch::Receiver<ChunkCommand>,
    _compute: &ComputePermit,
    compute_pool: Option<&ComputePool>,
) -> Result<Verified, Error> {
    #[cfg(any(test, feature = "internal"))]
    let before_prepare = store.verify_observer_task(entry.transaction.hash_ref());
    let environment = Arc::new(environment(status(&snapshot, &entry.proposal()), &snapshot));
    let max_cycles = entry
        .source
        .declared_cycles()
        .unwrap_or_else(|| snapshot.consensus().max_block_cycles());
    let groups = script_group_count(&resolved.transaction);
    let time_limit = entry
        .source
        .verification_time_limit_for_groups(config, groups);
    let (outcome, timestamp, spent_cpu) = match verify_rtx_with_compute(
        snapshot,
        Arc::clone(&resolved.transaction),
        environment,
        cache,
        max_cycles,
        commands,
        time_limit,
        compute_pool,
        #[cfg(any(test, feature = "internal"))]
        before_prepare,
    )
    .await
    {
        Ok(result) => result,
        Err(Reject::ExcessiveVerifyTime) => {
            crate::metrics::record_verify_time_budget_exceeded(matches!(
                entry.source,
                Source::Proposal { .. }
            ));
            return Err(Reject::ExcessiveVerifyTime.into());
        }
        Err(reject) => return Err(reject.into()),
    };
    if let Some(declared) = entry.source.declared_cycles()
        && outcome.cycles() != declared
    {
        return Err(Reject::DeclaredWrongCycles(declared, outcome.cycles()).into());
    }
    if let Some(proof) = outcome.executed_proof() {
        match cache.try_read() {
            Ok(guard) => guard.insert_concurrent(proof),
            Err(_) => cache.read().await.insert_concurrent(proof),
        }
    }
    Ok(Verified {
        resolved,
        scripts: VerifiedScripts::Vm(outcome.proof()),
        timestamp,
        spent_cpu,
        #[cfg(any(test, feature = "internal"))]
        fixture: None,
    })
}

#[cfg(any(test, feature = "internal"))]
pub(super) fn fixture(
    view: u64,
    entry: &crate::TxEntry,
    transaction: Arc<TransactionView>,
    premises: Premises,
    pool_cells: BTreeSet<OutPoint>,
    status: Status,
) -> Verified {
    let mut resolved = entry.rtx.as_ref().clone();
    resolved.transaction = transaction.as_ref().clone();
    Verified {
        resolved: Arc::new(super::model::Resolved {
            transaction: super::residency::compact_fixture_resolution(resolved),
            fee: entry.fee,
            view,
            premises,
            pool_cells,
        }),
        scripts: VerifiedScripts::Fixture(entry.cycles),
        timestamp: entry.timestamp,
        spent_cpu: Duration::ZERO,
        fixture: Some((status, entry.size)),
    }
}

#[cfg(test)]
#[path = "tests/verification.rs"]
mod tests;
