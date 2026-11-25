//! Synthetic owner-state fixtures; mutations use production Store or membership
//! paths but do not establish canonical VM validity. `entry` constructs an owner,
//! `insert` commits that owner, and `accept` seeds membership using synthetic
//! verification data. Canonical validity needs separate checks with real chain
//! cells and the production verifier.
use super::super::{
    jobs::{self, Verified},
    membership::{self, AdmissionOptions},
    model::{Entry, Error, OutPointKey, Phase, Premises, RemoteOrigin, Resolved, Source, Status},
    notice::{Batch, Class},
    state::OwnerEdit,
    store::Store,
};
use crate::{TxEntry, error::Reject};
use ckb_app_config::{BlockAssemblerConfig, TxPoolConfig};
use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_jsonrpc_types::ScriptHashType;
use ckb_network::PeerIndex;
use ckb_snapshot::Snapshot;
use ckb_store::{ChainStore, attach_block_cell};
use ckb_test_chain_utils::{
    MockStore, always_success_cell, always_success_consensus, create_always_success_out_point,
};
use ckb_types::{
    U256,
    bytes::Bytes,
    core::{
        BlockBuilder, BlockExt, Capacity, FeeRate, TransactionBuilder, TransactionView,
        cell::{CellMeta, ResolvedTransaction},
    },
    h256,
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint},
    prelude::*,
    utilities::merkle_mountain_range::ChainRootMMR,
};
use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{Duration, Instant},
};

/// Nextest gives each caller its own process and logger. Keep the capture
/// bounded so a diagnostic loop fails instead of hiding unbounded retention.
pub(in crate::authority) fn capture_rejections() -> &'static ckb_util::Mutex<Vec<serde_json::Value>>
{
    use ckb_logger::internal::{LevelFilter, Log, Metadata, Record};
    static RECORDS: std::sync::OnceLock<ckb_util::Mutex<Vec<serde_json::Value>>> =
        std::sync::OnceLock::new();
    struct Capture;
    impl Log for Capture {
        fn enabled(&self, metadata: &Metadata<'_>) -> bool {
            metadata.target() == "ckb_tx_pool::rejection"
        }

        fn log(&self, record: &Record<'_>) {
            if self.enabled(record.metadata()) {
                let value = serde_json::from_str(&record.args().to_string())
                    .expect("rejection diagnostic is valid JSON");
                let mut records = RECORDS.get().unwrap().lock();
                assert!(records.len() < 128, "bounded rejection capture");
                records.push(value);
            }
        }

        fn flush(&self) {}
    }
    let records = RECORDS.get_or_init(|| ckb_util::Mutex::new(Vec::with_capacity(128)));
    ckb_logger::internal::set_logger(&Capture).expect("one logger per isolated test");
    ckb_logger::internal::set_max_level(LevelFilter::Debug);
    records
}

pub(in crate::authority) fn diagnostic_hash(hash: &Byte32) -> String {
    let hash: ckb_types::H256 = hash.unpack();
    format!("{hash:x}")
}

pub(in crate::authority) fn config() -> TxPoolConfig {
    TxPoolConfig {
        min_fee_rate: FeeRate::zero(),
        min_rbf_rate: FeeRate::zero(),
        max_tx_pool_size: 4_000_000,
        max_tx_verify_workers: 2,
        max_ancestors_count: 128,
        ..TxPoolConfig::default()
    }
}

pub(in crate::authority) fn store() -> Arc<Store> {
    store_with_pipeline_limit(
        crate::test_support::genesis_snapshot(),
        &config(),
        64_000_000,
    )
}

/// Exercise resource accounting with controlled fixture residency.
pub(in crate::authority) fn store_with_pipeline_limit(
    snapshot: Arc<Snapshot>,
    config: &TxPoolConfig,
    pipeline: usize,
) -> Arc<Store> {
    let limits = super::super::budget::Limits::with_residency(
        config,
        snapshot.consensus(),
        crate::constants::ResidencyLimits {
            accepted: 32_000_000,
            pipeline,
        },
    )
    .expect("fixture residency limits are usable");
    Store::with_limits(snapshot, config, limits).expect("fixture store initializes")
}

pub(in crate::authority) fn tx(nonce: u32) -> TransactionView {
    TransactionBuilder::default().version(nonce).build()
}

pub(in crate::authority) fn output_tx(nonce: u32) -> TransactionView {
    TransactionBuilder::default()
        .version(nonce)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build()
}

