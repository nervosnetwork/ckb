//! Canonical resolution with bounded materialization and complete original reads.
//! Provider failures take precedence over the CellStatus::Unknown used to carry
//! them through the canonical CellProvider interface.

use super::super::{
    model::{DependencyKey, Entry, Error, Phase, Resolved},
    residency::detach_cell,
    store::{ReadSet, Store},
    waiting::pending_producer,
};
use super::Resolution;
use crate::{error::Reject, util::compact_packed, verification::check_tx_fee_with_min_fee_rate};
use ckb_app_config::TxPoolConfig;
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{
        DepType, TransactionView,
        cell::{
            CellMeta, CellProvider, CellStatus, HeaderChecker, ResolvedDep, ResolvedTransaction,
            SYSTEM_CELL, parse_dep_group_data, resolve_transaction,
        },
        error::OutPointError,
    },
    packed::OutPoint,
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashSet},
    mem::size_of,
    sync::Arc,
};

pub(in crate::authority) fn resolve(
    store: &Store,
    entry: &Entry,
    config: &TxPoolConfig,
) -> Result<Resolution, Error> {
    #[cfg(feature = "profiling")]
    let _span =
        tracing::trace_span!(target: "ckb_tx_pool_profile", "tx_pool.stage.resolve").entered();
    store.budget.limits.resolved_fits(entry)?;
    let (view, snapshot) = store.snapshot();
    let provider = Provider::new(store, &snapshot);
    let result = provider.resolve_transaction(&entry.transaction)?;
    match result {
        Ok(resolved) => {
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
            let (missing, mut observed) = provider.finish_missing(&entry.transaction)?;
            match missing {
                Ok(keys) if !keys.is_empty() => {
                    if entry.source.requires_known_producer() {
                        for key in &keys {
                            if let DependencyKey::Cell(point) = key
                                && !pending_producer(
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
impl<'a> Provider<'a> {
    fn new(store: &'a Store, snapshot: &'a Snapshot) -> Self {
        Self {
            store,
            snapshot,
            observed: RefCell::new(Observed::default()),
            max_bytes: store.budget.limits.per_job.bytes,
            max_edges: store.budget.limits.per_job.edges,
        }
    }

    /// Restore the provider error channel before interpreting canonical results.
    /// Successful resolution also observes cells supplied by the system cache.
    fn resolve_transaction(
        &self,
        transaction: &TransactionView,
    ) -> Result<Result<ResolvedTransaction, OutPointError>, Error> {
        let result = resolve_transaction(
            transaction.clone(),
            &mut HashSet::<OutPoint>::new(),
            self,
            self.snapshot,
        );
        if let Some(error) = self.error() {
            return Err(error);
        }
        if let Ok(resolved) = &result {
            for cell in resolved
                .resolved_inputs
                .iter()
                .chain(&resolved.resolved_cell_deps)
                .chain(&resolved.resolved_dep_groups)
            {
                if !self.observed.borrow().cells.contains_key(&cell.out_point) {
                    let _ = self.cell(&cell.out_point, false);
                }
            }
            if let Some(error) = self.error() {
                return Err(error);
            }
        }
        Ok(result)
    }

    /// Missing-frontier traversal can also lose a provider error behind Unknown.
    /// Deliver its domain result only after restoring that error channel.
    fn finish_missing(
        self,
        transaction: &TransactionView,
    ) -> Result<(Result<BTreeSet<DependencyKey>, Reject>, Observed), Error> {
        let missing = missing(transaction, &self);
        let mut observed = self.observed.into_inner();
        if let Some(error) = observed.error.take() {
            return Err(error);
        }
        Ok((missing, observed))
    }

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
        let data_bytes = if eager || cell.mem_cell_data.is_some() {
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
            // Store supplied data, exact length metadata and detached backing.
            state.pool.insert(compact_packed(point));
        } else {
            if eager {
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
            detach_cell(&mut cell);
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
        let members = parse_dep_group_data(data)
            .map_err(|_| Reject::Resolve(OutPointError::InvalidDepGroup(point)))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::{
        model::{FullReason, Status},
        tests::common::*,
    };
    use ckb_types::{
        bytes::Bytes,
        packed::{CellDep, CellOutput},
        prelude::*,
    };
    use std::collections::HashMap;

    #[test]
    fn pool_cell_detaches_all_backing_within_its_materialization_limit() {
        for data in [Bytes::new(), Bytes::from_static(b"pool cell data")] {
            let store = store();
            let parent = ckb_types::core::TransactionBuilder::default()
                .output(CellOutput::default())
                .output_data(data.pack())
                .output(CellOutput::default())
                .output_data(Bytes::from(vec![0x7a; 65_536]).pack())
                .build();
            accept(&store, parent.clone(), 1, 1, Status::Pending);
            let usage = store.budget.owner_usage();
            let point = OutPoint::new(parent.hash(), 0);
            let mut backing = vec![0; 4096];
            backing.extend_from_slice(point.as_slice());
            let backing = Bytes::from(backing);
            let point = OutPoint::new_unchecked(backing.slice(4096..));
            let mut reads = ReadSet::default();
            let limit = CellOutput::default().total_size() + data.len() + 256;
            assert!(matches!(
                store.pool_cell(&point, limit - 1, &mut reads),
                Err(Error::Full(FullReason::Other("pool cell materialization")))
            ));
            let cell = store.pool_cell(&point, limit, &mut reads).unwrap().unwrap();
            assert_eq!(cell.cell_output, CellOutput::default());
            assert_eq!(cell.out_point, point);
            assert_eq!(cell.transaction_info, None);
            assert_eq!(cell.data_bytes, data.len() as u64);
            assert_eq!(cell.mem_cell_data.as_ref(), Some(&data));
            assert_eq!(
                cell.mem_cell_data_hash,
                Some(CellOutput::calc_data_hash(&data))
            );
            let producer = parent.data();
            for source in [producer.as_slice(), backing.as_ref()] {
                let start = source.as_ptr() as usize;
                let range = start..start + source.len();
                for address in [
                    cell.cell_output.as_slice().as_ptr(),
                    cell.out_point.as_slice().as_ptr(),
                    cell.mem_cell_data.as_ref().unwrap().as_ptr(),
                    cell.mem_cell_data_hash
                        .as_ref()
                        .unwrap()
                        .as_slice()
                        .as_ptr(),
                ] {
                    assert!(!range.contains(&(address as usize)));
                }
            }
            assert_eq!(store.budget.owner_usage(), usage);
            assert!(store.budget.active_is_empty_for_test());
            store
                .read_selected(store.snapshot().0, &reads, || ())
                .unwrap();
        }
    }

    #[test]
    fn repeated_materialization_shares_one_precharged_detached_cell() {
        let store = store();
        let parent = ckb_types::core::TransactionBuilder::default()
            .version(1002u32)
            .output(CellOutput::default())
            .output_data(Bytes::from(vec![0x7a; 4096]).pack())
            .build();
        accept(&store, parent.clone(), 1, 1, Status::Pending);
        let (_, snapshot) = store.snapshot();
        let mut provider = Provider {
            store: &store,
            snapshot: &snapshot,
            observed: RefCell::new(Observed::default()),
            max_bytes: 100_000,
            max_edges: 4,
        };
        let point = OutPoint::new(parent.hash(), 0);
        let first = provider.materialize(&point, false).unwrap();
        provider.max_bytes = provider.observed.borrow().bytes;
        let second = provider.materialize(&point, true).unwrap();
        let (CellStatus::Live(first), CellStatus::Live(second)) = (first, second) else {
            panic!("accepted output is live")
        };
        assert_eq!(
            first.mem_cell_data.as_ref().unwrap().as_ptr(),
            second.mem_cell_data.as_ref().unwrap().as_ptr()
        );
        assert_eq!(provider.observed.borrow().cells.len(), 1);
        assert_eq!(provider.observed.borrow().bytes, provider.max_bytes);
    }

    #[test]
    fn chain_cell_upgrade_preserves_lazy_cache_until_its_data_fits() {
        let snapshot = chain_snapshot();
        let store = store_with_pipeline_limit(Arc::clone(&snapshot), &config(), 64_000_000);
        let point = ckb_test_chain_utils::create_always_success_out_point();
        let mut provider = Provider::new(&store, &snapshot);
        let CellStatus::Live(lazy) = provider.materialize(&point, false).unwrap() else {
            panic!("genesis cell is live")
        };
        assert!(lazy.mem_cell_data.is_none());
        assert!(lazy.data_bytes > 0);
        let lazy_bytes = provider.observed.borrow().bytes;
        let eager_bytes = lazy_bytes + usize::try_from(lazy.data_bytes).unwrap() * 4;
        provider.max_bytes = eager_bytes - 1;
        assert!(matches!(
            provider.materialize(&point, true),
            Err(Error::Full(FullReason::Other("active resolved cell bytes")))
        ));
        assert_eq!(provider.observed.borrow().bytes, lazy_bytes);
        assert!(
            provider.observed.borrow().cells[&point]
                .0
                .mem_cell_data
                .is_none()
        );

        provider.max_bytes = eager_bytes;
        let CellStatus::Live(loaded) = provider.materialize(&point, true).unwrap() else {
            panic!("genesis data fits the exact budget")
        };
        let data = loaded.mem_cell_data.as_ref().unwrap();
        assert_eq!(data, &ckb_test_chain_utils::always_success_cell().1);
        assert_eq!(provider.observed.borrow().bytes, eager_bytes);
        assert!(provider.observed.borrow().pool.is_empty());
        for eager in [false, true] {
            let CellStatus::Live(cached) = provider.materialize(&point, eager).unwrap() else {
                panic!("cached chain cell remains live")
            };
            assert_eq!(
                cached.mem_cell_data.as_ref().unwrap().as_ptr(),
                data.as_ptr()
            );
        }
        assert_eq!(provider.observed.borrow().bytes, eager_bytes);
        assert_eq!(provider.observed.borrow().cells.len(), 1);
    }

    #[test]
    fn chain_data_cannot_exceed_the_metadata_used_for_its_charge() {
        let (chain, snapshot) =
            chain_store(Arc::new(ckb_test_chain_utils::always_success_consensus()));
        let point = ckb_test_chain_utils::create_always_success_out_point();
        let (output, data, _) = ckb_test_chain_utils::always_success_cell();
        assert!(data.len() > 1);
        // Chain metadata and data come from separate columns; unlike pool
        // cells, their consistency is not established by one in-memory builder.
        let cell = ckb_types::packed::CellEntry::new_builder()
            .output(output.clone())
            .data_size(1u64)
            .build();
        let payload = ckb_types::packed::CellDataEntry::new_builder()
            .output_data(data.pack())
            .output_data_hash(CellOutput::calc_data_hash(data))
            .build();
        let transaction = chain.store().begin_transaction();
        transaction
            .insert_cells(std::iter::once((point.clone(), cell, Some(payload))))
            .unwrap();
        transaction.commit().unwrap();
        let snapshot = Arc::new(snapshot.refresh(chain.store().get_snapshot()));
        let store = store_with_pipeline_limit(Arc::clone(&snapshot), &config(), 64_000_000);
        let provider = Provider::new(&store, &snapshot);
        assert_eq!(provider.cell(&point, true), CellStatus::Unknown);
        assert!(matches!(
            provider.error(),
            Some(Error::Full(FullReason::Other(
                "cell data exceeds materialization metadata"
            )))
        ));
        assert!(provider.observed.borrow().cells.is_empty());
        assert!(provider.observed.borrow().bytes > 0);
    }

    #[test]
    fn materialization_keeps_current_spender_in_original_reads() {
        let store = store();
        let parent = output_tx(1010);
        let point = OutPoint::new(parent.hash(), 0);
        accept(&store, parent, 1, 1, Status::Pending);
        let consumer = accept(
            &store,
            spend(1011, std::slice::from_ref(&point), &[]),
            1,
            1,
            Status::Pending,
        );
        let (view, snapshot) = store.snapshot();
        let provider = Provider {
            store: &store,
            snapshot: &snapshot,
            observed: RefCell::new(Observed::default()),
            max_bytes: 100_000,
            max_edges: 4,
        };
        assert!(provider.materialize(&point, true).unwrap().is_live());
        let reads = provider.observed.into_inner().reads;
        assert_eq!(reads.spent().collect::<Vec<_>>(), vec![(&point, &consumer)]);
        let before = store.point(&consumer).1.unwrap();
        let mut plan = crate::authority::store::Plan::new(
            view,
            crate::authority::notice::Class::Trusted,
            Default::default(),
        );
        plan.edit(Some(before), None, None).unwrap();
        store.apply(plan).unwrap();
        assert!(matches!(
            store.read_selected(view, &reads, || ()),
            Err(Error::Stale)
        ));
    }

    #[test]
    fn canonical_unknown_cannot_hide_materialization_capacity() {
        let store = store();
        let (_, snapshot) = store.snapshot();
        let mut provider = Provider::new(&store, &snapshot);
        provider.max_edges = 0;
        let transaction = spend(1900, &[OutPoint::new(tx(1901).hash(), 0)], &[]);
        assert!(matches!(
            provider.resolve_transaction(&transaction),
            Err(Error::Full(_))
        ));
    }

    #[test]
    fn successful_system_cache_resolution_checks_its_added_observations() {
        let store = store();
        let (_, snapshot) = store.snapshot();
        let cached = SYSTEM_CELL.get_or_init(|| {
            let point = OutPoint::new(tx(1905).hash(), 0);
            let dep = CellDep::new_builder().out_point(point.clone()).build();
            HashMap::from([(
                dep,
                ResolvedDep::Cell(CellMeta {
                    out_point: point,
                    ..CellMeta::default()
                }),
            )])
        });
        let transaction = ckb_types::core::TransactionBuilder::default()
            .cell_dep(cached.keys().next().unwrap().clone())
            .build();
        let mut provider = Provider::new(&store, &snapshot);
        provider.max_edges = 0;
        let canonical = resolve_transaction(
            transaction.clone(),
            &mut HashSet::<OutPoint>::new(),
            &provider,
            snapshot.as_ref(),
        )
        .unwrap();
        assert!(!canonical.resolved_cell_deps.is_empty());
        assert!(
            provider.error().is_none(),
            "canonical cache bypassed the provider"
        );
        assert!(matches!(
            provider.resolve_transaction(&transaction),
            Err(Error::Full(_))
        ));
    }

    #[test]
    fn missing_frontier_cannot_hide_materialization_capacity_behind_a_header_error() {
        let store = store();
        let (_, snapshot) = store.snapshot();
        let mut provider = Provider::new(&store, &snapshot);
        provider.max_edges = 0;
        let transaction = spend(1902, &[OutPoint::new(tx(1903).hash(), 0)], &[])
            .as_advanced_builder()
            .header_dep(tx(1904).hash())
            .build();
        assert!(matches!(
            provider.finish_missing(&transaction),
            Err(Error::Full(_))
        ));
    }
}
