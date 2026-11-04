//! Proposal prefixes must advance the same prerequisites that block packing needs.

use super::graph_tests::{closure, dag, fixture};
use super::*;
use crate::authority::{model::Phase, tests::common};
use crate::block_assembler::BlockAssembler;
use ckb_chain_spec::consensus::ProposalWindow;
use ckb_proposal_table::{ProposalTable, ProposalView};
use ckb_test_chain_utils::MockStore;
use ckb_types::{core::BlockBuilder, packed::OutPoint};
use std::collections::BTreeSet;

fn phases(owners: &mut [Arc<model::Entry>], statuses: &[Status]) {
    assert_eq!(owners.len(), statuses.len());
    for (owner, &status) in owners.iter_mut().zip(statuses) {
        let mut accepted = owner.accepted().unwrap().clone();
        accepted.forced_status = Some(status);
        *owner = owner.with_phase(Phase::Accepted(accepted));
    }
}

#[test]
fn proposal_prefix_advances_ancestors_before_high_scoring_descendants() {
    let snapshot = crate::test_support::genesis_snapshot();
    let mut owners = fixture(&[vec![], vec![0], vec![1], vec![2]], |index, size| {
        (size as u64, 1, index as u64)
    });
    phases(&mut owners, &[Status::Pending; 4]);
    let selection = Selection::new(&owners, &snapshot, 4).unwrap();
    assert_eq!(
        selection.candidates_by_score().next().unwrap().hash(),
        &owners[3].hash()
    );
    let expected: Vec<_> = owners.iter().map(|owner| owner.proposal()).collect();
    for limit in 0..=5 {
        assert_eq!(
            selection.for_block().unwrap().proposal_short_ids(limit),
            expected[..(limit as usize).min(4)]
        );
    }
}

#[test]
fn proposal_walk_crosses_gap_and_proposed_ancestors() {
    let snapshot = crate::test_support::genesis_snapshot();
    let mut owners = fixture(&[vec![], vec![0], vec![1]], |index, size| {
        (size as u64, 1, index as u64)
    });
    for middle in [Status::Gap, Status::Proposed] {
        phases(&mut owners, &[Status::Pending, middle, Status::Pending]);
        let selection = Selection::new(&owners, &snapshot, 3).unwrap();
        assert_eq!(
            selection.for_block().unwrap().proposal_short_ids(1),
            vec![owners[0].proposal()]
        );
        assert_eq!(
            selection.for_block().unwrap().proposal_short_ids(3),
            vec![owners[0].proposal(), owners[2].proposal()]
        );
    }
}

#[test]
fn proposal_prefix_advances_retained_readers_before_their_spender() {
    let store = common::store();
    let point = OutPoint::new(Byte32::new([0xfe; 32]), 0);
    let reader = common::spend(8_000, &[], std::slice::from_ref(&point));
    let spender = common::spend(8_001, &[point], &[]);
    common::accept(&store, reader.clone(), 1, 1, Status::Pending);
    common::accept(&store, spender.clone(), 1_000_000, 1, Status::Pending);
    let captured = store.capture_accepted();
    let selection = Selection::new(&captured.owners, &captured.snapshot, 4).unwrap();
    assert_eq!(
        selection.for_block().unwrap().proposal_short_ids(1),
        vec![reader.proposal_short_id()]
    );
    assert_eq!(
        selection.for_block().unwrap().proposal_short_ids(2),
        vec![reader.proposal_short_id(), spender.proposal_short_id()]
    );
}

#[test]
fn proposal_prefix_keeps_a_shared_root_ahead_of_hundreds_of_children() {
    let snapshot = crate::test_support::genesis_snapshot();
    let mut parents = vec![vec![]];
    parents.extend(vec![vec![0]; 300]);
    let mut owners = fixture(&parents, |index, size| (size as u64, 1, index as u64));
    phases(&mut owners, &[Status::Pending; 301]);
    let selection = Selection::new(&owners, &snapshot, 2).unwrap();
    let block = selection.for_block().unwrap();
    for limit in [1, 2, 10, 300, 301, u64::MAX] {
        let proposals = block.proposal_short_ids(limit);
        assert_eq!(proposals.len(), (limit as usize).min(owners.len()));
        assert_eq!(proposals[0], owners[0].proposal());
        assert_eq!(
            proposals.iter().collect::<BTreeSet<_>>().len(),
            proposals.len()
        );
    }
}

