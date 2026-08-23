//! R3 pre-migration discriminator bound to the real local-removal delta.
//!
//! This is finite executable evidence.  It proves neither all-arm support
//! completeness nor global minimality; it kills an R3 design that cannot
//! derive a real disjoint write cut without a caller-owned route table.

use super::super::plan::TxPoolAuthority;
use super::super::state::{AcceptedStatus, RawTxHash};
use super::foundation::{
    accept_remote_transaction, accept_remote_transaction_with_payload, admit_remote, limits,
    owner_version, resolved_payload_with_facts, tx, verify_remote_transaction_with_payload,
};
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, TransactionBuilder},
    packed::{CellDep, CellInput, CellOutput, OutPoint},
    prelude::{Builder, Entity, Pack},
};

#[test]
fn real_entry_delta_derives_support_from_its_typed_payload() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let queued = admit_remote(&mut authority, 909, 909);
    let version = owner_version(&authority, &queued);
    let plan = authority
        .plan_terminalize_for_foundation(&queued, version)
        .expect("queued owner has one terminal Entry delta");
    let (support, exclusive) = plan
        .entry_shard_support()
        .expect("terminalization is the real Entry arm");

    assert!(support.len() > 0);
    assert!(exclusive.effect_log);
    assert!(!exclusive.membership_counts);
}

#[test]
fn real_local_removal_delta_derives_one_disjoint_shard_pair_and_names_globals() {
    let mut found = None;
    for seed in 912usize..1_104 {
        let mut authority = TxPoolAuthority::for_foundation(limits());
        let first = accept_remote_transaction(
            &mut authority,
            tx(910),
            910,
            AcceptedStatus::Pending,
            Vec::new(),
        );
        let independent = accept_remote_transaction(
            &mut authority,
            tx(seed as u64),
            seed,
            AcceptedStatus::Pending,
            Vec::new(),
        );

        let root_plan = authority
            .plan_local_removal(&first)
            .expect("first owner has one valid administrative closure")
            .expect("first owner remains present");
        let (root_support, root_exclusive) = root_plan
            .local_removal_shard_support()
            .expect("local removal has structural shard support");
        drop(root_plan);

        let plan = authority
            .plan_local_removal(&independent)
            .expect("independent owner has one local-removal plan")
            .expect("independent owner remains present");
        let (support, exclusive) = plan
            .local_removal_shard_support()
            .expect("local removal has structural shard support");
        if root_support.is_disjoint(support) {
            found = Some((root_support, root_exclusive, support, exclusive));
            break;
        }
    }
    let (root_support, root_exclusive, independent_support, independent_exclusive) =
        found.expect("the fixed 64-shard layout admits a real disjoint removal pair");

    assert!(root_support.len() > 0);
    assert!(independent_support.len() > 0);
    assert_eq!(root_exclusive, independent_exclusive);
    assert!(!root_exclusive.membership_counts);
    assert!(!root_exclusive.scheduler_cursor);
    assert!(!root_exclusive.dependency_control);
    assert!(!root_exclusive.effect_log);
}

#[test]
fn dependency_control_is_exclusive_only_for_a_real_loss_event() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let root = TransactionBuilder::default()
        .version(920u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    accept_remote_transaction_with_payload(
        &mut authority,
        root.clone(),
        920,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(&root, Vec::new(), Vec::new(), Capacity::shannons(1_000)),
    );
    let root_output = OutPoint::new(root.hash(), 0);
    let waiter_tx = TransactionBuilder::default()
        .version(921u32)
        .cell_dep(
            CellDep::new_builder()
                .out_point(root_output.clone())
                .build(),
        )
        .input(CellInput::new(OutPoint::new(root.hash(), 0), 0))
        .build();
    let _waiter = verify_remote_transaction_with_payload(
        &mut authority,
        waiter_tx.clone(),
        921,
        resolved_payload_with_facts(
            &waiter_tx,
            vec![root_output],
            Vec::new(),
            Capacity::shannons(2_000),
        ),
    );
    let independent = accept_remote_transaction(
        &mut authority,
        tx(922),
        922,
        AcceptedStatus::Pending,
        Vec::new(),
    );

    let root_plan = authority
        .plan_local_removal(&RawTxHash(root.hash()))
        .expect("root has one local-removal plan")
        .expect("root remains present");
    let (_, root_exclusive) = root_plan
        .local_removal_shard_support()
        .expect("local removal derives support");
    assert!(root_exclusive.dependency_control);
    let _ = root_plan.apply();
    assert!(authority.primary_projection_consistent());

    let independent_plan = authority
        .plan_local_removal(&independent)
        .expect("independent owner has one local-removal plan")
        .expect("independent owner remains present");
    let (_, independent_exclusive) = independent_plan
        .local_removal_shard_support()
        .expect("local removal derives support");
    assert!(!independent_exclusive.dependency_control);
    let _ = independent_plan.apply();
    assert!(authority.primary_projection_consistent());
}
