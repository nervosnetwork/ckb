use ckb_types::core::{BlockBuilder, BlockNumber, EpochNumberWithFraction};

use crate::block_assembler::candidate_uncles::{
    CandidateUncles, MAX_CANDIDATE_UNCLES, MAX_PER_HEIGHT,
};

use super::CellLivenessMemo;
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_snapshot::Snapshot;
use ckb_store::attach_block_cell;
use ckb_test_chain_utils::MockStore;
use ckb_types::{
    U256,
    core::{BlockExt, cell::CellChecker},
    packed::{Byte32, OutPoint},
};
use std::{collections::HashSet, sync::Arc};

fn genesis_snapshot() -> Arc<Snapshot> {
    let consensus = Arc::new(ConsensusBuilder::default().build());
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
        db_txn.commit().unwrap();
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

#[test]
fn cell_liveness_memo_caches_and_invalidates_on_tip_change() {
    let snapshot = genesis_snapshot();
    // The cellbase output of the genesis block is live in the snapshot.
    let live_out_point =
        snapshot.consensus().genesis_block().transactions()[0].output_pts()[0].clone();
    let unknown_out_point = OutPoint::new(Byte32::zero(), 0);

    let mut memo = CellLivenessMemo::default();
    // First lookup populates the memo and matches a direct snapshot query.
    assert_eq!(memo.get_or_load(&snapshot, &live_out_point), Some(true));
    assert_eq!(memo.inner.len(), 1);
    // Second lookup is served from the memo without growing it.
    assert_eq!(memo.get_or_load(&snapshot, &live_out_point), Some(true));
    assert_eq!(memo.inner.len(), 1);

    // Unknown out-points are memoized as not-live.
    assert_eq!(memo.get_or_load(&snapshot, &unknown_out_point), None);
    assert_eq!(memo.inner.len(), 2);
    assert_eq!(memo.get_or_load(&snapshot, &unknown_out_point), None);
    assert_eq!(memo.inner.len(), 2);

    // A tip change clears the memo automatically.
    memo.tip_hash = Some(Byte32::zero());
    assert_eq!(memo.get_or_load(&snapshot, &live_out_point), Some(true));
    assert_eq!(memo.inner.len(), 1);
    assert_eq!(
        memo.get_or_load(&snapshot, &live_out_point),
        snapshot.is_live(&live_out_point)
    );
}

#[test]
fn test_candidate_uncles_basic() {
    let mut candidate_uncles = CandidateUncles::new();
    let block = &BlockBuilder::default().build().as_uncle();
    assert!(candidate_uncles.insert(block.clone()));
    assert_eq!(candidate_uncles.len(), 1);
    // insert duplicate
    assert!(!candidate_uncles.insert(block.clone()));
    assert_eq!(candidate_uncles.len(), 1);

    assert!(candidate_uncles.remove_by_number(block));
    assert_eq!(candidate_uncles.len(), 0);
    assert_eq!(candidate_uncles.map.len(), 0);
}

#[test]
fn test_candidate_uncles_max_size() {
    let mut candidate_uncles = CandidateUncles::new();

    let mut blocks = Vec::new();
    for i in 0..(MAX_CANDIDATE_UNCLES + 3) {
        let number = i as BlockNumber;
        let block = BlockBuilder::default()
            .number(number)
            .epoch(EpochNumberWithFraction::new(
                number / 1000,
                number % 1000,
                10000,
            ))
            .build()
            .as_uncle();
        blocks.push(block);
    }

    for block in &blocks {
        candidate_uncles.insert(block.clone());
    }
    let first_key = *candidate_uncles.map.keys().next().unwrap();
    assert_eq!(candidate_uncles.len(), MAX_CANDIDATE_UNCLES);
    assert_eq!(first_key, 3);

    candidate_uncles.clear();
    for block in blocks.iter().rev() {
        candidate_uncles.insert(block.clone());
    }
    let first_key = *candidate_uncles.map.keys().next().unwrap();
    assert_eq!(candidate_uncles.len(), MAX_CANDIDATE_UNCLES);
    assert_eq!(first_key, 3);
}

#[test]
fn test_candidate_uncles_max_per_height() {
    let mut candidate_uncles = CandidateUncles::new();

    let mut blocks = Vec::new();
    for i in 0..(MAX_PER_HEIGHT + 3) {
        let block = BlockBuilder::default()
            .timestamp(i as u64)
            .build()
            .as_uncle();
        blocks.push(block);
    }

    for block in &blocks {
        candidate_uncles.insert(block.clone());
    }
    assert_eq!(candidate_uncles.map.len(), 1);
    assert_eq!(candidate_uncles.len(), MAX_PER_HEIGHT);
}

/// Bug #49: `uncle_size` must use the same accounting basis as
/// `basic_block_size` and the consensus block-bytes limit:
/// `serialized_size_in_block` minus the proposal ids (which are
/// accounted separately in the template's proposals section).
#[test]
fn uncle_size_matches_basic_block_size_basis() {
    use ckb_types::packed::ProposalShortId;

    // An uncle with no proposals: size == serialized_size_in_block.
    let bare = BlockBuilder::default()
        .number(1)
        .epoch(EpochNumberWithFraction::new(0, 0, 1))
        .build()
        .as_uncle();
    assert_eq!(
        super::BlockAssembler::uncle_size(&bare),
        ckb_types::core::UncleBlockView::serialized_size_in_block()
    );

    // An uncle with proposals: size == serialized_size_in_block - proposals * id_size.
    let proposals: Vec<ProposalShortId> = (0..3u8)
        .map(|i| ProposalShortId::from_tx_hash(&Byte32::new([i; 32])))
        .collect();
    let with_proposals = BlockBuilder::default()
        .number(2)
        .epoch(EpochNumberWithFraction::new(0, 0, 1))
        .proposals(proposals.clone())
        .build()
        .as_uncle();
    let expected = ckb_types::core::UncleBlockView::serialized_size_in_block()
        - proposals.len() * ProposalShortId::serialized_size();
    assert_eq!(
        super::BlockAssembler::uncle_size(&with_proposals),
        expected,
        "uncle_size must subtract proposal ids from the in-block size"
    );
    assert!(
        super::BlockAssembler::uncle_size(&with_proposals)
            < super::BlockAssembler::uncle_size(&bare),
        "an uncle with proposals must account for fewer bytes than a bare one"
    );
}

/// Bug #56: `prepare_uncles` must remove candidates that are already on
/// the main chain or embedded as an uncle, instead of retaining them
/// until the epoch boundary.
#[test]
fn prepare_uncles_removes_main_chain_and_embedded_candidates() {
    let snapshot = genesis_snapshot();
    let consensus = snapshot.consensus();
    let epoch_ext = consensus.genesis_epoch_ext().clone();

    let mut candidate_uncles = CandidateUncles::new();

    // The genesis block IS on the main chain of this snapshot.
    let genesis_uncle = consensus.genesis_block().as_uncle();
    candidate_uncles.insert(genesis_uncle.clone());
    assert_eq!(candidate_uncles.len(), 1);

    // A block that is NOT on the main chain (random hash).
    let off_chain = BlockBuilder::default()
        .number(0)
        .epoch(EpochNumberWithFraction::new(0, 0, 1).full_value())
        .parent_hash(consensus.genesis_block().hash())
        .build()
        .as_uncle();
    candidate_uncles.insert(off_chain.clone());
    assert_eq!(candidate_uncles.len(), 2);

    let uncles = candidate_uncles.prepare_uncles(&snapshot, &epoch_ext);

    // The genesis uncle is on the main chain: must be removed.
    assert!(
        !candidate_uncles.contains(&genesis_uncle),
        "main-chain candidate must be removed by prepare_uncles"
    );
    // The off-chain candidate is eligible (its parent is genesis which is
    // on the main chain) and is returned as a valid uncle.
    assert_eq!(uncles.len(), 1);
    assert_eq!(uncles[0].hash(), off_chain.hash());
    // It is retained in the candidate set (not removed, just not eligible
    // for removal — it's a valid uncle that was selected).
    assert!(candidate_uncles.contains(&off_chain));
}

/// A Pending proposal must win over an optional uncle carrying the same id.
/// If that uncle is removed, descendants that depended on it solely through
/// the in-template uncle chain must be removed too; unrelated valid uncles
/// remain available.
#[test]
fn pending_proposals_filter_conflicting_uncle_subtree() {
    use ckb_types::packed::ProposalShortId;

    let snapshot = genesis_snapshot();
    let genesis = snapshot.consensus().genesis_block();
    let epoch = snapshot
        .consensus()
        .genesis_epoch_ext()
        .number_with_fraction(1);
    let pending_id = ProposalShortId::from_tx_hash(&Byte32::new([1; 32]));
    let other_id = ProposalShortId::from_tx_hash(&Byte32::new([2; 32]));

    let conflicting = BlockBuilder::default()
        .number(1)
        .epoch(epoch)
        .parent_hash(genesis.hash())
        .proposals(vec![pending_id.clone()])
        .build()
        .as_uncle();
    let independent = BlockBuilder::default()
        .number(1)
        .epoch(epoch)
        .timestamp(1)
        .parent_hash(genesis.hash())
        .proposals(vec![other_id])
        .build()
        .as_uncle();
    let descendant = BlockBuilder::default()
        .number(2)
        .epoch(epoch)
        .parent_hash(conflicting.hash())
        .build()
        .as_uncle();
    let uncles = vec![conflicting.clone(), independent.clone(), descendant.clone()];

    let all = super::BlockAssembler::filter_uncles_conflicting_with_proposals(
        &snapshot,
        &uncles,
        &HashSet::new(),
    );
    assert_eq!(all, uncles, "a conflict-free uncle chain must be preserved");

    let filtered = super::BlockAssembler::filter_uncles_conflicting_with_proposals(
        &snapshot,
        &uncles,
        &HashSet::from([pending_id]),
    );
    assert_eq!(filtered, vec![independent]);
    assert!(!filtered.contains(&conflicting));
    assert!(!filtered.contains(&descendant));
}

#[test]
fn proposal_update_keeps_highest_scored_fitting_prefix() {
    use ckb_types::packed::ProposalShortId;

    let mut proposals: Vec<ProposalShortId> = (0..3u8)
        .map(|byte| ProposalShortId::from_tx_hash(&Byte32::new([byte; 32])))
        .collect();
    let id_size = ProposalShortId::serialized_size();
    let base = 1_000;
    let (proposal_bytes, total) =
        super::BlockAssembler::fit_proposal_prefix(&mut proposals, base, base + 2 * id_size)
            .expect("base template fits");

    assert_eq!(proposals.len(), 2);
    assert_eq!(proposal_bytes, 2 * id_size);
    assert_eq!(total, base + 2 * id_size);
}

/// Bug #37: both full rebuilds and uncle-only refreshes must participate in
/// the same serialization domain. Otherwise a full rebuild can read the old
/// uncle set, race an uncle refresh, and unconditionally swap the old set back.
#[tokio::test]
async fn full_and_uncle_updates_share_template_serialization_lock() {
    use crate::pool::TxPool;
    use ckb_app_config::{BlockAssemblerConfig, TxPoolConfig};
    use ckb_jsonrpc_types::ScriptHashType;
    use ckb_types::{h256, packed::CellInput};
    use std::sync::atomic::AtomicU64;
    use tokio::sync::{Mutex, RwLock, oneshot};

    let snapshot = genesis_snapshot();
    let config = BlockAssemblerConfig {
        code_hash: h256!("0x0"),
        args: Default::default(),
        hash_type: ScriptHashType::Data,
        message: Default::default(),
        use_binary_version_as_message_prefix: true,
        binary_version: "TEST".to_string(),
        update_interval_millis: 800,
        notify: vec![],
        notify_scripts: vec![],
        notify_timeout_millis: 800,
        notify_auth_token: None,
    };
    // Construct only the state needed to reach `template_lock`. The spawned
    // updates are cancelled while blocked, before they can consume this dummy
    // template; this keeps the test independent from chain-root MMR setup.
    let epoch = snapshot.consensus().genesis_epoch_ext().clone();
    let cellbase = ckb_types::core::TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(1))
        .build();
    let mut template_builder = super::BlockTemplateBuilder::new(&snapshot, &epoch).unwrap();
    template_builder
        .cellbase(cellbase)
        .work_id(0)
        .dao(Byte32::zero())
        .current_time(1);
    let current = super::CurrentTemplate {
        template: template_builder.build(),
        size: super::TemplateSize {
            txs: 0,
            proposals: 0,
            uncles: 0,
            total: 0,
        },
        snapshot: Arc::clone(&snapshot),
        epoch,
    };
    let assembler = super::BlockAssembler {
        config: Arc::new(config),
        work_id: Arc::new(AtomicU64::new(1)),
        candidate_uncles: Arc::new(Mutex::new(CandidateUncles::new())),
        current: Arc::new(RwLock::new(Arc::new(current))),
        version: Arc::new(AtomicU64::new(0)),
        template_lock: Arc::new(Mutex::new(())),
        cell_liveness_memo: Arc::new(std::sync::Mutex::new(CellLivenessMemo::default())),
        poster: Arc::new(
            hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                .build::<_, http_body_util::Full<ckb_types::bytes::Bytes>>(
                hyper_util::client::legacy::connect::HttpConnector::new(),
            ),
        ),
    };
    let pool = Arc::new(RwLock::new(TxPool::new(
        TxPoolConfig::default(),
        Arc::clone(&snapshot),
    )));

    let guard = assembler.template_lock.lock().await;

    let (full_started_tx, full_started_rx) = oneshot::channel();
    let full_assembler = assembler.clone();
    let full_pool = Arc::clone(&pool);
    let full = tokio::spawn(async move {
        let _ = full_started_tx.send(());
        full_assembler.update_full(&full_pool).await
    });

    let (uncle_started_tx, uncle_started_rx) = oneshot::channel();
    let uncle_assembler = assembler.clone();
    let uncle = tokio::spawn(async move {
        let _ = uncle_started_tx.send(());
        uncle_assembler.update_uncles().await;
    });

    full_started_rx.await.expect("full update task started");
    uncle_started_rx.await.expect("uncle update task started");
    tokio::task::yield_now().await;
    assert!(
        !full.is_finished(),
        "full update must wait for template_lock"
    );
    assert!(
        !uncle.is_finished(),
        "uncle update must wait for the same template_lock"
    );

    full.abort();
    uncle.abort();
    drop(guard);
    assert!(
        full.await
            .expect_err("full update was cancelled")
            .is_cancelled()
    );
    assert!(
        uncle
            .await
            .expect_err("uncle update was cancelled")
            .is_cancelled()
    );
}
