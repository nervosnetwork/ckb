//! R3 pre-migration discriminator bound to the real local-removal delta.
//!
//! This is finite executable evidence.  It proves neither all-arm support
//! completeness nor global minimality; it kills an R3 design that cannot
//! derive a real disjoint write cut without a caller-owned route table.

use super::super::plan::TxPoolAuthority;
use super::super::state::AcceptedStatus;
use super::foundation::{accept_remote_transaction, admit_remote, limits, owner_version, tx};

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
    assert!(exclusive.resource_totals);
    assert!(exclusive.source_versions);
    assert!(exclusive.effect_log);
    assert!(exclusive.clocks);
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
    assert!(root_exclusive.resource_totals);
    assert!(root_exclusive.membership_counts);
    assert!(root_exclusive.source_versions);
    assert!(root_exclusive.scheduler_cursor);
    assert!(root_exclusive.clocks);
    assert!(!root_exclusive.dependency_control);
    assert!(!root_exclusive.effect_log);
}
