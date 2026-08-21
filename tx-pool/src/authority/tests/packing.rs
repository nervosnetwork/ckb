//! Regression tests for pure authority-receipt block-template packing.

use super::super::{
    packing::TemplatePackingLimits,
    plan::TxPoolAuthority,
    resources::{AcceptedResources, ComputeLimits, ResourceLimits, ResourceVector},
    state::{AcceptedStatus, RawTxHash},
};
use super::foundation::{
    accept_remote_transaction_with_payload, accept_remote_transaction_with_payload_and_cycles,
    limits, resolved_payload_with_facts,
};
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, TransactionBuilder, TransactionView},
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint},
    prelude::{Builder, Entity, Pack},
};
use std::collections::{BTreeSet, HashMap};

fn model_packing_matches_production(
    receipt: &super::super::template::TemplateSelectionReceipt,
    serialized_bytes: usize,
    cycles: u64,
    max_consecutive_failures: usize,
    production_hashes: &[RawTxHash],
) -> bool {
    use crate::mathematical_model::{
        EvictionRefinementMetrics,
        two_phase::{
            ModelTemplatePackingLimits, TemplatePackingInput, template_packing_refinement,
        },
    };

    let by_hash = receipt
        .candidates()
        .iter()
        .enumerate()
        .map(|(index, candidate)| (candidate.hash().clone(), index))
        .collect::<HashMap<_, _>>();
    let inputs = receipt
        .candidates()
        .iter()
        .map(|candidate| {
            let metrics = candidate.metrics();
            let parents = candidate
                .parents()
                .iter()
                .map(|parent| {
                    by_hash
                        .get(parent)
                        .copied()
                        .expect("the production receipt closes every causal parent")
                })
                .collect::<BTreeSet<_>>();
            let mut identity = [0; 32];
            identity.copy_from_slice(candidate.hash().0.as_slice());
            TemplatePackingInput::new(
                EvictionRefinementMetrics::new(
                    metrics.fee.as_u64(),
                    u64::try_from(metrics.cost.serialized_bytes)
                        .expect("the production serialized size fits the model coordinate"),
                    metrics.cost.cycles,
                ),
                parents,
                candidate.status() == AcceptedStatus::Proposed,
                candidate.order().arrival().0,
                identity,
            )
        })
        .collect::<Vec<_>>();
    let model_hashes = template_packing_refinement(
        &inputs,
        ModelTemplatePackingLimits::new(
            u64::try_from(serialized_bytes).expect("the template limit fits the model coordinate"),
            cycles,
        ),
        max_consecutive_failures,
    )
    .expect("the immutable production receipt has one finite model packing observation")
    .selected
    .into_iter()
    .map(|index| receipt.candidates()[index].hash().clone())
    .collect::<Vec<_>>();
    model_hashes.as_slice() == production_hashes
}

fn packed_hashes(packed: &super::super::packing::PackedTemplateTransactions) -> Vec<RawTxHash> {
    packed.entries().iter().map(|entry| entry.hash()).collect()
}

fn assert_model_rejects_every_distinct_reordering(
    receipt: &super::super::template::TemplateSelectionReceipt,
    serialized_bytes: usize,
    cycles: u64,
    max_consecutive_failures: usize,
    production_hashes: &[RawTxHash],
) {
    fn visit(
        receipt: &super::super::template::TemplateSelectionReceipt,
        serialized_bytes: usize,
        cycles: u64,
        max_consecutive_failures: usize,
        production_hashes: &[RawTxHash],
        permutation: &mut [RawTxHash],
        index: usize,
    ) {
        if index == permutation.len() {
            if permutation != production_hashes {
                assert!(
                    !model_packing_matches_production(
                        receipt,
                        serialized_bytes,
                        cycles,
                        max_consecutive_failures,
                        permutation,
                    ),
                    "the model predicate must reject every distinct reordering of the production observation"
                );
            }
            return;
        }
        for swap_index in index..permutation.len() {
            permutation.swap(index, swap_index);
            visit(
                receipt,
                serialized_bytes,
                cycles,
                max_consecutive_failures,
                production_hashes,
                permutation,
                index + 1,
            );
            permutation.swap(index, swap_index);
        }
    }

    let mut permutation = production_hashes.to_vec();
    visit(
        receipt,
        serialized_bytes,
        cycles,
        max_consecutive_failures,
        production_hashes,
        &mut permutation,
        0,
    );
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
    authority: &mut TxPoolAuthority,
    transaction: TransactionView,
    peer: usize,
    status: AcceptedStatus,
    fee: u64,
    cycles: u64,
) -> RawTxHash {
    accept_remote_transaction_with_payload_and_cycles(
        authority,
        transaction.clone(),
        peer,
        status,
        resolved_payload_with_facts(
            &transaction,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(fee),
        ),
        cycles,
    )
}

