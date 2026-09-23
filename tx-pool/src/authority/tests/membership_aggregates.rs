//! Full-graph totals agree with independent per-owner closure calculations.
use super::super::{
    membership::{self, Aggregate, Members},
    model::{Accepted, Entry, Error, Phase, Source, Status},
};
use super::common::spend;
use crate::error::Reject;
use ckb_types::{
    core::{Capacity, cell::ResolvedTransaction},
    packed::{Byte32, OutPoint},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

fn fixture(parents: &[Vec<usize>]) -> Members {
    let mut owners: Vec<Arc<Entry>> = Vec::with_capacity(parents.len());
    for (index, parents) in parents.iter().enumerate() {
        let points: Vec<_> = parents
            .iter()
            .map(|parent| OutPoint::new(owners[*parent].hash(), 0))
            .collect();
        let transaction = Arc::new(spend(index as u32, &[], &points));
        let value = Accepted {
            transaction: Arc::new(ResolvedTransaction::dummy_resolve(
                transaction.as_ref().clone(),
            )),
            size: transaction.data().serialized_size_in_block(),
            cycles: index as u64 * 17 + 1,
            fee: Capacity::shannons(index as u64 * 23 + 1),
            timestamp: index as u64,
            parents: parents
                .iter()
                .map(|parent| owners[*parent].hash())
                .collect(),
            context_sensitive: false,
            forced_status: Some(Status::Proposed),
        };
        owners.push(Arc::new(Entry {
            transaction,
            arrival: index as u64,
            source: Source::Local,
            phase: Phase::Accepted(value),
        }));
    }
    owners
        .into_iter()
        .map(|owner| (owner.hash(), owner))
        .collect()
}

fn reference(members: &Members, max_ancestors: usize) -> BTreeMap<Byte32, (Aggregate, Aggregate)> {
    let children = membership::children(members);
    members
        .keys()
        .map(|hash| {
            let ancestors = membership::ancestor_hashes(members, hash, max_ancestors).unwrap();
            let descendants =
                membership::descendant_hashes(&children, [hash.clone()], members.len()).unwrap();
            (
                hash.clone(),
                (
                    membership::aggregate(members, &ancestors).unwrap(),
                    membership::aggregate(members, &descendants).unwrap(),
                ),
            )
        })
        .collect()
}

fn change(members: &mut Members, hash: &Byte32, update: impl FnOnce(&mut Accepted)) {
    let owner = members.get(hash).unwrap();
    let mut accepted = owner.accepted().unwrap().clone();
    update(&mut accepted);
    members.insert(hash.clone(), owner.with_phase(Phase::Accepted(accepted)));
}

#[test]
fn complete_totals_match_every_five_node_dag_and_a_full_ancestor_chain() {
    // Every subset of the ten forward edges includes shared ancestors,
    // diamonds, independent components and multiple paths to the same owner.
    for mask in 0u16..(1 << 10) {
        let mut edge = 0;
        let parents: Vec<Vec<usize>> = (0..5)
            .map(|child| {
                (0..child)
                    .filter(|_| {
                        let present = mask & (1 << edge) != 0;
                        edge += 1;
                        present
                    })
                    .collect()
            })
            .collect();
        let members = fixture(&parents);
        assert_eq!(
            membership::aggregates(&members, 5).unwrap(),
            reference(&members, 5)
        );
    }
    let members = fixture(
        &(0..64)
            .map(|index| if index == 0 { vec![] } else { vec![index - 1] })
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        membership::aggregates(&members, 64).unwrap(),
        reference(&members, 64)
    );
    assert!(matches!(
        membership::aggregates(&members, 63),
        Err(Error::Rejected(Reject::ExceededMaximumAncestorsCount))
    ));
    assert!(
        membership::aggregates(&Members::new(), 0)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn complete_totals_preserve_zero_limit_missing_parent_phase_and_cycle_rejections() {
    let mut members = fixture(&[vec![]]);
    let hash = members.keys().next().unwrap().clone();
    assert!(matches!(
        membership::aggregates(&members, 0),
        Err(Error::Rejected(Reject::ExceededMaximumAncestorsCount))
    ));
    change(&mut members, &hash, |value| {
        value.parents.insert(Byte32::new([0xff; 32]));
    });
    assert!(matches!(
        membership::aggregates(&members, 1),
        Err(Error::Rejected(Reject::ExceededMaximumAncestorsCount))
    ));
    assert!(matches!(
        membership::aggregates(&members, 2),
        Err(Error::Stale)
    ));
    change(&mut members, &hash, |value| {
        value.parents = BTreeSet::from([hash.clone()])
    });
    assert!(matches!(
        membership::aggregates(&members, 0),
        Err(Error::Rejected(Reject::Invalidated(_)))
    ));
    assert!(matches!(
        membership::aggregates(&members, 2),
        Err(Error::Rejected(Reject::Invalidated(_)))
    ));
    let owner = members.get(&hash).unwrap().with_phase(Phase::Resolve);
    members.insert(hash, owner);
    assert!(matches!(
        membership::aggregates(&members, 2),
        Err(Error::Stale)
    ));
}

#[test]
fn complete_totals_keep_wide_fees_and_reject_byte_or_cycle_overflow() {
    let members = fixture(&[vec![], vec![0]]);
    let parent = members
        .values()
        .find(|owner| owner.accepted().unwrap().parents.is_empty())
        .unwrap()
        .hash();
    let mut fees = members.clone();
    let hashes: Vec<_> = fees.keys().cloned().collect();
    for hash in hashes {
        change(&mut fees, &hash, |value| {
            value.fee = Capacity::shannons(u64::MAX)
        });
    }
    let totals = membership::aggregates(&fees, 2).unwrap();
    assert_eq!(totals, reference(&fees, 2));
    assert_eq!(totals[&parent].1.fee, u128::from(u64::MAX) * 2);
    assert_eq!(totals[&parent].1.fee(), Capacity::shannons(u64::MAX));
    let mut bytes = members.clone();
    change(&mut bytes, &parent, |value| value.size = usize::MAX);
    assert!(matches!(
        membership::aggregates(&bytes, 2),
        Err(Error::Rejected(Reject::Full(_)))
    ));
    let mut cycles = members;
    change(&mut cycles, &parent, |value| value.cycles = u64::MAX);
    assert!(matches!(
        membership::aggregates(&cycles, 2),
        Err(Error::Rejected(Reject::Full(_)))
    ));
}
