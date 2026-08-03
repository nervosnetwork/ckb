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
    assert_eq!(exact.serialized_bytes(), package_bytes);
    assert_eq!(exact.cycles(), 30);
    assert_eq!(
        exact
            .entries()
            .iter()
            .map(|entry| entry.hash())
            .collect::<Vec<_>>(),
        vec![&parent, &child]
    );
    assert_eq!(exact.entries()[0].accepted_at().0, 0);
    assert_eq!(exact.entries()[1].metrics().cost.cycles, 20);
    assert_eq!(
        exact.entries()[1].proposal_short_id(),
        &child_tx.proposal_short_id()
    );
    assert_eq!(exact.entries()[1].resolved().transaction.hash(), child.0);
    assert!(!exact.entries().iter().any(|entry| entry.hash() == &rival));

    let one_byte_short = receipt
        .pack_transactions(TemplatePackingLimits::new(package_bytes - 1, u64::MAX))
        .expect("a one-byte-short limit is an ordinary packing result");
    assert!(
        !one_byte_short
            .entries()
            .iter()
            .any(|entry| entry.hash() == &child)
    );

    let one_cycle_short = receipt
        .pack_transactions(TemplatePackingLimits::new(usize::MAX, 29))
        .expect("a one-cycle-short limit is an ordinary packing result");
    assert!(
        !one_cycle_short
            .entries()
            .iter()
            .any(|entry| entry.hash() == &child)
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
    let hashes = packed
        .entries()
        .iter()
        .map(|entry| entry.hash())
        .collect::<Vec<_>>();
    assert_eq!(hashes, vec![&parent, &strongest, &rescored]);
    assert!(!hashes.contains(&&rival));
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

    let packed = selection(&authority)
        .pack_transactions(TemplatePackingLimits::new(usize::MAX, 3))
        .expect("both selected-parent deltas are aggregated exactly once");
    assert_eq!(
        packed
            .entries()
            .iter()
            .map(|entry| entry.hash())
            .collect::<Vec<_>>(),
        vec![&first_parent, &second_parent, &child]
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
    assert_eq!(
        bounded
            .entries()
            .iter()
            .map(|entry| entry.hash())
            .collect::<Vec<_>>(),
        vec![&first]
    );

    let complete = receipt
        .pack_transactions(TemplatePackingLimits::new(usize::MAX, 2))
        .expect("the production bound reaches the later fitting candidate");
    assert_eq!(
        complete
            .entries()
            .iter()
            .map(|entry| entry.hash())
            .collect::<Vec<_>>(),
        vec![&first, &small]
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
    assert!(!hashes.contains(&&child));
    let reader_position = hashes
        .iter()
        .position(|hash| *hash == &reader)
        .expect("reader is selected");
    let spender_position = hashes
        .iter()
        .position(|hash| *hash == &spender)
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
        strongest.as_ref().expect("non-empty cycle")
    );
}
