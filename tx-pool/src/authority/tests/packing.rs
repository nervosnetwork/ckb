//! The existing package-selection regressions, exercised through the new Store.
use super::TemplatePackingLimits;
use crate::authority::{
    model::{Entry, Status},
    packing::Selection,
    store::Store,
    tests::common,
};
use ckb_snapshot::Snapshot;
use ckb_types::{
    bytes::Bytes,
    core::{TransactionBuilder, TransactionView},
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint},
    prelude::{Builder, Entity, Pack},
};
use std::{collections::BTreeSet, sync::Arc};
fn packed_hashes(packed: &[crate::TxEntry]) -> Vec<Byte32> {
    packed
        .iter()
        .map(|entry| entry.transaction().hash())
        .collect()
}
fn bytes(packed: &[crate::TxEntry]) -> usize {
    packed.iter().map(|entry| entry.size).sum()
}
fn cycles(packed: &[crate::TxEntry]) -> u64 {
    packed.iter().map(|entry| entry.cycles).sum()
}
fn output_transaction(version: u32) -> TransactionView {
    TransactionBuilder::default()
        .version(version)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build()
}

fn child_transaction(version: u32, parent: &TransactionView) -> TransactionView {
    child_transaction_at(version, parent, 0)
}

fn child_transaction_at(
    version: u32,
    parent: &TransactionView,
    output_index: u32,
) -> TransactionView {
    TransactionBuilder::default()
        .version(version)
        .input(CellInput::new(
            OutPoint::new(parent.hash(), output_index),
            0,
        ))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build()
}

fn accept(
    store: &Store,
    transaction: TransactionView,
    _peer: usize,
    status: Status,
    fee: u64,
    cycles: u64,
) -> Byte32 {
    common::accept(store, transaction, fee, cycles, status)
}
fn capture(store: &Store) -> (Arc<Snapshot>, Vec<Arc<Entry>>) {
    let (_, snapshot, owners, _) = store.capture(true);
    (snapshot, owners)
}
fn selection<'a>(owners: &'a [Arc<Entry>], snapshot: &Snapshot) -> Selection<'a> {
    Selection::new(owners, snapshot, common::config().max_ancestors_count).unwrap()
}

#[test]
fn uak_template_packer_selects_an_exact_fit_cpfp_package_parent_first() {
    let authority = common::store();
    let parent_tx = output_transaction(2_001);
    let parent = accept(&authority, parent_tx.clone(), 1, Status::Proposed, 1, 10);
    let child_tx = child_transaction(2_002, &parent_tx);
    let child = accept(
        &authority,
        child_tx.clone(),
        2,
        Status::Proposed,
        1_000_000_000,
        20,
    );
    let rival_tx = TransactionBuilder::default().version(2_003u32).build();
    let rival_bytes = rival_tx.data().serialized_size_in_block();
    let rival = accept(&authority, rival_tx, 3, Status::Proposed, 1_000, 30);
    let package_bytes = parent_tx
        .data()
        .serialized_size_in_block()
        .checked_add(child_tx.data().serialized_size_in_block())
        .expect("fixture bytes fit");
    let (snapshot, owners) = capture(&authority);
    let receipt = selection(&owners, &snapshot);

    let exact = receipt
        .pack_transactions(TemplatePackingLimits::new(package_bytes, 30))
        .expect("the exact parent-child package fits");
    let exact_hashes = packed_hashes(&exact);
    assert_eq!(exact_hashes, vec![parent.clone(), child.clone()]);
    assert_eq!(bytes(&exact), package_bytes);
    assert_eq!(cycles(&exact), 30);
    assert_eq!(exact[0].timestamp, 0);
    assert_eq!(exact[1].cycles, 20);
    assert_eq!(exact[1].proposal_short_id(), child_tx.proposal_short_id());
    assert_eq!(exact[1].transaction().hash(), child);
    assert!(
        !exact
            .iter()
            .any(|entry| entry.transaction().hash() == rival)
    );

    let one_byte_limit = package_bytes - 1;
    let one_byte_short = receipt
        .pack_transactions(TemplatePackingLimits::new(one_byte_limit, u64::MAX))
        .expect("a one-byte-short limit is an ordinary packing result");
    let one_byte_hashes = packed_hashes(&one_byte_short);
    assert_eq!(
        one_byte_hashes,
        vec![rival.clone(), parent.clone()],
        "one byte below the CPFP package must retain the independently fitting rival and parent"
    );
    assert_eq!(
        bytes(&one_byte_short),
        rival_bytes + parent_tx.data().serialized_size_in_block()
    );
    assert_eq!(cycles(&one_byte_short), 40);
    assert!(bytes(&one_byte_short) <= one_byte_limit);
    assert!(
        !one_byte_short
            .iter()
            .any(|entry| entry.transaction().hash() == child)
    );

    let one_cycle_short = receipt
        .pack_transactions(TemplatePackingLimits::new(usize::MAX, 29))
        .expect("a one-cycle-short limit is an ordinary packing result");
    let one_cycle_hashes = packed_hashes(&one_cycle_short);
    assert_eq!(one_cycle_hashes, vec![parent]);
    assert_eq!(
        bytes(&one_cycle_short),
        parent_tx.data().serialized_size_in_block()
    );
    assert_eq!(cycles(&one_cycle_short), 10);
    assert!(cycles(&one_cycle_short) <= 29);
    assert!(
        !one_cycle_short
            .iter()
            .any(|entry| entry.transaction().hash() == child)
    );
    assert!(
        !one_cycle_short
            .iter()
            .any(|entry| entry.transaction().hash() == rival)
    );
}

