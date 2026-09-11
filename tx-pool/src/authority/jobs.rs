//! Queue selection gives one worker its exact owner and active reservation
//! through admission/discard. Selection does not rewrite the immutable owner.
use super::model::DependencyKey;
use super::{
    budget::ActivePermit,
    model::{Entry, Error, Phase, Resolved, Status, status},
    store::{ReadSet, Store},
};
use crate::{
    error::Reject,
    util::compact_packed,
    verification::{TxPoolVerificationBudget, check_tx_fee_with_min_fee_rate, verify_rtx},
};
use ckb_app_config::TxPoolConfig;
use ckb_script::{ChunkCommand, InitialProgramLoadLimit, TxPoolVmExecutionMode};
use ckb_snapshot::Snapshot;
use ckb_types::{
    bytes::Bytes,
    core::{
        DepType, TransactionView,
        cell::{
            CellMeta, CellProvider, CellStatus, HeaderChecker, ResolvedDep, ResolvedTransaction,
            SYSTEM_CELL, resolve_transaction,
        },
        error::OutPointError,
    },
    packed::{OutPoint, OutPointVec},
    prelude::*,
};
use ckb_verification::{
    TxVerifyEnv,
    cache::{ScriptVerificationRules, TxVerificationCache, TxVerificationCacheKey},
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashSet},
    mem::size_of,
    sync::Arc,
};
use tokio::sync::{RwLock, watch};

pub(super) struct Job {
    pub(super) store: Arc<Store>,
    pub(super) entry: Arc<Entry>,
    pub(super) view: u64,
    _memory: ActivePermit,
    done: bool,
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
            done: false,
        }
    }
    /// Mark this selection as handled; rejection, requeue and stale discard
    /// also complete a job. Its active reservation remains held until Job drops.
    pub(super) fn complete(&mut self) {
        self.done = true;
    }
    pub(super) fn current(&self) -> Result<bool, Error> {
        Ok(self
            .store
            .get(&self.entry.hash(), &mut ReadSet::default())?
            .is_some_and(|entry| Arc::ptr_eq(&entry, &self.entry)))
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        // Selection removed the queue item. Abandoning its still-current owner
        // would strand that work; a successor already owns its own scheduling.
        if !self.done && self.current().unwrap_or(true) {
            self.store.fault();
        }
    }
}