pub(in crate::authority) fn spend(
    nonce: u32,
    inputs: &[OutPoint],
    deps: &[OutPoint],
) -> TransactionView {
    TransactionBuilder::default()
        .version(nonce)
        .inputs(inputs.iter().cloned().map(|point| CellInput::new(point, 0)))
        .cell_deps(
            deps.iter()
                .cloned()
                .map(|point| CellDep::new_builder().out_point(point).build()),
        )
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build()
}

pub(in crate::authority) fn entry(
    store: &Store,
    transaction: TransactionView,
    source: Source,
) -> Arc<Entry> {
    Entry::try_new(
        Arc::new(transaction),
        store.next_arrival().unwrap(),
        source,
        Phase::Resolve,
    )
    .unwrap()
}

pub(in crate::authority) fn commit_edits(
    store: &Store,
    edits: Vec<OwnerEdit>,
    extra_wakes: Vec<OutPoint>,
) -> Result<Option<Arc<Batch>>, Error> {
    let (_, batch) = store.commit_with(|tx| {
        for edit in &edits {
            let current = tx.state.owner(&edit.hash);
            let matches = match (&edit.before, current) {
                (Some(expected), Some(actual)) => Arc::ptr_eq(expected, actual),
                (None, None) => true,
                _ => false,
            };
            if !matches {
                return Err(Error::Stale);
            }
        }
        let wake_keys = extra_wakes
            .iter()
            .map(OutPointKey::from_out_point)
            .collect();
        tx.commit_edits(edits, wake_keys, Vec::new(), Class::Trusted)
    })?;
    Ok(batch)
}

pub(in crate::authority) type OwnerTransition = (Option<Arc<Entry>>, Option<Arc<Entry>>);

pub(in crate::authority) fn commit_pairs(
    store: &Store,
    pairs: Vec<OwnerTransition>,
) -> Result<Option<Arc<Batch>>, Error> {
    let edits = pairs
        .into_iter()
        .map(|(before, after)| {
            let hash = before
                .as_ref()
                .or(after.as_ref())
                .expect("pair must name an owner")
                .hash_bytes();
            OwnerEdit {
                hash,
                before,
                after,
                ancestors: None,
            }
        })
        .collect();
    commit_edits(store, edits, Vec::new())
}

pub(in crate::authority) fn insert(store: &Store, entry: Arc<Entry>) {
    commit_edits(
        store,
        vec![OwnerEdit {
            hash: entry.hash_bytes(),
            before: None,
            after: Some(entry),
            ancestors: None,
        }],
        Vec::new(),
    )
    .unwrap();
}

pub(in crate::authority) fn replace(store: &Store, before: Arc<Entry>, phase: Phase) -> Arc<Entry> {
    let after = before.with_phase(phase);
    commit_edits(
        store,
        vec![OwnerEdit {
            hash: before.hash_bytes(),
            before: Some(before),
            after: Some(Arc::clone(&after)),
            ancestors: None,
        }],
        Vec::new(),
    )
    .unwrap();
    after
}

pub(in crate::authority) fn install_snapshot(
    store: &Store,
    next: Arc<Snapshot>,
    attached_blocks: Vec<Arc<ckb_types::core::BlockView>>,
) -> Result<Option<Arc<Batch>>, Error> {
    let (_, batch) = store.commit_with(|tx| {
        let proposals: Option<Vec<_>> = tx
            .state
            .snapshot
            .proposals()
            .status_changes(next.proposals(), tx.state.usage.accepted.items)
            .map(Iterator::collect);
        tx.apply_lifecycle(next, proposals.as_deref(), &attached_blocks);
        Ok(())
    })?;
    Ok(batch)
}

/// Supply cells, fees and cycles for owner-state tests without running resolution
/// or script verification. Canonical verification tests use real chain cells.
pub(in crate::authority) fn verification_fixture(
    store: &Store,
    candidate: &Entry,
    fee: u64,
    cycles: u64,
    status: Status,
) -> Verified {
    let mut premises = Premises::default();
    let mut pool_cells = BTreeSet::new();
    let mut input_cell = |point: OutPoint| {
        store.spender(&point, &mut premises).unwrap();
        if store
            .get(&point.tx_hash(), &mut premises)
            .unwrap()
            .is_some_and(|entry| entry.accepted().is_some())
        {
            pool_cells.insert(point.clone());
        }
        CellMeta {
            out_point: point,
            ..CellMeta::default()
        }
    };
    let resolved_inputs = candidate
        .transaction
        .input_pts_iter()
        .map(&mut input_cell)
        .collect();
    let mut dep_cell = |point: OutPoint| {
        if store
            .get(&point.tx_hash(), &mut premises)
            .unwrap()
            .is_some_and(|entry| entry.accepted().is_some())
        {
            pool_cells.insert(point.clone());
        }
        CellMeta {
            out_point: point,
            ..CellMeta::default()
        }
    };
    let resolved = ResolvedTransaction {
        transaction: candidate.transaction.as_ref().clone(),
        resolved_inputs,
        resolved_cell_deps: candidate
            .transaction
            .cell_deps_iter()
            .map(|dep| dep_cell(dep.out_point()))
            .collect(),
        resolved_dep_groups: Vec::new(),
    };
    let size = candidate.transaction.data().serialized_size_in_block();
    let projection =
        TxEntry::new_with_timestamp(Arc::new(resolved), cycles, Capacity::shannons(fee), size, 0);
    jobs::fixture(
        store.snapshot().0,
        &projection,
        Arc::clone(&candidate.transaction),
        premises,
        pool_cells,
        status,
    )
}

