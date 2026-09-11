//! Small input fixtures; every mutation uses the production Store or membership path.
use super::super::{
    jobs::{self, Verified},
    membership,
    model::{Entry, Error, Phase, Resolved, Source, Status},
    notice::Class,
    store::{Plan, ReadSet, Store},
};
use crate::{TxEntry, error::Reject};
use ckb_app_config::TxPoolConfig;
use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
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
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint},
    prelude::*,
    utilities::merkle_mountain_range::ChainRootMMR,
};
use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{Duration, Instant},
};

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
    Arc::new(Entry {
        transaction: Arc::new(transaction),
        arrival: store.next_arrival().unwrap(),
        source,
        phase: Phase::Resolve,
    })
}
pub(in crate::authority) fn insert(store: &Store, entry: Arc<Entry>) {
    let mut plan = Plan::new(store.snapshot().0, Class::Trusted, Default::default());
    plan.edit(None, Some(entry), None).unwrap();
    store.apply(plan).unwrap();
}
pub(in crate::authority) fn replace(store: &Store, before: Arc<Entry>, phase: Phase) -> Arc<Entry> {
    let after = before.with_phase(phase);
    let mut plan = Plan::new(store.snapshot().0, Class::Trusted, Default::default());
    plan.edit(Some(before), Some(Arc::clone(&after)), None)
        .unwrap();
    store.apply(plan).unwrap();
    after
}
pub(in crate::authority) fn verified(
    store: &Store,
    candidate: &Entry,
    fee: u64,
    cycles: u64,
    status: Status,
) -> Verified {
    let mut reads = ReadSet::default();
    let mut pool_cells = BTreeSet::new();
    let mut cell = |point: OutPoint| {
        if store
            .get(&point.tx_hash(), &mut reads)
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
        resolved_inputs: candidate
            .transaction
            .input_pts_iter()
            .map(&mut cell)
            .collect(),
        resolved_cell_deps: candidate
            .transaction
            .cell_deps_iter()
            .map(|dep| cell(dep.out_point()))
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
        reads,
        pool_cells,
        status,
    )
}
pub(in crate::authority) fn admission(
    store: &Store,
    candidate: &Arc<Entry>,
    fee: u64,
    cycles: u64,
    status: Status,
    config: &TxPoolConfig,
) -> Result<(Plan, Option<Reject>), Error> {
    let before = store.point(&candidate.hash()).1;
    let verified = verified(store, candidate, fee, cycles, status);
    membership::admission(store, candidate, before, &verified, config, true)
}
pub(in crate::authority) fn accept(
    store: &Store,
    transaction: TransactionView,
    fee: u64,
    cycles: u64,
    status: Status,
) -> Byte32 {
    let candidate = entry(store, transaction, Source::Local);
    let (plan, reject) = admission(store, &candidate, fee, cycles, status, &config()).unwrap();
    assert!(reject.is_none(), "fixture admission rejected: {reject:?}");
    store.apply(plan).unwrap();
    candidate.hash()
}

pub(in crate::authority) fn chain_snapshot() -> Arc<Snapshot> {
    let consensus = Arc::new(always_success_consensus());
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
        db_txn.commit().expect("the fixture commits genesis");
    }
    Arc::new(Snapshot::new(
        genesis.header(),
        U256::zero(),
        epoch_ext,
        store.store().get_snapshot(),
        Default::default(),
        consensus,
    ))
}

pub(in crate::authority) fn funded_tx(input: OutPoint, output_capacity: u64) -> TransactionView {
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

pub(in crate::authority) fn funded_parent(nonce: u32, capacity: u64) -> TransactionView {
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

pub(in crate::authority) fn template_snapshot_with_child(
    child_timestamp: Option<u64>,
) -> Arc<Snapshot> {
    template_snapshot_with_consensus(
        child_timestamp,
        Arc::new(ConsensusBuilder::default().build()),
    )
}

pub(in crate::authority) fn template_snapshot_with_consensus(
    child_timestamp: Option<u64>,
    consensus: Arc<Consensus>,
) -> Arc<Snapshot> {
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
        peer: PeerIndex::from(peer),
        deadline: Instant::now() + Duration::from_secs(60),
        cycles: Some(cycles),
    }
}

pub(in crate::authority) fn delete(store: &Store, entry: Arc<Entry>) -> Plan {
    let mut plan = Plan::new(store.snapshot().0, Class::Trusted, Default::default());
    plan.edit(Some(entry), None, None).unwrap();
    plan
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
        reads: ReadSet::default(),
        pool_cells: BTreeSet::new(),
    })))
}