fn selection(authority: &TxPoolAuthority) -> super::super::template::TemplateSelectionReceipt {
    authority
        .read_view()
        .capture_template()
        .expect("fixture template projection is coherent")
        .into_selection()
        .expect("fixture receipt becomes an owned selection")
}

fn large_template_limits() -> ResourceLimits {
    const MIB: usize = 1024 * 1024;
    ResourceLimits::new(
        ResourceVector::new(128, 64 * MIB, 10_000, 8),
        ResourceVector::new(128, 64 * MIB, 10_000, 8),
        ResourceVector::new(1, MIB, 128, 1),
        AcceptedResources::new(128, 64 * MIB, 64 * MIB, u64::MAX),
        ComputeLimits::new(MIB, MIB, 128),
    )
    .expect("large template fixture limits are monotonic")
}

#[test]
fn uak_template_packer_selects_an_exact_fit_cpfp_package_parent_first() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_tx = output_transaction(2_001);
    let parent = accept(
        &mut authority,
        parent_tx.clone(),
        1,
        AcceptedStatus::Proposed,
        1,
        10,
    );
    let child_tx = child_transaction(2_002, &parent_tx);
    let child = accept(
        &mut authority,
        child_tx.clone(),
        2,
        AcceptedStatus::Proposed,
        1_000_000_000,
        20,
    );
    let rival_tx = TransactionBuilder::default().version(2_003u32).build();
    let rival_bytes = rival_tx.data().serialized_size_in_block();
    let rival = accept(
        &mut authority,
        rival_tx,
        3,
        AcceptedStatus::Proposed,
        1_000,
        30,
    );
    let package_bytes = parent_tx
        .data()
        .serialized_size_in_block()
        .checked_add(child_tx.data().serialized_size_in_block())
        .expect("fixture bytes fit");
    let receipt = selection(&authority);

    let exact = receipt
        .pack_transactions(TemplatePackingLimits::new(package_bytes, 30))
        .expect("the exact parent-child package fits");
    let exact_hashes = packed_hashes(&exact);
    assert_eq!(exact_hashes, vec![parent.clone(), child.clone()]);
    assert_eq!(exact.serialized_bytes(), package_bytes);
    assert_eq!(exact.cycles(), 30);
    assert_eq!(exact.entries()[0].accepted_at().0, 0);
    assert_eq!(exact.entries()[1].metrics().cost.cycles, 20);
    assert_eq!(
        exact.entries()[1].proposal_short_id(),
        child_tx.proposal_short_id()
    );
    assert_eq!(exact.entries()[1].resolved().transaction.hash(), child.0);
    assert!(!exact.entries().iter().any(|entry| entry.hash() == rival));
    assert!(
        model_packing_matches_production(
            &receipt,
            package_bytes,
            30,
            crate::mathematical_model::two_phase::TEMPLATE_PACKING_FAILURE_BOUND,
            &exact_hashes,
        ),
        "the production packer must refine the independent package model"
    );
    assert_model_rejects_every_distinct_reordering(
        &receipt,
        package_bytes,
        30,
        crate::mathematical_model::two_phase::TEMPLATE_PACKING_FAILURE_BOUND,
        &exact_hashes,
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
        one_byte_short.serialized_bytes(),
        rival_bytes + parent_tx.data().serialized_size_in_block()
    );
    assert_eq!(one_byte_short.cycles(), 40);
    assert!(one_byte_short.serialized_bytes() <= one_byte_limit);
    assert!(
        !one_byte_short
            .entries()
            .iter()
            .any(|entry| entry.hash() == child)
    );
    assert!(
        model_packing_matches_production(
            &receipt,
            one_byte_limit,
            u64::MAX,
            crate::mathematical_model::two_phase::TEMPLATE_PACKING_FAILURE_BOUND,
            &one_byte_hashes,
        ),
        "the model must match the independently fixed one-byte-short production result"
    );

    let one_cycle_short = receipt
        .pack_transactions(TemplatePackingLimits::new(usize::MAX, 29))
        .expect("a one-cycle-short limit is an ordinary packing result");
    let one_cycle_hashes = packed_hashes(&one_cycle_short);
    assert_eq!(one_cycle_hashes, vec![parent.clone()]);
    assert_eq!(
        one_cycle_short.serialized_bytes(),
        parent_tx.data().serialized_size_in_block()
    );
    assert_eq!(one_cycle_short.cycles(), 10);
    assert!(one_cycle_short.cycles() <= 29);
    assert!(
        !one_cycle_short
            .entries()
            .iter()
            .any(|entry| entry.hash() == child)
    );
    assert!(
        !one_cycle_short
            .entries()
            .iter()
            .any(|entry| entry.hash() == rival)
    );
    assert!(
        model_packing_matches_production(
            &receipt,
            usize::MAX,
            29,
            crate::mathematical_model::two_phase::TEMPLATE_PACKING_FAILURE_BOUND,
            &one_cycle_hashes,
        ),
        "the model must match the independently fixed one-cycle-short production result"
    );
}

