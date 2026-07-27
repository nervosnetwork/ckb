//! End-to-end tests for the tx-pool resolve -> verify -> submit pipeline.

use crate::component::pool_map::Status;
use crate::process::PreCheckedTx;
use crate::service::TxPoolService;
use crate::tx_source::TxSource;
use ckb_app_config::{TxPoolConfig, VerifyOrdering};
use ckb_chain_spec::consensus::Consensus;
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_crypto::secp::Privkey;
use ckb_snapshot::Snapshot;
use ckb_stop_handler::CancellationToken;
use ckb_store::attach_block_cell;
use ckb_system_scripts::BUNDLED_CELL;
use ckb_test_chain_utils::{MockStore, always_success_cell};
use ckb_types::{
    H160, H256, U256,
    bytes::Bytes,
    core::{
        BlockBuilder, BlockExt, BlockView, Capacity, EpochNumberWithFraction, TransactionBuilder,
        TransactionView,
    },
    h160, h256,
    packed::{CellDep, CellInput, CellOutput, OutPoint, ProposalShortId, Script, WitnessArgs},
    prelude::*,
    utilities::difficulty_to_compact,
    utilities::merkle_mountain_range::ChainRootMMR,
};
use ckb_verification::{
    TxVerifyEnv,
    cache::{Completed, TxVerificationCacheKey},
};
use std::sync::Arc;
use std::time::Duration;

const MAX_TX_VERIFY_CYCLES: u64 = 70_000_000;
const ISSUE_OUTPUT_CAPACITY: u64 = 5_000;

mod dependency;
mod identity;
mod lifecycle;
mod template;

pub(crate) use dependency::service_with_rbf;
pub(crate) use identity::secp_test_consensus;
use identity::{
    SECP_FEE, SECP_ISSUE_CAPACITY, build_secp_tx, measured_cycles,
    secp_service_with_pipeline_workers, submit_local_tx, verify_cycles,
};

pub(crate) fn tx_pool_config() -> TxPoolConfig {
    TxPoolConfig {
        max_tx_pool_size: 180_000_000,
        max_tx_pool_resident_size: 1_000_000_000,
        min_fee_rate: ckb_types::core::FeeRate::zero(),
        min_rbf_rate: ckb_types::core::FeeRate::zero(),
        max_tx_verify_cycles: MAX_TX_VERIFY_CYCLES,
        max_tx_verify_workers: 2,
        max_ancestors_count: 125,
        keep_rejected_tx_hashes_days: 1,
        keep_rejected_tx_hashes_count: 1000,
        persisted_data: Default::default(),
        recent_reject: Default::default(),
        expiry_hours: 24,
        verify_ordering: VerifyOrdering::ArrivalTime,
        max_tx_pipeline_resident_size: 384_000_000,
    }
}

pub(crate) fn test_consensus(issue_outputs: usize) -> (Consensus, Vec<OutPoint>) {
    let (always_success_cell, always_success_data, always_success_script) = always_success_cell();

    let always_success_tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .output(always_success_cell.clone())
        .output_data(always_success_data)
        .witness(always_success_script.clone().into_witness())
        .build();

    let issue_output = CellOutput::new_builder()
        .capacity(Capacity::bytes(ISSUE_OUTPUT_CAPACITY as usize).unwrap())
        .lock(always_success_script.clone())
        .build();
    let issue_tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .outputs((0..issue_outputs).map(|_| issue_output.clone()))
        .outputs_data((0..issue_outputs).map(|_| Bytes::default().pack()))
        .build();

    let issue_out_points: Vec<_> = (0..issue_outputs)
        .map(|i| OutPoint::new(issue_tx.hash(), i as u32))
        .collect();

    let dao = ckb_dao_utils::genesis_dao_data(vec![&always_success_tx, &issue_tx]).unwrap();
    let genesis = BlockBuilder::default()
        .timestamp(1_557_310_743u64)
        .compact_target(difficulty_to_compact(U256::from(1000u64)))
        .dao(dao)
        .transaction(always_success_tx)
        .transaction(issue_tx)
        .build();

    let consensus = ConsensusBuilder::default()
        .genesis_block(genesis)
        .cellbase_maturity(EpochNumberWithFraction::new(0, 0, 1))
        .build();

    (consensus, issue_out_points)
}

pub(crate) fn snapshot_with_genesis(consensus: Arc<Consensus>) -> (MockStore, Arc<Snapshot>) {
    let store = MockStore::default();
    let genesis = consensus.genesis_block();
    let epoch_ext = consensus.genesis_epoch_ext().clone();

    {
        let db_txn = store.store().begin_transaction();
        let last_block_hash_in_previous_epoch = epoch_ext.last_block_hash_in_previous_epoch();
        db_txn.insert_block(genesis).unwrap();
        db_txn.attach_block(genesis).unwrap();
        attach_block_cell(&db_txn, genesis).unwrap();
        db_txn
            .insert_block_epoch_index(&genesis.hash(), &last_block_hash_in_previous_epoch)
            .unwrap();
        db_txn
            .insert_epoch_ext(&last_block_hash_in_previous_epoch, &epoch_ext)
            .unwrap();
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
            .unwrap();
        let mut mmr = ChainRootMMR::new(0, &db_txn);
        mmr.push(genesis.digest()).unwrap();
        mmr.commit().unwrap();
        db_txn.commit().unwrap();
    }

    let snap = Arc::new(Snapshot::new(
        genesis.header(),
        U256::zero(),
        epoch_ext,
        store.store().get_snapshot(),
        Default::default(),
        consensus,
    ));
    (store, snap)
}

