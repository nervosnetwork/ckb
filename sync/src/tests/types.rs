use ckb_types::{
    core::{BlockNumber, EpochNumberWithFraction, HeaderBuilder},
    packed::Byte32,
    prelude::*,
    U256,
};
use rand::{thread_rng, Rng};
use std::collections::{BTreeMap, HashMap};

use crate::types::{HeaderView, TtlFilter, FILTER_TTL};

const SKIPLIST_LENGTH: u64 = 10_000;

#[test]
fn test_get_ancestor_use_skip_list() {
    let mut header_map: HashMap<Byte32, HeaderView> = HashMap::default();
    let mut hashes: BTreeMap<BlockNumber, Byte32> = BTreeMap::default();

    let mut parent_hash = None;
    for number in 0..SKIPLIST_LENGTH {
        let mut header_builder = HeaderBuilder::default()
            .number(number.pack())
            .epoch(EpochNumberWithFraction::new(number / 1000, number % 1000, 1000).pack());
        if let Some(parent_hash) = parent_hash.take() {
            header_builder = header_builder.parent_hash(parent_hash);
        }
        let header = header_builder.build();
        hashes.insert(number, header.hash());
        parent_hash = Some(header.hash());

        let mut view = HeaderView::new(header, U256::zero());
        view.build_skip(0, |hash, _| header_map.get(hash).cloned(), |_, _| None);
        header_map.insert(view.hash(), view);
    }

    for (number, hash) in &hashes {
        if *number > 0 {
            let skip_view = header_map
                .get(hash)
                .and_then(|view| header_map.get(view.skip_hash.as_ref().unwrap()))
                .unwrap();
            assert_eq!(
                Some(skip_view.hash()).as_ref(),
                hashes.get(&skip_view.number())
            );
            assert!(skip_view.number() < *number);
        } else {
            assert!(header_map[hash].skip_hash.is_none());
        }
    }

    let mut rng = thread_rng();
    let a_to_b = |a, b, limit| {
        let mut count = 0;
        let header = header_map
            .get(&hashes[&a])
            .cloned()
            .unwrap()
            .get_ancestor(
                0,
                b,
                |hash, _| {
                    count += 1;
                    header_map.get(hash).cloned()
                },
                |_, _| None,
            )
            .unwrap();

        // Search must finished in <limit> steps
        assert!(count <= limit);

        header
    };
    for _ in 0..100 {
        let from: u64 = rng.gen_range(0, SKIPLIST_LENGTH);
        let to: u64 = rng.gen_range(0, from + 1);
        let view_from = &header_map[&hashes[&from]];
        let view_to = &header_map[&hashes[&to]];
        let view_0 = &header_map[&hashes[&0]];

        let found_from_header = a_to_b(SKIPLIST_LENGTH - 1, from, 120);
        assert_eq!(found_from_header.hash(), view_from.hash());

        let found_to_header = a_to_b(from, to, 120);
        assert_eq!(found_to_header.hash(), view_to.hash());

        let found_0_header = a_to_b(from, 0, 120);
        assert_eq!(found_0_header.hash(), view_0.hash());
    }
}

#[test]
fn ttl_filter() {
    let mut filter = TtlFilter::default();
    let mut _faketime_guard = ckb_systemtime::faketime();
    _faketime_guard.set_faketime(0);
    filter.insert(1);
    let mut _faketime_guard = ckb_systemtime::faketime();
    _faketime_guard.set_faketime(FILTER_TTL * 1000 + 1000);
    filter.insert(2);
    filter.remove_expired();
    assert!(!filter.contains(&1));
    assert!(filter.contains(&2));
}