/// Commit admission with synthetic verification; membership policy is production code.
pub(in crate::authority) fn commit_admission(
    store: &Store,
    candidate: &Arc<Entry>,
    fee: u64,
    cycles: u64,
    status: Status,
    config: &TxPoolConfig,
) -> Result<(Option<Arc<Batch>>, Option<Reject>), Error> {
    let before = store.point(&candidate.hash()).1;
    let verified = verification_fixture(store, candidate, fee, cycles, status);
    membership::commit_admission(
        store,
        candidate,
        before,
        &verified,
        config,
        AdmissionOptions::commit(),
    )
}

/// Preview admission without mutating live pool state or queuing effects.
pub(in crate::authority) fn preview_admission(
    store: &Store,
    candidate: &Arc<Entry>,
    fee: u64,
    cycles: u64,
    status: Status,
    config: &TxPoolConfig,
) -> Result<Option<Reject>, Error> {
    let before = store.point(&candidate.hash()).1;
    let verified = verification_fixture(store, candidate, fee, cycles, status);
    let (_, reject) = membership::commit_admission(
        store,
        candidate,
        before,
        &verified,
        config,
        AdmissionOptions::preview(),
    )?;
    Ok(reject)
}

/// Seed accepted ownership through the real admission and commit paths.
pub(in crate::authority) fn accept(
    store: &Store,
    transaction: TransactionView,
    fee: u64,
    cycles: u64,
    status: Status,
) -> Byte32 {
    let candidate = entry(store, transaction, Source::Local);
    let (_, reject) = commit_admission(store, &candidate, fee, cycles, status, &config()).unwrap();
    assert!(reject.is_none(), "fixture admission rejected: {reject:?}");
    candidate.hash()
}

pub(in crate::authority) fn chain_snapshot() -> Arc<Snapshot> {
    chain_store(Arc::new(always_success_consensus())).1
}

/// A real genesis cell set, also usable by tests that attach and detach blocks.
pub(crate) fn chain_store(consensus: Arc<Consensus>) -> (MockStore, Arc<Snapshot>) {
    let store = MockStore::default();
    let genesis = consensus.genesis_block();
    let epoch_ext = consensus.genesis_epoch_ext().clone();
    {
        let db_txn = store.store().begin_transaction();
        let previous_epoch_hash = epoch_ext.last_block_hash_in_previous_epoch();
        db_txn
            .insert_block(genesis)
            .expect("the fixture stores genesis");
        db_txn
            .attach_block(genesis)
            .expect("the fixture attaches genesis");
        attach_block_cell(&db_txn, genesis).expect("the fixture stores genesis cells");
        db_txn
            .insert_block_epoch_index(&genesis.hash(), &previous_epoch_hash)
            .expect("the fixture stores the epoch index");
        db_txn
            .insert_epoch_ext(&previous_epoch_hash, &epoch_ext)
            .expect("the fixture stores the epoch extension");
        db_txn
            .insert_block_ext(
                &genesis.hash(),
                &BlockExt {
                    received_at: 0,
                    total_difficulty: U256::zero(),
                    total_uncles_count: 0,
                    verified: Some(true),
                    txs_fees: vec![],
                    cycles: None,
                    txs_sizes: None,
                },
            )
            .expect("the fixture stores the block extension");
        let mut mmr = ChainRootMMR::new(0, &db_txn);
        mmr.push(genesis.digest())
            .expect("the fixture appends the genesis digest");
        mmr.commit().expect("the fixture commits the chain root");
        db_txn.commit().expect("the fixture commits genesis");
    }
    let snapshot = Arc::new(Snapshot::new(
        genesis.header(),
        U256::zero(),
        epoch_ext,
        store.store().get_snapshot(),
        Default::default(),
        consensus,
    ));
    (store, snapshot)
}

