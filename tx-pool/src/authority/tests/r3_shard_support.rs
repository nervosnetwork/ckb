//! Physical-support canaries for the canonical shared local-removal adapter.

use super::super::plan::TxPoolAuthority;
use super::super::state::{AcceptedStatus, RawTxHash};
use super::foundation::{
    accept_remote_transaction, accept_remote_transaction_with_payload, admit_remote, apply_plan,
    limits, owner_version, resolved_payload_with_facts, tx, verify_remote_transaction_with_payload,
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
fn shared_local_removal_derives_one_disjoint_physical_pair() {
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
            .prepare_shared_local_removal_for_foundation(&first)
            .expect("first owner has one valid administrative closure")
            .expect("first owner remains present");
        let root_support = root_plan.physical_write_support_for_foundation();
        drop(root_plan);

        let plan = authority
            .prepare_shared_local_removal_for_foundation(&independent)
            .expect("independent owner has one local-removal plan")
            .expect("independent owner remains present");
        let support = plan.physical_write_support_for_foundation();
        if root_support.is_disjoint(support) {
            found = Some((first, independent));
            break;
        }
    }
    let (first, independent) =
        found.expect("the fixed 64-shard layout admits a real disjoint removal pair");
    assert_ne!(first, independent);
}

#[test]
fn real_owner_shard_write_guards_for_a_disjoint_pair_coexist() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let first = accept_remote_transaction(
        &mut authority,
        tx(1_105),
        1_105,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let second = (1_106usize..1_300)
        .find_map(|seed| {
            let candidate = accept_remote_transaction(
                &mut authority,
                tx(seed as u64),
                seed,
                AcceptedStatus::Pending,
                Vec::new(),
            );
            (authority.entries_for_reference().owner_shard(&candidate)
                != authority.entries_for_reference().owner_shard(&first))
            .then_some(candidate)
        })
        .expect("the fixed layout yields a second real owner shard");

    let entries = authority.entries_for_reference();
    let first_owner = entries
        .get(&first)
        .as_deref()
        .cloned()
        .expect("the first Accepted owner exists");
    let second_owner = entries
        .get(&second)
        .as_deref()
        .cloned()
        .expect("the second Accepted owner exists");
    let first_support = entries.owner_write_support(std::iter::once(&first));
    let second_support = entries.owner_write_support(std::iter::once(&second));

    let mut first_cut = entries.write_cut(first_support);
    let mut second_cut = entries
        .try_write_cut(second_support)
        .expect("a disjoint real owner shard has no common exclusive guard");
    let first_previous = first_cut.replace(
        entries.owner_shard(&first),
        first.clone(),
        Some(first_owner),
    );
    let second_previous = second_cut.replace(
        entries.owner_shard(&second),
        second.clone(),
        Some(second_owner),
    );
    drop(second_cut);
    drop(first_cut);
    drop(second_previous);
    drop(first_previous);

    assert!(authority.primary_projection_consistent());
}

#[test]
fn multi_owner_support_holds_one_atomic_sorted_guard_bundle() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let first = accept_remote_transaction(
        &mut authority,
        tx(1_301),
        1_301,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let second = (1_302usize..1_500)
        .find_map(|seed| {
            let candidate = accept_remote_transaction(
                &mut authority,
                tx(seed as u64),
                seed,
                AcceptedStatus::Pending,
                Vec::new(),
            );
            (authority.entries_for_reference().owner_shard(&candidate)
                != authority.entries_for_reference().owner_shard(&first))
            .then_some(candidate)
        })
        .expect("the fixed layout yields a distinct second owner shard");
    let entries = authority.entries_for_reference();
    let combined = entries.owner_write_support([&first, &second]);
    let first_only = entries.owner_write_support(std::iter::once(&first));
    let second_only = entries.owner_write_support(std::iter::once(&second));

    let combined_cut = entries.write_cut(combined);
    assert!(entries.try_write_cut(first_only).is_none());
    assert!(entries.try_write_cut(second_only).is_none());
    drop(combined_cut);
    assert!(entries.try_write_cut(first_only).is_some());
    assert!(entries.try_write_cut(second_only).is_some());
}

#[test]
fn dependency_loss_uses_the_same_shared_local_removal_adapter() {
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
        .prepare_shared_local_removal_for_foundation(&RawTxHash(root.hash()))
        .expect("root has one local-removal plan")
        .expect("root remains present");
    apply_plan(root_plan);
    assert!(authority.primary_projection_consistent());

    let independent_plan = authority
        .prepare_shared_local_removal_for_foundation(&independent)
        .expect("independent owner has one local-removal plan")
        .expect("independent owner remains present");
    apply_plan(independent_plan);
    assert!(authority.primary_projection_consistent());
}