fn always_success_script() -> ckb_types::packed::Script {
    always_success_cell().2.clone()
}

fn always_success_dep() -> CellDep {
    CellDep::new_builder()
        .out_point(ckb_test_chain_utils::create_always_success_out_point())
        .build()
}

fn service_with_pipeline(
    issue_outputs: usize,
) -> (
    TxPoolService,
    ckb_channel::Receiver<crate::service::TxVerificationResult>,
    CancellationToken,
    MockStore,
    Vec<OutPoint>,
) {
    let h = super::harness::harness(issue_outputs).build();
    (h.service, h.relay_rx, h.cancel, h.store, h.out_points)
}

fn service_with_pipeline_workers(
    issue_outputs: usize,
    max_workers: usize,
) -> (
    TxPoolService,
    ckb_channel::Receiver<crate::service::TxVerificationResult>,
    CancellationToken,
    MockStore,
    Vec<OutPoint>,
) {
    let h = super::harness::harness(issue_outputs)
        .max_workers(max_workers)
        .build();
    (h.service, h.relay_rx, h.cancel, h.store, h.out_points)
}

fn build_tx(input: &OutPoint, output_capacity: usize) -> TransactionView {
    TransactionBuilder::default()
        .cell_dep(always_success_dep())
        .input(CellInput::new(input.clone(), 0))
        .output(
            CellOutput::new_builder()
                .capacity(Capacity::bytes(output_capacity).unwrap())
                .lock(always_success_script())
                .build(),
        )
        .output_data(Bytes::default().pack())
        .build()
}

/// Keep the network ingress parameters visible at each test call site without
/// repeating the `TxSource::Remote` construction in every scenario.
async fn submit_remote(
    service: &TxPoolService,
    tx: TransactionView,
    cycles: u64,
    peer: ckb_network::PeerIndex,
) -> Result<bool, crate::error::Reject> {
    service
        .submit_remote_tx(tx, TxSource::Remote { cycles, peer })
        .await
}

/// Wait for the accepted-pool count while preserving a test-specific failure
/// message at the call site. Polling stays test-only and uses a fixed short
/// interval; production liveness remains event-driven.
async fn wait_for_pending(
    service: &TxPoolService,
    expected: usize,
    timeout: Duration,
) -> Result<(), tokio::time::error::Elapsed> {
    tokio::time::timeout(timeout, async {
        loop {
            if service.pool.tx_pool.read().await.pool_map.pending_size() == expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
}

/// Drive one pipeline transaction to the coordinator's verified boundary
/// without spawning workers. Tests can then exercise the production commit
/// sequencer deterministically.
async fn stage_verified_candidate(service: &TxPoolService, tx: TransactionView, source: TxSource) {
    use crate::component::pre_pool::{ResolveLane, WorkCapability};

    match source {
        TxSource::Remote { peer, .. } => {
            let cycles = measured_cycles(service, tx.clone()).await;
            submit_remote(service, tx.clone(), cycles, peer)
                .await
                .unwrap();
        }
        TxSource::Proposal => {
            service.notify_tx(tx.clone()).await.unwrap();
        }
        TxSource::Local => panic!("Local submissions bypass the pre-pool"),
    }
    let raw = service
        .pipeline
        .kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap()
        .unwrap();
    service.process_pipeline_raw_lease(raw).await;
    let verify = service
        .pipeline
        .kernel
        .mutate(|coordinator| coordinator.checkout_verify(WorkCapability::Any))
        .unwrap()
        .unwrap();
    let snapshot = service.pool.tx_pool.read().await.cloned_snapshot();
    let verified = service
        .verify_pipeline_resolved((*verify.payload).clone(), snapshot, None)
        .await
        .unwrap();
    let charge = verified
        .candidate
        .resident_size
        .checked_add(std::mem::size_of::<
            crate::component::pre_pool::PipelineVerifiedTx,
        >())
        .unwrap();
    service
        .pipeline
        .kernel
        .mutate(|coordinator| coordinator.complete_verify(&verify, verified, charge))
        .unwrap();
}

async fn stage_verified_remote_candidate(
    service: &TxPoolService,
    tx: TransactionView,
    peer: ckb_network::PeerIndex,
) {
    stage_verified_candidate(service, tx, TxSource::Remote { cycles: 0, peer }).await;
}

fn with_cached_hash(tx: TransactionView, hash: ckb_types::packed::Byte32) -> TransactionView {
    ckb_types::packed::TransactionView::new_builder()
        .data(tx.data())
        .hash(hash)
        .witness_hash(tx.witness_hash())
        .build()
        .unpack()
}