#[test]
fn proposal_prefixes_cover_pending_closures_in_every_four_node_dag_and_phase() {
    let snapshot = crate::test_support::genesis_snapshot();
    for mask in 0..1 << 6 {
        let parents = dag(4, mask);
        let mut owners = fixture(&parents, |index, size| {
            (size as u64 * [1, 9, 4, 100][index], 1, index as u64)
        });
        for mut phase_bits in 0..3usize.pow(4) {
            let statuses: [_; 4] = std::array::from_fn(|_| {
                let status = [Status::Pending, Status::Gap, Status::Proposed][phase_bits % 3];
                phase_bits /= 3;
                status
            });
            phases(&mut owners, &statuses);
            let selection = Selection::new(&owners, &snapshot, 4).unwrap();
            let block = selection.for_block().unwrap();
            let full = block.proposal_short_ids(u64::MAX);
            assert_eq!(
                full.len(),
                statuses
                    .iter()
                    .filter(|status| **status == Status::Pending)
                    .count()
            );
            for limit in 0..=5 {
                let proposals = block.proposal_short_ids(limit);
                assert_eq!(proposals, full[..(limit as usize).min(full.len())]);
                let mut seen = BTreeSet::new();
                for id in proposals {
                    let index = owners
                        .iter()
                        .position(|owner| owner.proposal() == id)
                        .unwrap();
                    assert_eq!(statuses[index], Status::Pending);
                    assert!(seen.insert(index), "duplicate proposal");
                    for ancestor in closure(index, &parents) {
                        assert!(
                            statuses[ancestor] != Status::Pending || seen.contains(&ancestor),
                            "mask={mask}, statuses={statuses:?}, index={index}, ancestor={ancestor}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn proposal_byte_prefix_does_not_depend_on_an_uncle_that_may_be_filtered() {
    let snapshot = crate::test_support::genesis_snapshot();
    let mut owners = fixture(&[vec![], vec![0], vec![1]], |index, size| {
        (size as u64, 1, index as u64)
    });
    phases(&mut owners, &[Status::Pending; 3]);
    let selection = Selection::new(&owners, &snapshot, 3).unwrap();
    let block = selection.for_block().unwrap();
    let root = owners[0].proposal();
    let uncle = BlockBuilder::default()
        .number(1)
        .epoch(snapshot.epoch_ext().number_with_fraction(1))
        .parent_hash(snapshot.tip_header().hash())
        .proposals(vec![root.clone()])
        .build()
        .as_uncle();
    let base = 1_000;
    let proposal_bytes = ProposalShortId::serialized_size();
    for bytes in [
        proposal_bytes,
        2 * proposal_bytes - 1,
        3 * proposal_bytes + ckb_types::core::UncleBlockView::serialized_size_in_block(),
    ] {
        let fitted = BlockAssembler::fit_optional_content(
            &snapshot,
            block.proposal_short_ids(3),
            std::slice::from_ref(&uncle),
            base,
            base + bytes,
        )
        .unwrap();
        assert_eq!(fitted.proposals[0], root);
        assert!(
            fitted.uncles.is_empty(),
            "a main proposal conflict removes the tentative uncle"
        );
        assert_eq!(fitted.proposals.len(), (bytes / proposal_bytes).min(3));
    }
}

#[test]
fn proposal_prefix_drains_a_chain_through_the_real_proposal_window() {
    let base = crate::test_support::genesis_snapshot();
    let db = MockStore::default();
    let parents: Vec<_> = (0..8)
        .map(|index: usize| index.checked_sub(1).into_iter().collect())
        .collect();
    let mut owners = fixture(&parents, |index, size| (size as u64, 1, index as u64));
    for owner in &mut owners {
        let mut accepted = owner.accepted().unwrap().clone();
        accepted.forced_status = None;
        *owner = owner.with_phase(Phase::Accepted(accepted));
    }
    let expected: Vec<_> = owners.iter().map(|owner| owner.hash()).collect();
    let mut committed = Vec::new();
    let mut table = ProposalTable::new(ProposalWindow(2, 5)).unwrap();
    let mut view = ProposalView::default();
    let mut cache = Cache::default();
    for height in 1..=7 {
        let snapshot = Snapshot::new(
            base.tip_header()
                .clone()
                .as_advanced_builder()
                .number(height - 1)
                .epoch(base.epoch_ext().number_with_fraction(height - 1))
                .build(),
            base.total_difficulty().clone(),
            base.epoch_ext().clone(),
            db.store().get_snapshot(),
            view.clone(),
            base.cloned_consensus(),
        );
        let selection = cache.selection(&owners, &snapshot, 8).unwrap();
        let block = selection.for_block().unwrap();
        let proposals = block.proposal_short_ids(2);
        let packed = block
            .pack_transactions(TemplatePackingLimits::new(usize::MAX, 2))
            .unwrap();
        assert_eq!(
            packed.len(),
            if height < 3 {
                0
            } else if height <= 6 {
                2
            } else {
                0
            }
        );
        for entry in &packed {
            assert!(view.contains_proposed(&entry.proposal_short_id()));
            committed.push(entry.transaction().hash());
        }
        drop(selection);
        owners.retain(|owner| !committed.contains(&owner.hash()));
        for owner in &mut owners {
            let mut accepted = owner.accepted().unwrap().clone();
            accepted
                .parents
                .retain(|parent| !committed.contains(parent));
            *owner = owner.with_phase(Phase::Accepted(accepted));
        }
        table.insert(height, proposals);
        view = table.finalize(&view, height);
    }
    assert!(owners.is_empty());
    assert_eq!(committed, expected);
}
