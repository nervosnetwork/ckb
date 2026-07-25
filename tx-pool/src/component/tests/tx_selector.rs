use super::*;
use crate::component::pool_map::Status;
use crate::component::tests::util::{MOCK_CYCLES, build_tx};
use ckb_types::core::{Capacity, TransactionView};
use ckb_types::packed::Byte32;

const SIZE: usize = 100;

impl TxSelector<'_> {
    fn set_descendants_cache_budget_for_test(&mut self, budget: usize) {
        self.descendants_cache_budget = budget;
    }
}

fn add_proposed(pool_map: &mut PoolMap, tx: &TransactionView, fee: u64) {
    pool_map
        .add_entry(
            TxEntry::dummy_resolve(tx.clone(), MOCK_CYCLES, Capacity::shannons(fee), SIZE),
            Status::Proposed,
        )
        .unwrap();
}

/// A CPFP-shaped graph: roots A and B, child C spending both, grandchild
/// D spending C. Committed A and B share descendants C and D.
fn shared_descendant_graph() -> (PoolMap, [TransactionView; 4]) {
    let mut pool_map = PoolMap::new(125);
    let a = build_tx(vec![(&Byte32::new([1u8; 32]), 0)], 1);
    let b = build_tx(vec![(&Byte32::new([2u8; 32]), 0)], 1);
    let c = build_tx(vec![(&a.hash(), 0), (&b.hash(), 0)], 1);
    let d = build_tx(vec![(&c.hash(), 0)], 1);
    add_proposed(&mut pool_map, &a, 1_000);
    add_proposed(&mut pool_map, &b, 2_000);
    add_proposed(&mut pool_map, &c, 3_000);
    add_proposed(&mut pool_map, &d, 4_000);
    (pool_map, [a, b, c, d])
}

/// Aggregate batch subtraction must produce exactly the same adjusted
/// entries as before, with and without the descendants cache, and match
/// the hand-computed expectation.
#[test]
fn aggregate_adjustments_match_across_cache_modes() {
    let (pool_map, [a, b, c, d]) = shared_descendant_graph();
    let committed: Vec<(ProposalShortId, TxEntry)> = [&a, &b]
        .iter()
        .map(|tx| {
            let id = tx.proposal_short_id();
            (id.clone(), pool_map.get(&id).cloned().unwrap())
        })
        .collect();
    let committed_ids: HashSet<ProposalShortId> =
        committed.iter().map(|(id, _)| id.clone()).collect();

    let mut results = Vec::new();
    for budget in [usize::MAX, 0] {
        let mut selector = TxSelector::new(&pool_map);
        selector.set_descendants_cache_budget_for_test(budget);
        selector.update_modified_entries(&committed, &committed_ids);
        let c_adj = selector
            .modified_entries
            .get(&c.proposal_short_id())
            .cloned()
            .expect("C is adjusted");
        let d_adj = selector
            .modified_entries
            .get(&d.proposal_short_id())
            .cloned()
            .expect("D is adjusted");
        results.push((c_adj, d_adj));
    }
    let (c_transient, d_transient) = results.pop().unwrap();
    let (c_cached, d_cached) = results.pop().unwrap();
    assert_eq!(c_cached, c_transient);
    assert_eq!(d_cached, d_transient);

    // C's ancestors were exactly {A, B}: subtracting both leaves only
    // C's own weight.
    assert_eq!(c_cached.ancestors_count, 1);
    assert_eq!(c_cached.ancestors_size, SIZE);
    assert_eq!(c_cached.ancestors_fee, Capacity::shannons(3_000));
    // D's ancestors were {A, B, C}: subtracting A and B leaves D + C.
    assert_eq!(d_cached.ancestors_count, 2);
    assert_eq!(d_cached.ancestors_size, 2 * SIZE);
    assert_eq!(d_cached.ancestors_fee, Capacity::shannons(7_000));
}

/// The cache must never hold more memberships than its budget allows,
/// no matter how wide the descendant graph is; over-budget lookups fall
/// back to transient (uncached) results.
#[test]
fn descendants_cache_members_stay_within_budget() {
    let (pool_map, [a, b, c, d]) = shared_descendant_graph();
    let mut selector = TxSelector::new(&pool_map);
    selector.set_descendants_cache_budget_for_test(2);

    for tx in [&a, &b, &c, &d] {
        let _ = selector.descendants_of(&tx.proposal_short_id());
    }
    assert!(selector.descendants_cache_members <= 2);
    // A's descendant set ({C, D}) fit exactly; B's identical set pushed
    // the total over budget and was served transiently.
    assert!(
        selector
            .descendants_cache
            .contains_key(&a.proposal_short_id())
    );
    assert!(
        !selector
            .descendants_cache
            .contains_key(&b.proposal_short_id())
    );
}
