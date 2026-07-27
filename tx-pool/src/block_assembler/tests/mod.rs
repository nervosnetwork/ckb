use ckb_types::core::{BlockBuilder, EpochNumberWithFraction};

use crate::block_assembler::candidate_uncles::CandidateUncles;

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
        super::BlockAssembler::uncle_size(&bare).unwrap(),
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
        super::BlockAssembler::uncle_size(&with_proposals).unwrap(),
        expected,
        "uncle_size must subtract proposal ids from the in-block size"
    );
    assert!(
        super::BlockAssembler::uncle_size(&with_proposals).unwrap()
            < super::BlockAssembler::uncle_size(&bare).unwrap(),
        "an uncle with proposals must account for fewer bytes than a bare one"
    );
}

/// Bug #56: a committed uncle plan must remove candidates that are already on
/// the main chain or embedded as an uncle. Read-only preparation itself must
/// not mutate the live cache because its publication token may lose a race.
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

    let (uncles, stale) = candidate_uncles
        .prepare_uncles(&snapshot, &epoch_ext)
        .into_parts();

    assert!(
        candidate_uncles.contains(&genesis_uncle),
        "read-only preparation cannot prune before publication"
    );
    candidate_uncles.prune(stale);

    // The genesis uncle is on the main chain: must be removed.
    assert!(
        !candidate_uncles.contains(&genesis_uncle),
        "main-chain candidate must be removed by committed uncle cleanup"
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

#[test]
fn uncle_update_keeps_ordered_fitting_prefix_with_checked_accounting() {
    let uncles: Vec<_> = (0..3)
        .map(|timestamp| {
            BlockBuilder::default()
                .timestamp(timestamp)
                .build()
                .as_uncle()
        })
        .collect();
    let one_uncle = super::BlockAssembler::uncle_size(&uncles[0]).unwrap();
    let base = 1_000;
    let size = super::TemplateSize {
        txs: 0,
        proposals: 0,
        uncles: 0,
        total: base,
    };

    let mut exact = uncles.clone();
    let (uncle_bytes, total) =
        super::BlockAssembler::fit_uncle_prefix(&mut exact, size, base + 2 * one_uncle)
            .expect("base template fits");
    assert_eq!(exact, uncles[..2]);
    assert_eq!(uncle_bytes, 2 * one_uncle);
    assert_eq!(total, base + 2 * one_uncle);

    let mut partial = uncles.clone();
    let (uncle_bytes, total) =
        super::BlockAssembler::fit_uncle_prefix(&mut partial, size, base + one_uncle)
            .expect("base template fits");
    assert_eq!(partial, uncles[..1]);
    assert_eq!(uncle_bytes, one_uncle);
    assert_eq!(total, base + one_uncle);

    let mut none = uncles.clone();
    let (uncle_bytes, total) =
        super::BlockAssembler::fit_uncle_prefix(&mut none, size, base + one_uncle - 1)
            .expect("base template still fits");
    assert!(none.is_empty());
    assert_eq!(uncle_bytes, 0);
    assert_eq!(total, base);

    let corrupt = super::TemplateSize {
        uncles: base + 1,
        ..size
    };
    assert!(
        super::BlockAssembler::fit_uncle_prefix(&mut uncles.clone(), corrupt, usize::MAX).is_none(),
        "an internally inconsistent size ledger must not saturate into a valid update"
    );
}

/// Full publication wins over an intervening partial revision, while stale
/// partial work is rejected. Both guarantees come from tokens co-located with
/// the current template; no full/reset lock serializes construction.
#[tokio::test]
async fn full_reset_and_partial_priority_use_template_owned_tokens() {
    use ckb_app_config::BlockAssemblerConfig;
    use ckb_jsonrpc_types::ScriptHashType;
    use ckb_types::{h256, packed::CellInput};
    use ckb_util::Mutex;
    use std::sync::atomic::AtomicU64;
    use tokio::sync::RwLock;

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
    let epoch = snapshot.consensus().genesis_epoch_ext().clone();
    let cellbase = ckb_types::core::TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(1))
        .build();
    let template_draft = super::BlockTemplateDraft::new(&snapshot, &epoch).unwrap();
    let current = super::CurrentTemplate {
        template: template_draft.build(cellbase, 0, Byte32::zero(), 1),
        size: super::TemplateSize {
            txs: 0,
            proposals: 0,
            uncles: 0,
            total: 0,
        },
        snapshot,
        epoch,
        revision: super::TemplateRevision::INITIAL,
        reset_epoch: super::ResetEpoch::INITIAL,
    };
    let assembler = super::BlockAssembler {
        config: Arc::new(config),
        work_id: Arc::new(AtomicU64::new(1)),
        candidate_uncles: Arc::new(Mutex::new(CandidateUncles::new())),
        current: Arc::new(RwLock::new(Arc::new(current))),
        cell_liveness_memo: Arc::new(ckb_util::Mutex::new(CellLivenessMemo::default())),
        poster: Arc::new(
            hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                .build::<_, http_body_util::Full<ckb_types::bytes::Bytes>>(
                hyper_util::client::legacy::connect::HttpConnector::new(),
            ),
        ),
        notify_count: Arc::new(AtomicU64::new(0)),
    };
    let original = assembler.current.read().await.clone();
    let stale_candidate = BlockBuilder::default().build().as_uncle();
    assert!(
        assembler
            .candidate_uncles
            .lock()
            .insert(stale_candidate.clone())
    );

    let at_time = |current: &super::CurrentTemplate, current_time| {
        let template = &current.template;
        let mut builder = super::BlockTemplateBuilder::for_update(
            template,
            super::builder::TemplateContentUpdate::Full {
                uncles: template.uncles.clone(),
                transactions: template.transactions.clone(),
                proposals: template.proposals.clone(),
                dao: template.dao.clone(),
            },
        );
        builder.current_time(current_time);
        current.with_content(builder.build(), current.size)
    };

    let partial = at_time(&original, 11);
    assert!(
        assembler
            .try_publish_partial(partial, original.revision, Vec::new())
            .await
            .unwrap()
    );

    let full = at_time(&original, 22);
    assert!(
        assembler
            .try_publish_full(full, original.reset_epoch, Vec::new())
            .await
            .unwrap(),
        "full publication ignores a partial-only revision race"
    );
    assert_eq!(assembler.current.read().await.template.current_time, 22);

    let stale_partial = at_time(&original, 33);
    assert!(
        !assembler
            .try_publish_partial(stale_partial, original.revision, Vec::new())
            .await
            .unwrap(),
        "partial publication cannot overwrite newer full content"
    );
    assert_eq!(assembler.current.read().await.template.current_time, 22);

    // Model the linearization point of a reset publication: both tokens move
    // with the replacement template. A full build captured before this point
    // must not cross it; a rebuild captured afterwards remains authoritative.
    let reset = {
        let mut guard = assembler.current.write().await;
        let mut reset = at_time(guard.as_ref(), 44);
        reset.revision = guard.revision.next().unwrap();
        reset.reset_epoch = guard.reset_epoch.next().unwrap();
        *guard = Arc::new(reset);
        guard.clone()
    };

    let pre_reset_full = at_time(&original, 55);
    assert!(
        !assembler
            .try_publish_full(
                pre_reset_full,
                original.reset_epoch,
                vec![stale_candidate.clone()],
            )
            .await
            .unwrap(),
        "a full build captured before reset cannot cross its epoch"
    );
    assert_eq!(assembler.current.read().await.template.current_time, 44);
    assert!(
        assembler.candidate_uncles.lock().contains(&stale_candidate),
        "a rejected full plan cannot apply its candidate-cache cleanup"
    );

    let post_reset_full = at_time(&reset, 66);
    assert!(
        assembler
            .try_publish_full(
                post_reset_full,
                reset.reset_epoch,
                vec![stale_candidate.clone()],
            )
            .await
            .unwrap(),
        "a full build captured after reset publishes normally"
    );
    assert_eq!(assembler.current.read().await.template.current_time, 66);
    assert!(
        !assembler.candidate_uncles.lock().contains(&stale_candidate),
        "a committed full plan applies its candidate-cache cleanup"
    );
}
