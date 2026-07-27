use super::*;
use crate::component::pool_map::Status;
use crate::test_support::{MOCK_CYCLES, build_tx};
use ckb_types::core::{Capacity, TransactionBuilder, TransactionView};
use ckb_types::packed::{Byte32, CellDep, CellInput, OutPoint};
use ckb_types::prelude::*;

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
        selector
            .update_modified_entries(&committed, &committed_ids)
            .unwrap();
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
        selector.descendants_of(&tx.proposal_short_id()).unwrap();
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

fn tx_with_input_and_dep(input: OutPoint, dep: OutPoint) -> TransactionView {
    TransactionBuilder::default()
        .input(CellInput::new_builder().previous_output(input).build())
        .cell_dep(CellDep::new_builder().out_point(dep).build())
        .build()
}

#[test]
fn selected_reader_is_ordered_before_spender() {
    let shared = OutPoint::new(Byte32::new([0x31; 32]), 0);
    let reader = tx_with_input_and_dep(OutPoint::new(Byte32::new([0x41; 32]), 0), shared.clone());
    let spender = TransactionBuilder::default()
        .input(
            CellInput::new_builder()
                .previous_output(shared.clone())
                .build(),
        )
        .build();
    let mut pool = PoolMap::new(125);
    add_proposed(&mut pool, &reader, 100);
    add_proposed(&mut pool, &spender, 10_000);

    let (selected, _, _) = TxSelector::new(&pool)
        .txs_to_commit(usize::MAX, u64::MAX)
        .unwrap();
    let ids = selected
        .iter()
        .map(TxEntry::proposal_short_id)
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![reader.proposal_short_id(), spender.proposal_short_id()]
    );
}

#[test]
fn conditional_cycle_drops_weakest_member() {
    let x = OutPoint::new(Byte32::new([0x51; 32]), 0);
    let y = OutPoint::new(Byte32::new([0x52; 32]), 0);
    // A reads x and spends y; B reads y and spends x.
    let a = tx_with_input_and_dep(y.clone(), x.clone());
    let b = tx_with_input_and_dep(x, OutPoint::new(Byte32::new([0x52; 32]), 0));
    let mut pool = PoolMap::new(125);
    add_proposed(&mut pool, &a, 100);
    add_proposed(&mut pool, &b, 10_000);

    let (selected, _, _) = TxSelector::new(&pool)
        .txs_to_commit(usize::MAX, u64::MAX)
        .unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].proposal_short_id(), b.proposal_short_id());
}

#[test]
fn conditional_cycle_does_not_drop_acyclic_downstream_entry() {
    let x = OutPoint::new(Byte32::new([0x61; 32]), 0);
    let y = OutPoint::new(Byte32::new([0x62; 32]), 0);
    let z = OutPoint::new(Byte32::new([0x63; 32]), 0);
    // A -> B and B -> A form the only cycle. B -> C is conditional but C is
    // not a cycle member, even though it remains blocked in Kahn's residual.
    let a = tx_with_input_and_dep(y.clone(), x.clone());
    let b = TransactionBuilder::default()
        .input(CellInput::new_builder().previous_output(x).build())
        .cell_dep(CellDep::new_builder().out_point(y).build())
        .cell_dep(CellDep::new_builder().out_point(z.clone()).build())
        .build();
    let c = TransactionBuilder::default()
        .input(CellInput::new_builder().previous_output(z).build())
        .build();
    let mut pool = PoolMap::new(125);
    add_proposed(&mut pool, &a, 100);
    add_proposed(&mut pool, &b, 10_000);
    add_proposed(&mut pool, &c, 1);

    let (selected, _, _) = TxSelector::new(&pool)
        .txs_to_commit(usize::MAX, u64::MAX)
        .unwrap();
    let ids = selected
        .iter()
        .map(TxEntry::proposal_short_id)
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![b.proposal_short_id(), c.proposal_short_id()]);
}

#[test]
fn dense_conditional_scc_uses_bounded_fallback_and_keeps_strongest() {
    let points = (0u16..66)
        .map(|seed| {
            let mut hash = [0u8; 32];
            hash[..2].copy_from_slice(&seed.to_le_bytes());
            OutPoint::new(Byte32::new(hash), 0)
        })
        .collect::<Vec<_>>();
    let mut pool = PoolMap::new(125);
    let mut strongest = None;
    for (index, input) in points.iter().enumerate() {
        let mut builder = TransactionBuilder::default().input(
            CellInput::new_builder()
                .previous_output(input.clone())
                .build(),
        );
        for (dep_index, dep) in points.iter().enumerate() {
            if dep_index != index {
                builder = builder.cell_dep(CellDep::new_builder().out_point(dep.clone()).build());
            }
        }
        let tx = builder.build();
        add_proposed(&mut pool, &tx, (index as u64 + 1) * 100);
        strongest = Some(tx.proposal_short_id());
    }

    let (selected, _, _) = TxSelector::new(&pool)
        .txs_to_commit(usize::MAX, u64::MAX)
        .unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(
        selected[0].proposal_short_id(),
        strongest.expect("dense SCC has a strongest entry")
    );
}

#[test]
fn over_budget_dep_entry_does_not_censor_independent_suffix() {
    let dep_a = OutPoint::new(Byte32::new([0x71; 32]), 0);
    let dep_b = OutPoint::new(Byte32::new([0x72; 32]), 0);
    let expensive = TransactionBuilder::default()
        .input(
            CellInput::new_builder()
                .previous_output(OutPoint::new(Byte32::new([0x73; 32]), 0))
                .build(),
        )
        .cell_dep(CellDep::new_builder().out_point(dep_a).build())
        .cell_dep(CellDep::new_builder().out_point(dep_b).build())
        .build();
    let independent = TransactionBuilder::default()
        .input(
            CellInput::new_builder()
                .previous_output(OutPoint::new(Byte32::new([0x74; 32]), 0))
                .build(),
        )
        .build();
    let mut pool = PoolMap::new(125);
    add_proposed(&mut pool, &expensive, 10_000);
    add_proposed(&mut pool, &independent, 1);

    let selector = TxSelector::new(&pool);
    let retained = selector
        .retain_selected_with_dep_budget(
            vec![
                pool.get(&expensive.proposal_short_id()).unwrap().clone(),
                pool.get(&independent.proposal_short_id()).unwrap().clone(),
            ],
            1,
        )
        .unwrap();
    assert_eq!(retained.len(), 1);
    assert_eq!(
        retained[0].proposal_short_id(),
        independent.proposal_short_id()
    );
}