pub(super) enum Resolution {
    Ready(Arc<Resolved>),
    Waiting(BTreeSet<DependencyKey>, ReadSet),
    Rejected(Reject, ReadSet),
}
#[derive(Default)]
struct Observed {
    reads: ReadSet,
    cells: BTreeMap<OutPoint, (CellMeta, usize)>,
    pool: BTreeSet<OutPoint>,
    bytes: usize,
    error: Option<Error>,
}
struct Provider<'a> {
    store: &'a Store,
    snapshot: &'a Snapshot,
    observed: RefCell<Observed>,
    max_bytes: usize,
    max_edges: usize,
}
impl Provider<'_> {
    fn materialize(&self, point: &OutPoint, eager: bool) -> Result<CellStatus, Error> {
        let mut state = self.observed.borrow_mut();
        if let Some(error) = &state.error {
            return Err(error.clone());
        }
        if state.cells.len() >= self.max_edges && !state.cells.contains_key(point) {
            return Err(Error::Full("resolved dependency count".into()));
        }
        // Pool spends are admission policy; resolution keeps the live backing
        // and its original spender for the final checked replacement decision.
        let _ = self.store.spender(point, &mut state.reads)?;
        if let Some((cell, _)) = state.cells.get(point)
            && (!eager || cell.mem_cell_data.is_some() || cell.data_bytes == 0)
        {
            return Ok(CellStatus::live_cell(cell.clone()));
        }
        // Keep the conservative four-copy envelope for materialization, the
        // detached cache and lazy-to-eager replacement. Canonical resolution
        // shares these allocations across input, dep and dep-group occurrences;
        // retaining its result must not copy each occurrence again.
        const CELL_METADATA_BYTES: usize = size_of::<CellMeta>() * 4 + 256;
        let old_bytes = state.cells.get(point).map_or(0, |(_, bytes)| *bytes);
        let remaining = self
            .max_bytes
            .checked_sub(state.bytes)
            .and_then(|bytes| bytes.checked_add(old_bytes))
            .ok_or(Error::Full("active cell bytes".into()))?;
        let payload_limit = remaining.saturating_sub(CELL_METADATA_BYTES) / 4;
        let (mut cell, pool) = if let Some(cell) =
            self.store
                .pool_cell(point, payload_limit, &mut state.reads)?
        {
            (cell, true)
        } else {
            match self.snapshot.cell(point, false) {
                CellStatus::Live(cell) => (cell, false),
                status => return Ok(status),
            }
        };
        let data_bytes = if eager || pool || cell.mem_cell_data.is_some() {
            usize::try_from(cell.data_bytes).map_err(|_| Error::Full("cell data size".into()))?
        } else {
            0
        };
        let bytes = cell
            .cell_output
            .total_size()
            .checked_add(data_bytes)
            .and_then(|b| b.checked_mul(4))
            .and_then(|b| b.checked_add(CELL_METADATA_BYTES))
            .ok_or(Error::Full("cell byte arithmetic".into()))?;
        let next = state
            .bytes
            .checked_sub(old_bytes)
            .and_then(|sum| sum.checked_add(old_bytes.max(bytes)))
            .ok_or(Error::Full("active cell arithmetic".into()))?;
        if next > self.max_bytes {
            return Err(Error::Full("active resolved cell bytes".into()));
        }
        // Precharge the eager group load from canonical metadata before asking
        // the database for its bytes. Pool cells were already copied in-budget.
        state.bytes = next;
        if pool {
            state.pool.insert(compact_packed(point));
        }
        if eager && !pool {
            match self.snapshot.cell(point, true) {
                CellStatus::Live(loaded) => cell = loaded,
                status => return Ok(status),
            }
        }
        if cell
            .mem_cell_data
            .as_ref()
            .is_some_and(|data| data.len() > data_bytes)
        {
            return Err(Error::Full(
                "cell data exceeds materialization metadata".into(),
            ));
        }
        cell.cell_output = ckb_types::packed::CellOutput::new_unchecked(Bytes::copy_from_slice(
            cell.cell_output.as_slice(),
        ));
        cell.out_point = compact_packed(&cell.out_point);
        if let Some(info) = &mut cell.transaction_info {
            info.block_hash = compact_packed(&info.block_hash);
        }
        if let Some(data) = &cell.mem_cell_data {
            cell.mem_cell_data = Some(Bytes::copy_from_slice(data));
        }
        if let Some(hash) = &cell.mem_cell_data_hash {
            cell.mem_cell_data_hash = Some(compact_packed(hash));
        }
        state
            .cells
            .insert(compact_packed(point), (cell.clone(), bytes.max(old_bytes)));
        Ok(CellStatus::live_cell(cell))
    }
    fn error(&self) -> Option<Error> {
        self.observed.borrow().error.clone()
    }
}
impl CellProvider for Provider<'_> {
    fn cell(&self, point: &OutPoint, eager_load: bool) -> CellStatus {
        match self.materialize(point, eager_load) {
            Ok(status) => status,
            Err(error) => {
                self.observed.borrow_mut().error.get_or_insert(error);
                CellStatus::Unknown
            }
        }
    }
}

