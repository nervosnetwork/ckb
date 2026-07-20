//! Regression test: an RBF replacement whose commit is *certain* to fail
//! must be rejected before its conflicts are removed.
//!
//! `prepare_rbf_replacement` used to remove the conflicted (victim) cluster
//! first and only discover commit failures afterwards, so every failed
//! attempt evicted the victim cluster and then restored it. A replacement
//! crafted to always fail (here: more than `max_ancestors_count` in-pool
//! ancestors via a cell dep on a long chain) could repeat that
//! remove-and-restore churn indefinitely at no cost — a failed replacement
//! pays no fee. `PoolMap::validate_ancestor_capacity` now rejects such
//! entries before anything is removed.

use crate::{
    component::{entry::TxEntry, pool_map::Status},
    error::Reject,
};
use ckb_types::{core::Capacity, packed::Byte32};

use super::pipeline::service_with_rbf;
use super::util::{MOCK_CYCLES, MOCK_SIZE, build_tx, build_tx_with_dep};

/// The pool accepts a chain link `a_i` (spending `a_{i-1}:0`) while
/// `i + 1 <= max_ancestors_count`, so the deepest addable chain is exactly
/// `max_ancestors_count` transactions long.
const CHAIN_LEN: u32 = 125;

#[tokio::test]
async fn rbf_replacement_certain_to_fail_commit_cannot_churn_pool() {
    let (service, _relay, _cancel, _store, _out_points) = service_with_rbf(2);

    // The victim cluster: a tx and its child, both in the pool.
    let victim = build_tx(vec![(&Byte32::zero(), 1)], 1);
    let victim_child = build_tx(vec![(&victim.hash(), 0)], 1);

    // The attack entry's in-pool ancestry: the deepest chain the pool
    // accepts. A cell dep on the chain tip then pushes the attack entry
    // over the ancestor limit even after the victim cluster is removed.
    let mut chain_tip = Byte32::zero();
    {
        let mut tx_pool = service.tx_pool.write().await;
        for _ in 0..CHAIN_LEN {
            let link = build_tx(vec![(&chain_tip, 0)], 1);
            chain_tip = link.hash();
            tx_pool
                .pool_map
                .add_entry(
                    TxEntry::dummy_resolve(link, MOCK_CYCLES, Capacity::zero(), MOCK_SIZE),
                    Status::Pending,
                )
                .unwrap();
        }
        for tx in [&victim, &victim_child] {
            tx_pool
                .pool_map
                .add_entry(
                    TxEntry::dummy_resolve(tx.clone(), MOCK_CYCLES, Capacity::zero(), MOCK_SIZE),
                    Status::Pending,
                )
                .unwrap();
        }
    }

    // Conflicts with the victim by spending the same input; passes the RBF
    // rules (inputs are all conflict inputs; fee is far above the victim
    // cluster's) but can never be committed: its in-pool ancestors are the
    // whole chain (125) plus itself, exceeding the limit of 125.
    let attack = build_tx_with_dep(vec![(&Byte32::zero(), 1)], vec![(&chain_tip, 0)], 1);
    let attack_entry = TxEntry::dummy_resolve(
        attack,
        MOCK_CYCLES,
        Capacity::shannons(1_000_000_000),
        MOCK_SIZE,
    );
    let attack_id = attack_entry.proposal_short_id();

    let (result, recovered, reject_events) = {
        let mut tx_pool = service.tx_pool.write().await;
        let snapshot = tx_pool.cloned_snapshot();
        let pre_resolve_tip = snapshot.tip_hash();
        service.try_submit_entry(
            &mut tx_pool,
            snapshot,
            pre_resolve_tip,
            attack_entry,
            Status::Pending,
            attack_id,
        )
    };

    // The replacement is rejected *before* any removal: nothing was
    // evicted, nothing needs recovering, and the victim cluster never left
    // the pool.
    assert!(matches!(result, Err(Reject::ExceededMaximumAncestorsCount)));
    assert!(recovered.is_empty());
    assert!(reject_events.is_empty());
    let tx_pool = service.tx_pool.read().await;
    let resident = |tx: &ckb_types::core::TransactionView| {
        tx_pool
            .pool_map
            .get_by_id(&tx.proposal_short_id())
            .is_some_and(|entry| entry.inner.transaction().hash() == tx.hash())
    };
    assert!(resident(&victim), "victim must remain in the pool");
    assert!(
        resident(&victim_child),
        "victim's descendant must remain in the pool"
    );
}

/// `conflict_closure` must abort as soon as the union exceeds the given
/// limit, making rule #5's candidate cap the hard bound on traversal cost
/// regardless of pool population.
#[test]
fn conflict_closure_aborts_at_candidate_limit() {
    use crate::component::pool_map::{ConflictClosure, PoolMap};
    use std::collections::HashSet;

    let mut pool = PoolMap::new(super::util::DEFAULT_MAX_ANCESTORS_COUNT);
    // A chain one link longer than the candidate limit (depth stays within
    // the pool's ancestor limit).
    let chain_len = 101u32;
    let mut tip = Byte32::zero();
    let mut chain = Vec::new();
    for _ in 0..chain_len {
        let link = build_tx(vec![(&tip, 0)], 1);
        tip = link.hash();
        chain.push(link.clone());
        pool.add_entry(
            TxEntry::dummy_resolve(link, MOCK_CYCLES, Capacity::zero(), MOCK_SIZE),
            Status::Pending,
        )
        .unwrap();
    }
    let root = chain[0].proposal_short_id();
    let roots = HashSet::from([root.clone()]);

    match pool.conflict_closure(&roots, 100) {
        ConflictClosure::Exceeded { count_lower_bound } => {
            assert_eq!(count_lower_bound, 101);
        }
        ConflictClosure::Complete { .. } => panic!("closure must abort above the limit"),
    }

    // The same structure completes within a higher limit, ordered children
    // before parents (chain tip first, root last).
    match pool.conflict_closure(&roots, 125) {
        ConflictClosure::Complete {
            removal,
            removal_set,
        } => {
            assert_eq!(removal.len(), chain_len as usize);
            assert_eq!(removal_set.len(), chain_len as usize);
            assert_eq!(
                removal.first(),
                Some(&chain[chain.len() - 1].proposal_short_id())
            );
            assert_eq!(removal.last(), Some(&root));
        }
        ConflictClosure::Exceeded { .. } => panic!("closure must complete within the limit"),
    }
}
