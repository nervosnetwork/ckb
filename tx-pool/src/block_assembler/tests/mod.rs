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

/// Uncle proposals are excluded by the consensus block-size accounting
/// basis. The canonical per-uncle increment is therefore independent of the
/// number of proposal ids carried by that uncle.
#[test]
fn uncle_size_matches_the_canonical_block_size_basis() {
    use ckb_types::packed::ProposalShortId;

    let snapshot = genesis_snapshot();
    let cellbase = snapshot.consensus().genesis_block().transactions()[0].data();
    let bare = BlockBuilder::default()
        .number(1)
        .epoch(EpochNumberWithFraction::new(0, 0, 1))
        .build()
        .as_uncle();
    let proposals: Vec<ProposalShortId> = (0..32u8)
        .map(|i| ProposalShortId::from_tx_hash(&Byte32::new([i; 32])))
        .collect();
    let with_proposals = BlockBuilder::default()
        .number(2)
        .epoch(EpochNumberWithFraction::new(0, 0, 1))
        .proposals(proposals.clone())
        .build()
        .as_uncle();
    let base = super::BlockAssembler::basic_block_size(
        cellbase.clone(),
        &[],
        std::iter::empty::<&ProposalShortId>(),
        None,
    );
    let expected = ckb_types::core::UncleBlockView::serialized_size_in_block();

    for uncle in [bare, with_proposals] {
        assert_eq!(super::BlockAssembler::uncle_size(&uncle), expected);
        let with_uncle = super::BlockAssembler::basic_block_size(
            cellbase.clone(),
            &[uncle],
            std::iter::empty::<&ProposalShortId>(),
            None,
        );
        assert_eq!(with_uncle.checked_sub(base), Some(expected));
    }
}

/// Bug #56: a committed uncle plan must remove candidates that are already on
/// the main chain or embedded as an uncle. Read-only preparation itself must
/// not mutate the live cache because its publication token may lose a race.
#[test]
fn candidate_uncle_receipt_is_exact_and_committed_stale_prune_is_version_neutral() {
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

    let prepared = candidate_uncles.prepare_uncles(&snapshot, &epoch_ext);
    let (uncles, stale, captured_source) = prepared.into_parts();
    let current_source = candidate_uncles
        .prepare_uncles(&snapshot, &epoch_ext)
        .into_parts()
        .2;
    assert_eq!(captured_source, current_source);

    assert!(
        candidate_uncles.contains(&genesis_uncle),
        "read-only preparation cannot prune before publication"
    );
    candidate_uncles.prune(stale);
    assert_eq!(
        candidate_uncles
            .prepare_uncles(&snapshot, &epoch_ext)
            .into_parts()
            .2,
        captured_source,
        "pruning candidates proven absent from this chain cut cannot dirty an equivalent template source"
    );

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
    // for removal; it is a valid uncle that was selected).
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
fn optional_content_uses_one_budget_and_filters_only_published_conflicts() {
    use ckb_types::packed::ProposalShortId;

    let snapshot = genesis_snapshot();
    let genesis = snapshot.consensus().genesis_block();
    let epoch = snapshot
        .consensus()
        .genesis_epoch_ext()
        .number_with_fraction(1);
    let proposal = ProposalShortId::from_tx_hash(&Byte32::new([3; 32]));
    let conflicting = BlockBuilder::default()
        .number(1)
        .epoch(epoch)
        .parent_hash(genesis.hash())
        .proposals(vec![proposal.clone()])
        .build()
        .as_uncle();
    let independent = BlockBuilder::default()
        .number(1)
        .epoch(epoch)
        .timestamp(1)
        .parent_hash(genesis.hash())
        .build()
        .as_uncle();
    let base = 1_000;
    let expected_uncle_size = super::BlockAssembler::uncle_size(&independent);
    let max = base + ProposalShortId::serialized_size() + expected_uncle_size;

    let fitted = super::BlockAssembler::fit_optional_content(
        &snapshot,
        vec![proposal.clone()],
        &[conflicting, independent.clone()],
        base,
        max,
    )
    .unwrap()
    .expect("mandatory template content fits");

    assert_eq!(fitted.proposals, vec![proposal]);
    assert_eq!(fitted.uncles, vec![independent]);
    assert_eq!(fitted.proposals_size, ProposalShortId::serialized_size());
    assert_eq!(fitted.uncles_size, expected_uncle_size);
    assert_eq!(fitted.total_size, max);
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