pub(super) fn resolve(
    store: &Store,
    entry: &Entry,
    config: &TxPoolConfig,
) -> Result<Resolution, Error> {
    #[cfg(feature = "profiling")]
    let _span =
        tracing::trace_span!(target: "ckb_tx_pool_profile", "tx_pool.stage.resolve").entered();
    store.budget.limits.resolved_fits(entry)?;
    let (view, snapshot) = store.snapshot();
    let provider = Provider {
        store,
        snapshot: &snapshot,
        observed: RefCell::new(Observed::default()),
        max_bytes: store.budget.limits.per_job.bytes,
        max_edges: store.budget.limits.per_job.edges,
    };
    let result = resolve_transaction(
        entry.transaction.as_ref().clone(),
        &mut HashSet::<OutPoint>::new(),
        &provider,
        snapshot.as_ref(),
    );
    if let Some(error) = provider.error() {
        return Err(error);
    }
    match result {
        Ok(resolved) => {
            // Canonical system-cell caches can bypass CellProvider. Capture
            // their original producer observations before retaining proof.
            for cell in resolved
                .resolved_inputs
                .iter()
                .chain(&resolved.resolved_cell_deps)
                .chain(&resolved.resolved_dep_groups)
            {
                if !provider
                    .observed
                    .borrow()
                    .cells
                    .contains_key(&cell.out_point)
                {
                    let _ = provider.cell(&cell.out_point, false);
                }
            }
            if let Some(error) = provider.error() {
                return Err(error);
            }
            let observed = provider.observed.into_inner();
            // Provider has detached dynamic cell payloads before returning
            // them. The only bypass is SYSTEM_CELL, whose immutable OnceLock
            // retains its backing for the process lifetime. Keep canonical
            // sharing here; copying every occurrence would duplicate payloads
            // before the complete resolved-owner budget check below.
            let transaction = Arc::new(resolved);
            let fee = match check_tx_fee_with_min_fee_rate(
                &snapshot,
                &transaction,
                entry.transaction.data().serialized_size_in_block(),
                config.min_fee_rate,
            ) {
                Ok(fee) => fee,
                Err(reject) => return Ok(Resolution::Rejected(reject, observed.reads)),
            };
            let resolved = Arc::new(Resolved {
                transaction,
                fee,
                view,
                reads: observed.reads.into_verification_reads(),
                pool_cells: observed.pool,
            });
            store
                .budget
                .limits
                .resolved_fits(&entry.with_phase(Phase::Verify(Arc::clone(&resolved))))?;
            Ok(Resolution::Ready(resolved))
        }
        Err(OutPointError::Unknown(_)) => {
            let missing = missing(&entry.transaction, &provider);
            if let Some(error) = provider.error() {
                return Err(error);
            }
            let mut observed = provider.observed.into_inner();
            match missing {
                Ok(keys) if !keys.is_empty() => {
                    if entry.source.requires_known_producer() {
                        for key in &keys {
                            if let DependencyKey::Cell(point) = key
                                && !super::waiting::pending_producer(
                                    store.get(&point.tx_hash(), &mut observed.reads)?.as_deref(),
                                    point,
                                )
                            {
                                return Ok(Resolution::Rejected(
                                    Reject::Resolve(OutPointError::Unknown(point.clone())),
                                    observed.reads,
                                ));
                            }
                        }
                    }
                    store
                        .budget
                        .limits
                        .resolved_fits(&entry.with_phase(Phase::Waiting(keys.clone())))?;
                    Ok(Resolution::Waiting(keys, observed.reads))
                }
                Ok(_) => Err(Error::Stale),
                Err(reject) => Ok(Resolution::Rejected(reject, observed.reads)),
            }
        }
        Err(error) => Ok(Resolution::Rejected(
            Reject::Resolve(error),
            provider.observed.into_inner().reads,
        )),
    }
}

