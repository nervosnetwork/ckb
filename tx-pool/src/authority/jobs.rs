//! Queue selection gives one worker its exact owner and active reservation
//! through admission/discard. Selection does not rewrite the immutable owner.
use super::model::DependencyKey;
use super::{
    budget::ActivePermit,
    model::{Entry, Error, Resolved, Status, status},
    service::ComputePermit,
    store::{ReadSet, Store},
};
use crate::{
    error::Reject,
    verification::{TxPoolVerificationBudget, verify_rtx},
};
use ckb_app_config::TxPoolConfig;
use ckb_script::ChunkCommand;
use ckb_snapshot::Snapshot;
#[cfg(any(test, feature = "internal"))]
use ckb_types::{core::TransactionView, packed::OutPoint};
use ckb_verification::{
    TxVerifyEnv,
    cache::{ScriptVerificationRules, TxVerificationCache, TxVerificationCacheKey},
};
use std::{collections::BTreeSet, sync::Arc};
use tokio::sync::{RwLock, watch};

mod resolution;
pub(super) use resolution::resolve;

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
    Waiting(BTreeSet<DependencyKey>, ReadSet),
    Rejected(Reject, ReadSet),
}

/// Only canonical contextual/VM verification constructs a successful result.
#[derive(Clone)]
pub(super) struct Verified {
    resolved: Arc<Resolved>,
    cycles: u64,
    timestamp: u64,
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
        self.cycles
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

#[cfg_attr(
    feature = "profiling",
    tracing::instrument(
        level = "trace",
        target = "ckb_tx_pool_profile",
        name = "tx_pool.stage.verify",
        skip_all
    )
)]
pub(super) async fn verify(
    store: &Store,
    entry: &Entry,
    resolved: Arc<Resolved>,
    config: &TxPoolConfig,
    cache: &RwLock<TxVerificationCache>,
    commands: &mut watch::Receiver<ChunkCommand>,
    compute: &ComputePermit,
) -> Result<Verified, Error> {
    let (view, snapshot) = store.snapshot();
    if view != resolved.view {
        return Err(Error::Stale);
    }
    store.read_selected(view, &resolved.reads, || ())?;
    let environment = Arc::new(environment(status(&snapshot, &entry.proposal()), &snapshot));
    let rules = ScriptVerificationRules::from_env(snapshot.consensus(), &environment);
    let key = TxVerificationCacheKey::from_resolved(&resolved.transaction, rules);
    let cached = cache.read().await.lookup(&key);
    let budget = entry
        .source
        .verification_time_limit(config)
        .map(|limit| TxPoolVerificationBudget::new(limit, compute.mode()));
    let max_cycles = entry
        .source
        .declared_cycles()
        .unwrap_or_else(|| snapshot.consensus().max_block_cycles());
    let timestamp = ckb_systemtime::unix_time_as_millis();
    let outcome = verify_rtx(
        snapshot,
        Arc::clone(&resolved.transaction),
        environment,
        cached,
        max_cycles,
        commands,
        budget,
    )
    .await?;
    if let Some(declared) = entry.source.declared_cycles()
        && outcome.cycles() != declared
    {
        return Err(Reject::DeclaredWrongCycles(declared, outcome.cycles()).into());
    }
    if let Some(proof) = outcome.executed_proof() {
        cache.write().await.insert(proof);
    }
    Ok(Verified {
        resolved,
        cycles: outcome.cycles(),
        timestamp,
        #[cfg(any(test, feature = "internal"))]
        fixture: None,
    })
}

#[cfg(any(test, feature = "internal"))]
pub(super) fn fixture(
    view: u64,
    entry: &crate::TxEntry,
    transaction: Arc<TransactionView>,
    reads: ReadSet,
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
            reads,
            pool_cells,
        }),
        cycles: entry.cycles,
        timestamp: entry.timestamp,
        fixture: Some((status, entry.size)),
    }
}

#[cfg(test)]
#[path = "tests/verification.rs"]
mod tests;