#[test]
fn uak_template_packer_rescores_descendants_after_shared_parent_selection() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_tx = TransactionBuilder::default()
        .version(2_010u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let parent = accept(
        &mut authority,
        parent_tx.clone(),
        10,
        AcceptedStatus::Proposed,
        1,
        1,
    );
    let strongest_tx = child_transaction(2_011, &parent_tx);
    let strongest = accept(
        &mut authority,
        strongest_tx,
        11,
        AcceptedStatus::Proposed,
        1_000_000,
        1,
    );
    let rescored_tx = child_transaction_at(2_012, &parent_tx, 1);
    let rescored = accept(
        &mut authority,
        rescored_tx,
        12,
        AcceptedStatus::Proposed,
        100_000,
        1,
    );
    let rival_tx = output_transaction(2_013);
    let rival = accept(
        &mut authority,
        rival_tx,
        13,
        AcceptedStatus::Proposed,
        60_000,
        1,
    );

    let receipt = selection(&authority);
    let rescored_initial = receipt
        .candidates()
        .iter()
        .find(|candidate| candidate.hash() == &rescored)
        .expect("rescored child is captured");
    let rival_initial = receipt
        .candidates()
        .iter()
        .find(|candidate| candidate.hash() == &rival)
        .expect("rival is captured");
    assert!(
        rival_initial.order() > rescored_initial.order(),
        "the rival must start ahead of the child package"
    );
    let packed = receipt
        .pack_transactions(TemplatePackingLimits::new(usize::MAX, 3))
        .expect("dynamic CPFP scoring remains coherent");
    let hashes = packed_hashes(&packed);
    assert_eq!(hashes, vec![parent, strongest, rescored]);
    assert!(!hashes.contains(&rival));
    assert!(
        model_packing_matches_production(
            &receipt,
            usize::MAX,
            3,
            crate::mathematical_model::two_phase::TEMPLATE_PACKING_FAILURE_BOUND,
            &hashes,
        ),
        "the model must match the independently fixed rescore result"
    );
    assert_model_rejects_every_distinct_reordering(
        &receipt,
        usize::MAX,
        3,
        crate::mathematical_model::two_phase::TEMPLATE_PACKING_FAILURE_BOUND,
        &hashes,
    );
}

#[test]
fn uak_template_packer_aggregates_multi_parent_descendant_adjustments() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let first_parent_tx = output_transaction(2_014);
    let first_parent = accept(
        &mut authority,
        first_parent_tx.clone(),
        14,
        AcceptedStatus::Proposed,
        1_000_000,
        1,
    );
    let second_parent_tx = output_transaction(2_015);
    let second_parent = accept(
        &mut authority,
        second_parent_tx.clone(),
        15,
        AcceptedStatus::Proposed,
        500_000,
        1,
    );
    let child_tx = TransactionBuilder::default()
        .version(2_016u32)
        .input(CellInput::new(OutPoint::new(first_parent_tx.hash(), 0), 0))
        .input(CellInput::new(OutPoint::new(second_parent_tx.hash(), 0), 0))
        .build();
    let child = accept(&mut authority, child_tx, 16, AcceptedStatus::Proposed, 1, 1);

    let receipt = selection(&authority);
    let packed = receipt
        .pack_transactions(TemplatePackingLimits::new(usize::MAX, 3))
        .expect("both selected-parent deltas are aggregated exactly once");
    let hashes = packed_hashes(&packed);
    assert_eq!(hashes, vec![first_parent, second_parent, child]);
    assert!(
        model_packing_matches_production(
            &receipt,
            usize::MAX,
            3,
            crate::mathematical_model::two_phase::TEMPLATE_PACKING_FAILURE_BOUND,
            &hashes,
        ),
        "the model must match the independently fixed multi-parent result"
    );
}