fn missing(
    tx: &TransactionView,
    provider: &Provider<'_>,
) -> Result<BTreeSet<DependencyKey>, Reject> {
    fn inspect(
        status: CellStatus,
        point: OutPoint,
        keys: &mut BTreeSet<DependencyKey>,
    ) -> Result<Option<CellMeta>, Reject> {
        match status {
            CellStatus::Unknown => {
                keys.insert(DependencyKey::Cell(compact_packed(&point)));
                Ok(None)
            }
            CellStatus::Dead => Err(Reject::Resolve(OutPointError::Dead(point))),
            CellStatus::Live(cell) => Ok(Some(cell)),
        }
    }
    let mut keys = BTreeSet::new();
    for point in tx.input_pts_iter() {
        inspect(provider.cell(&point, false), point, &mut keys)?;
    }
    let mut edges = tx
        .inputs()
        .len()
        .checked_add(tx.header_deps().len())
        .ok_or_else(|| Reject::Full("dependency arithmetic".into()))?;
    for dep in tx.cell_deps_iter() {
        if let Some(cached) = SYSTEM_CELL.get().and_then(|system| system.get(&dep)) {
            let count = match cached {
                ResolvedDep::Cell(_) => 1,
                ResolvedDep::Group(_, cells) => cells
                    .len()
                    .checked_add(1)
                    .ok_or_else(|| Reject::Full("system dependency arithmetic".into()))?,
            };
            edges = edges
                .checked_add(count)
                .filter(|count| *count <= provider.max_edges)
                .ok_or_else(|| Reject::Full("dependency count".into()))?;
            continue;
        }
        edges = edges
            .checked_add(1)
            .filter(|count| *count <= provider.max_edges)
            .ok_or_else(|| Reject::Full("dependency count".into()))?;
        let point = dep.out_point();
        let group = dep.dep_type() == DepType::DepGroup.into();
        let Some(cell) = inspect(provider.cell(&point, group), point.clone(), &mut keys)? else {
            continue;
        };
        if !group {
            continue;
        }
        let data = cell
            .mem_cell_data
            .as_ref()
            .ok_or_else(|| Reject::Resolve(OutPointError::InvalidDepGroup(point.clone())))?;
        let members = OutPointVec::from_slice(data)
            .map_err(|_| Reject::Resolve(OutPointError::InvalidDepGroup(point.clone())))?;
        if members.is_empty() {
            return Err(Reject::Resolve(OutPointError::InvalidDepGroup(point)));
        }
        edges = edges
            .checked_add(members.len())
            .filter(|count| *count <= provider.max_edges)
            .ok_or_else(|| Reject::Full("expanded dependency count".into()))?;
        for member in members {
            inspect(provider.cell(&member, false), member, &mut keys)?;
        }
    }
    for hash in tx.header_deps_iter() {
        provider
            .snapshot
            .check_valid(&hash)
            .map_err(Reject::Resolve)?;
    }
    Ok(keys)
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
pub(super) fn context_sensitive(resolved: &ResolvedTransaction) -> bool {
    resolved
        .transaction
        .inputs()
        .into_iter()
        .any(|input| Into::<u64>::into(input.since()) != 0)
        || resolved
            .resolved_inputs
            .iter()
            .chain(&resolved.resolved_cell_deps)
            .any(|cell| {
                cell.transaction_info
                    .as_ref()
                    .is_some_and(|info| info.block_number > 0 && info.is_cellbase())
            })
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
    mode: TxPoolVmExecutionMode,
) -> Result<Verified, Error> {
    let (view, snapshot) = store.snapshot();
    if view != resolved.view {
        return Err(Error::Stale);
    }
    store.read_selected(view, &resolved.reads, || ())?;
    let environment = Arc::new(environment(status(&snapshot, &entry.proposal()), &snapshot));
    let rules = ScriptVerificationRules::from_env(snapshot.consensus(), &environment);
    let key = TxVerificationCacheKey::from_transaction(&entry.transaction, rules);
    let cached = cache.read().await.lookup(&key);
    let load = InitialProgramLoadLimit::new(config.max_tx_verify_initial_load_bytes)
        .ok_or(Error::Full("initial program load configuration".into()))?;
    let budget = TxPoolVerificationBudget::new(entry.source.duration(config), load)
        .with_vm_execution_mode(mode);
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
