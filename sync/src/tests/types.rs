use ckb_shared::types::HeaderIndexView;
use ckb_types::{
    U256,
    core::{BlockNumber, EpochNumberWithFraction, HeaderBuilder},
    packed::Byte32,
};
use rand::{Rng, thread_rng};
use std::{
    collections::{BTreeMap, HashMap},
    sync::atomic::{
        AtomicUsize,
        Ordering::{Acquire, SeqCst},
    },
};

use crate::types::{FILTER_TTL, TtlFilter};

const SKIPLIST_LENGTH: u64 = 10_000;

#[test]
fn test_get_ancestor() {
    let mut header_map: HashMap<Byte32, HeaderIndexView> = HashMap::default();
    let mut hashes: BTreeMap<BlockNumber, Byte32> = BTreeMap::default();

    let mut parent_hash = None;
    for number in 0..SKIPLIST_LENGTH {
        let mut header_builder =
            HeaderBuilder::default()
                .number(number)
                .epoch(EpochNumberWithFraction::new(
                    number / 1000,
                    number % 1000,
                    1000,
                ));
        if let Some(parent_hash) = parent_hash.take() {
            header_builder = header_builder.parent_hash(parent_hash);
        }
        let header = header_builder.build();
        hashes.insert(number, header.hash());
        parent_hash = Some(header.hash());

        let view: HeaderIndexView = (header, U256::zero()).into();
        header_map.insert(view.hash(), view);
    }

    let mut rng = thread_rng();
    for _ in 0..100 {
        let from: u64 = rng.gen_range(0..SKIPLIST_LENGTH);
        let to: u64 = rng.gen_range(0..=from);
        let view_from = &header_map[&hashes[&from]];
        let view_to = &header_map[&hashes[&to]];
        let view_0 = &header_map[&hashes[&0]];

        let found_from_header = header_map
            .get(&hashes[&(SKIPLIST_LENGTH - 1)])
            .cloned()
            .unwrap()
            .get_ancestor(
                0,
                from,
                |hash, _| header_map.get(hash).cloned(),
                |_, _| None,
            )
            .unwrap();
        assert_eq!(found_from_header.hash(), view_from.hash());

        let found_to_header = header_map
            .get(&hashes[&from])
            .cloned()
            .unwrap()
            .get_ancestor(0, to, |hash, _| header_map.get(hash).cloned(), |_, _| None)
            .unwrap();
        assert_eq!(found_to_header.hash(), view_to.hash());

        let found_0_header = header_map
            .get(&hashes[&from])
            .cloned()
            .unwrap()
            .get_ancestor(0, 0, |hash, _| header_map.get(hash).cloned(), |_, _| None)
            .unwrap();
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