pub(crate) fn funded_tx(input: OutPoint, output_capacity: u64) -> TransactionView {
    TransactionBuilder::default()
        .input(CellInput::new(input, 0))
        .output(
            CellOutput::new_builder()
                .capacity(Capacity::shannons(output_capacity))
                .lock(always_success_cell().2.clone())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .cell_dep(
            CellDep::new_builder()
                .out_point(create_always_success_out_point())
                .build(),
        )
        .build()
}

pub(crate) fn funded_parent(nonce: u32, capacity: u64) -> TransactionView {
    TransactionBuilder::default()
        .version(nonce)
        .output(
            CellOutput::new_builder()
                .capacity(Capacity::shannons(capacity))
                .lock(always_success_cell().2.clone())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .build()
}

pub(crate) fn template_config(update_interval_millis: u64) -> BlockAssemblerConfig {
    BlockAssemblerConfig {
        code_hash: h256!("0x0"),
        args: Default::default(),
        hash_type: ScriptHashType::Data,
        message: Default::default(),
        use_binary_version_as_message_prefix: true,
        binary_version: "TEST".to_string(),
        update_interval_millis,
        notify: vec![],
        notify_scripts: vec![],
        notify_timeout_millis: 800,
        notify_auth_token: None,
    }
}

pub(crate) fn template_snapshot_with_child(child_timestamp: Option<u64>) -> Arc<Snapshot> {
    template_snapshot_with_consensus(
        child_timestamp,
        Arc::new(ConsensusBuilder::default().build()),
    )
}

pub(in crate::authority) fn template_snapshot_with_consensus(
    child_timestamp: Option<u64>,
    consensus: Arc<Consensus>,
) -> Arc<Snapshot> {
    let (store, base) = chain_store(Arc::clone(&consensus));
    let genesis = consensus.genesis_block();
    let epoch_ext = base.epoch_ext().clone();
    let child = child_timestamp.map(|timestamp| {
        BlockBuilder::default()
            .number(1)
            .parent_hash(genesis.hash())
            .timestamp(timestamp)
            .compact_target(genesis.compact_target())
            .epoch(epoch_ext.number_with_fraction(1))
            .dao(genesis.dao())
            .build()
    });
    if let Some(child) = &child {
        store.insert_block(child, &epoch_ext);
        let db_txn = store.store().begin_transaction();
        // Genesis occupies the first MMR node; append the height-one tip.
        let mut mmr = ChainRootMMR::new(1, &db_txn);
        mmr.push(child.digest())
            .expect("the fixture appends the child digest");
        mmr.commit().expect("the fixture commits the child root");
        db_txn.commit().expect("the fixture commits the child MMR");
        for position in 0..3 {
            assert!(
                store.store().get_header_digest(position).is_some(),
                "the fixture stores child MMR position {position}"
            );
        }
    }
    let tip = child.map_or_else(|| genesis.header(), |child| child.header());
    Arc::new(Snapshot::new(
        tip,
        U256::zero(),
        epoch_ext,
        store.store().get_snapshot(),
        Default::default(),
        consensus,
    ))
}

pub(in crate::authority) fn remote(peer: usize, cycles: u64) -> Source {
    Source::Remote {
        origin: RemoteOrigin {
            peer: PeerIndex::from(peer),
            deadline: Instant::now() + Duration::from_secs(60),
        },
        cycles: Some(cycles),
    }
}

pub(in crate::authority) fn delete(
    store: &Store,
    entry: Arc<Entry>,
) -> Result<Option<Arc<Batch>>, Error> {
    commit_edits(
        store,
        vec![OwnerEdit {
            hash: entry.hash_bytes(),
            before: Some(entry),
            after: None,
            ancestors: None,
        }],
        Vec::new(),
    )
}

pub(in crate::authority) fn queued(
    store: &Store,
    nonce: u32,
    source: Source,
    fee: u64,
) -> Arc<Entry> {
    let owner = entry(store, tx(nonce), source);
    owner.with_phase(Phase::Verify(Arc::new(Resolved {
        transaction: Arc::new(ResolvedTransaction {
            transaction: owner.transaction.as_ref().clone(),
            resolved_inputs: vec![],
            resolved_cell_deps: vec![],
            resolved_dep_groups: vec![],
        }),
        fee: Capacity::shannons(fee),
        view: store.snapshot().0,
        premises: Premises::default(),
        pool_cells: BTreeSet::new(),
    })))
}