#[test]
fn uak_template_packer_rescores_descendants_after_shared_parent_selection() {
    let authority = common::store();
    let parent_tx = TransactionBuilder::default()
        .version(2_010u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let parent = accept(&authority, parent_tx.clone(), 10, Status::Proposed, 1, 1);
    let strongest_tx = child_transaction(2_011, &parent_tx);
    let strongest = accept(&authority, strongest_tx, 11, Status::Proposed, 1_000_000, 1);
    let rescored_tx = child_transaction_at(2_012, &parent_tx, 1);
    let rescored = accept(&authority, rescored_tx, 12, Status::Proposed, 100_000, 1);
    let rival_tx = output_transaction(2_013);
    let rival = accept(&authority, rival_tx, 13, Status::Proposed, 60_000, 1);

    let (snapshot, owners) = capture(&authority);
    let receipt = selection(&owners, &snapshot);
    let initial: Vec<_> = receipt
        .candidates_by_score()
        .map(|candidate| candidate.hash())
        .collect();
    assert!(
        initial.iter().position(|hash| **hash == rival).unwrap()
            < initial.iter().position(|hash| **hash == rescored).unwrap(),
        "the rival must start ahead of the child package"
    );
    let packed = receipt
        .pack_transactions(TemplatePackingLimits::new(usize::MAX, 3))
        .expect("dynamic CPFP scoring remains coherent");
    let hashes = packed_hashes(&packed);
    assert_eq!(hashes, vec![parent, strongest, rescored]);
    assert!(!hashes.contains(&rival));
}

#[test]
fn uak_template_packer_aggregates_multi_parent_descendant_adjustments() {
    let authority = common::store();
    let first_parent_tx = output_transaction(2_014);
    let first_parent = accept(
        &authority,
        first_parent_tx.clone(),
        14,
        Status::Proposed,
        1_000_000,
        1,
    );
    let second_parent_tx = output_transaction(2_015);
    let second_parent = accept(
        &authority,
        second_parent_tx.clone(),
        15,
        Status::Proposed,
        500_000,
        1,
    );
    let child_tx = TransactionBuilder::default()
        .version(2_016u32)
        .input(CellInput::new(OutPoint::new(first_parent_tx.hash(), 0), 0))
        .input(CellInput::new(OutPoint::new(second_parent_tx.hash(), 0), 0))
        .build();
    let child = accept(&authority, child_tx, 16, Status::Proposed, 1, 1);

    let (snapshot, owners) = capture(&authority);
    let receipt = selection(&owners, &snapshot);
    let packed = receipt
        .pack_transactions(TemplatePackingLimits::new(usize::MAX, 3))
        .expect("both selected-parent deltas are aggregated exactly once");
    let hashes = packed_hashes(&packed);
    assert_eq!(hashes, vec![first_parent, second_parent, child]);
}

#[test]
fn uak_template_packer_bounds_non_fitting_work_without_changing_the_policy() {
    let authority = common::store();
    let first = accept(
        &authority,
        TransactionBuilder::default().version(2_020u32).build(),
        20,
        Status::Proposed,
        1_000_000_000,
        1,
    );
    for (version, fee) in [(2_021u32, 100_000_000u64), (2_022u32, 10_000_000u64)] {
        accept(
            &authority,
            TransactionBuilder::default().version(version).build(),
            usize::try_from(version).expect("fixture peer fits"),
            Status::Proposed,
            fee,
            2,
        );
    }
    let small = accept(
        &authority,
        TransactionBuilder::default().version(2_023u32).build(),
        23,
        Status::Proposed,
        1,
        1,
    );
    let (snapshot, owners) = capture(&authority);
    let receipt = selection(&owners, &snapshot);

    let bounded = receipt
        .pack_transactions_with_failure_bound(TemplatePackingLimits::new(usize::MAX, 2), 1)
        .expect("the failure bound is a deterministic early stop");
    let bounded_hashes = packed_hashes(&bounded);
    assert_eq!(bounded_hashes, vec![first.clone()]);

    let complete = receipt
        .pack_transactions(TemplatePackingLimits::new(usize::MAX, 2))
        .expect("the production bound reaches the later fitting candidate");
    let complete_hashes = packed_hashes(&complete);
    assert_eq!(complete_hashes, vec![first, small]);
}

#[test]
fn uak_template_packer_requires_proposed_ancestors_and_orders_conditional_edges() {
    let authority = common::store();
    let parent_tx = output_transaction(2_030);
    accept(&authority, parent_tx.clone(), 30, Status::Pending, 1, 1);
    let child_tx = child_transaction(2_031, &parent_tx);
    let child = accept(&authority, child_tx, 31, Status::Proposed, 1_000_000, 1);

    let shared = OutPoint::new(Byte32::new([201; 32]), 0);
    let reader_input = OutPoint::new(Byte32::new([202; 32]), 0);
    let reader_tx = TransactionBuilder::default()
        .version(2_032u32)
        .input(CellInput::new(reader_input, 0))
        .cell_dep(CellDep::new_builder().out_point(shared.clone()).build())
        .build();
    let reader = accept(&authority, reader_tx, 32, Status::Proposed, 1, 1);
    let spender_tx = TransactionBuilder::default()
        .version(2_033u32)
        .input(CellInput::new(shared, 0))
        .build();
    let spender = accept(&authority, spender_tx, 33, Status::Proposed, 1_000_000, 1);

    let (snapshot, owners) = capture(&authority);
    let packed = selection(&owners, &snapshot)
        .pack_transactions(TemplatePackingLimits::new(usize::MAX, u64::MAX))
        .expect("the selected-set conditional graph is coherent");
    let hashes = packed
        .iter()
        .map(|entry| entry.transaction().hash())
        .collect::<Vec<_>>();
    assert!(!hashes.contains(&child));
    let reader_position = hashes
        .iter()
        .position(|hash| *hash == reader)
        .expect("reader is selected");
    let spender_position = hashes
        .iter()
        .position(|hash| *hash == spender)
        .expect("spender is selected");
    assert!(reader_position < spender_position);
}

#[test]
fn uak_template_packer_bounds_long_conditional_scc_fallback() {
    const CYCLE_MEMBERS: usize = 66;
    let points = (0..CYCLE_MEMBERS)
        .map(|seed| {
            let mut hash = [0u8; 32];
            hash[..size_of::<usize>()].copy_from_slice(&seed.to_le_bytes());
            OutPoint::new(Byte32::new(hash), 0)
        })
        .collect::<Vec<_>>();
    let authority = common::store();
    let mut preferred = None;
    for (index, input) in points.iter().enumerate() {
        let mut builder = TransactionBuilder::default()
            .version(u32::try_from(2_100 + index).expect("fixture version fits"))
            .input(CellInput::new(input.clone(), 0));
        // Bidirectional adjacent edges form one SCC without causal parents.
        // Each round removes its least preferred owner. The last two force
        // the bounded fallback; the highest-fee singleton remains valid.
        for dependency_index in [
            (index + points.len() - 1) % points.len(),
            (index + 1) % points.len(),
        ] {
            builder = builder.cell_dep(
                CellDep::new_builder()
                    .out_point(points[dependency_index].clone())
                    .build(),
            );
        }
        let transaction = builder.build();
        preferred = Some(accept(
            &authority,
            transaction,
            index,
            Status::Proposed,
            ((index + 1) * 100) as u64,
            1,
        ));
    }

    let (snapshot, owners) = capture(&authority);
    let packed = selection(&owners, &snapshot)
        .pack_transactions(TemplatePackingLimits::new(usize::MAX, u64::MAX))
        .expect("the long conditional SCC uses the bounded deterministic fallback");
    assert_eq!(packed.len(), 1);
    assert_eq!(
        packed[0].transaction().hash(),
        preferred.expect("the highest-fee independent owner is retained")
    );
}

fn causal_dag_transactions(edge_mask: u8) -> Vec<TransactionView> {
    let edges = [(0usize, 1usize), (0, 2), (1, 2)];
    let mut transactions = Vec::<TransactionView>::new();
    for child in 0..3 {
        let mut builder =
            TransactionBuilder::default().version(1_900 + u32::from(edge_mask) * 3 + child as u32);
        for (edge_index, (parent, edge_child)) in edges.into_iter().enumerate() {
            if edge_child == child && edge_mask & (1 << edge_index) != 0 {
                builder = builder.input(CellInput::new(
                    OutPoint::new(transactions[parent].hash(), child as u32),
                    0,
                ));
            }
        }
        for _ in 0..3 {
            builder = builder
                .output(CellOutput::default())
                .output_data(Bytes::new().pack());
        }
        transactions.push(builder.build());
    }
    transactions
}

fn expected_causal_membership(statuses: &[u8], parents: &[BTreeSet<usize>]) -> BTreeSet<usize> {
    fn eligible(
        index: usize,
        statuses: &[u8],
        parents: &[BTreeSet<usize>],
        memo: &mut [u8],
    ) -> bool {
        match memo[index] {
            1 | 2 => return false,
            3 => return true,
            _ => {}
        }
        if statuses[index] != 2 {
            memo[index] = 2;
            return false;
        }
        memo[index] = 1;
        let result = parents[index]
            .iter()
            .all(|parent| eligible(*parent, statuses, parents, memo));
        memo[index] = if result { 3 } else { 2 };
        result
    }

    let mut memo = vec![0; statuses.len()];
    (0..statuses.len())
        .filter(|index| eligible(*index, statuses, parents, &mut memo))
        .collect()
}

#[test]
fn template_causal_eligibility_matches_every_three_vertex_dag_and_proposal_phase() {
    let snapshot = crate::test_support::genesis_snapshot();
    for edge_mask in 0u8..8 {
        let transactions = causal_dag_transactions(edge_mask);
        for encoding in 0u8..27 {
            let store = Store::new(std::sync::Arc::clone(&snapshot), &common::config()).unwrap();
            let mut digits = encoding;
            for transaction in &transactions {
                let status =
                    [Status::Pending, Status::Gap, Status::Proposed][usize::from(digits % 3)];
                digits /= 3;
                common::accept(&store, transaction.clone(), 1, 1, status);
            }
            let (snapshot, owners) = capture(&store);
            let selection = selection(&owners, &snapshot);
            let by_hash = selection.candidate_index().unwrap();
            let statuses: Vec<_> = selection
                .candidates()
                .iter()
                .map(
                    |candidate| match candidate.accepted.status(&store.snapshot().1) {
                        Status::Pending => 0,
                        Status::Gap => 1,
                        Status::Proposed => 2,
                    },
                )
                .collect();
            let parents: Vec<BTreeSet<_>> = selection
                .candidates()
                .iter()
                .map(|candidate| {
                    candidate
                        .accepted
                        .parents
                        .iter()
                        .map(|parent| by_hash[parent])
                        .collect()
                })
                .collect();
            let expected = expected_causal_membership(&statuses, &parents);
            let actual = selection
                .package_eligible_proposed()
                .unwrap()
                .into_iter()
                .collect::<BTreeSet<_>>();
            assert_eq!(actual, expected, "edges={edge_mask} phases={encoding}");
        }
    }
}