#[test]
fn uak_template_packer_bounds_non_fitting_work_without_changing_the_policy() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let first = accept(
        &mut authority,
        TransactionBuilder::default().version(2_020u32).build(),
        20,
        AcceptedStatus::Proposed,
        1_000_000_000,
        1,
    );
    for (version, fee) in [(2_021u32, 100_000_000u64), (2_022u32, 10_000_000u64)] {
        accept(
            &mut authority,
            TransactionBuilder::default().version(version).build(),
            usize::try_from(version).expect("fixture peer fits"),
            AcceptedStatus::Proposed,
            fee,
            2,
        );
    }
    let small = accept(
        &mut authority,
        TransactionBuilder::default().version(2_023u32).build(),
        23,
        AcceptedStatus::Proposed,
        1,
        1,
    );
    let receipt = selection(&authority);

    let bounded = receipt
        .pack_transactions_for_foundation(TemplatePackingLimits::new(usize::MAX, 2), 1)
        .expect("the failure bound is a deterministic early stop");
    let bounded_hashes = packed_hashes(&bounded);
    assert_eq!(bounded_hashes, vec![first.clone()]);
    assert!(
        model_packing_matches_production(&receipt, usize::MAX, 2, 1, &bounded_hashes),
        "the model must match the independently fixed bounded-failure result"
    );

    let complete = receipt
        .pack_transactions(TemplatePackingLimits::new(usize::MAX, 2))
        .expect("the production bound reaches the later fitting candidate");
    let complete_hashes = packed_hashes(&complete);
    assert_eq!(complete_hashes, vec![first, small]);
    assert!(
        model_packing_matches_production(
            &receipt,
            usize::MAX,
            2,
            crate::mathematical_model::two_phase::TEMPLATE_PACKING_FAILURE_BOUND,
            &complete_hashes,
        ),
        "the model must match the independently fixed production-bound result"
    );
}

#[test]
fn uak_template_packer_requires_proposed_ancestors_and_orders_conditional_edges() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_tx = output_transaction(2_030);
    accept(
        &mut authority,
        parent_tx.clone(),
        30,
        AcceptedStatus::Pending,
        1,
        1,
    );
    let child_tx = child_transaction(2_031, &parent_tx);
    let child = accept(
        &mut authority,
        child_tx,
        31,
        AcceptedStatus::Proposed,
        1_000_000,
        1,
    );

    let shared = OutPoint::new(Byte32::new([201; 32]), 0);
    let reader_input = OutPoint::new(Byte32::new([202; 32]), 0);
    let reader_tx = TransactionBuilder::default()
        .version(2_032u32)
        .input(CellInput::new(reader_input.clone(), 0))
        .cell_dep(CellDep::new_builder().out_point(shared.clone()).build())
        .build();
    let reader = accept_remote_transaction_with_payload(
        &mut authority,
        reader_tx.clone(),
        32,
        AcceptedStatus::Proposed,
        resolved_payload_with_facts(
            &reader_tx,
            Vec::new(),
            vec![reader_input],
            Capacity::shannons(1),
        ),
    );
    let spender_tx = TransactionBuilder::default()
        .version(2_033u32)
        .input(CellInput::new(shared.clone(), 0))
        .build();
    let spender = accept_remote_transaction_with_payload(
        &mut authority,
        spender_tx.clone(),
        33,
        AcceptedStatus::Proposed,
        resolved_payload_with_facts(
            &spender_tx,
            Vec::new(),
            vec![shared],
            Capacity::shannons(1_000_000),
        ),
    );

    let packed = selection(&authority)
        .pack_transactions(TemplatePackingLimits::new(usize::MAX, u64::MAX))
        .expect("the selected-set conditional graph is coherent");
    let hashes = packed
        .entries()
        .iter()
        .map(|entry| entry.hash())
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
    const CYCLE_MEMBERS: u16 = 66;
    let points = (0..CYCLE_MEMBERS)
        .map(|seed| {
            let mut hash = [0u8; 32];
            hash[..2].copy_from_slice(&seed.to_le_bytes());
            OutPoint::new(Byte32::new(hash), 0)
        })
        .collect::<Vec<_>>();
    let mut authority = TxPoolAuthority::for_foundation(large_template_limits());
    let mut strongest = None;
    for (index, input) in points.iter().enumerate() {
        let mut builder = TransactionBuilder::default()
            .version(u32::try_from(2_100 + index).expect("fixture version fits"))
            .input(CellInput::new(input.clone(), 0));
        // Bidirectional adjacent edges form one SCC. Removing the weakest
        // endpoint leaves another bidirectional path, forcing 65 sequential
        // shedding rounds while each hostile transaction owns only two deps.
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
        strongest = Some(accept_remote_transaction_with_payload_and_cycles(
            &mut authority,
            transaction.clone(),
            index,
            AcceptedStatus::Proposed,
            resolved_payload_with_facts(
                &transaction,
                Vec::new(),
                vec![input.clone()],
                Capacity::shannons(
                    u64::try_from(index + 1)
                        .expect("fixture fee fits")
                        .checked_mul(100)
                        .expect("fixture fee multiplication fits"),
                ),
            ),
            1,
        ));
    }

    let packed = selection(&authority)
        .pack_transactions(TemplatePackingLimits::new(usize::MAX, u64::MAX))
        .expect("the long conditional SCC uses the bounded deterministic fallback");
    assert_eq!(packed.entries().len(), 1);
    assert_eq!(
        packed.entries()[0].hash(),
        strongest.expect("non-empty cycle")
    );
}
