use super::*;
use ckb_types::bytes::Bytes;
use ckb_types::core::{BlockNumber, EpochNumberWithFraction, UncleBlockView};
use ckb_types::packed;
use ckb_types::prelude::Entity;

impl CandidateUncles {
    /// Remove every candidate in the test-sized container.
    pub(in crate::block_assembler) fn clear(&mut self) {
        self.map.clear();
    }
}

fn uncle(number: BlockNumber, seed: u8) -> UncleBlockView {
    ckb_types::core::BlockBuilder::default()
        .number(number)
        .epoch(ckb_types::core::EpochNumberWithFraction::new(0, 0, 1).full_value())
        .parent_hash(packed::Byte32::new([seed; 32]))
        .build()
        .as_uncle()
}

fn fill_to_capacity(container: &mut CandidateUncles, lowest_has_room: bool) {
    let mut seed = 1u8;
    let lowest_count = if lowest_has_room {
        MAX_PER_HEIGHT - 1
    } else {
        MAX_PER_HEIGHT
    };
    for _ in 0..lowest_count {
        assert!(container.insert(uncle(1, seed)));
        seed = seed.checked_add(1).expect("test seed fits u8");
    }
    let mut height = 2;
    while container.len() < MAX_CANDIDATE_UNCLES {
        let count = MAX_PER_HEIGHT.min(MAX_CANDIDATE_UNCLES - container.len());
        for _ in 0..count {
            assert!(container.insert(uncle(height, seed)));
            seed = seed.checked_add(1).expect("test seed fits u8");
        }
        height += 1;
    }
}

#[test]
fn basic_insert_and_remove() {
    let mut candidates = CandidateUncles::new();
    let block = uncle(0, 0);
    assert!(candidates.insert(block.clone()));
    assert_eq!(candidates.len(), 1);
    assert!(!candidates.insert(block.clone()));
    assert!(candidates.remove_by_number(&block));
    assert!(candidates.is_empty());
    assert!(candidates.map.is_empty());
}

#[test]
fn keeps_the_highest_bounded_candidates() {
    let mut candidates = CandidateUncles::new();
    let blocks = (0..(MAX_CANDIDATE_UNCLES + 3))
        .map(|index| {
            let number = index as BlockNumber;
            ckb_types::core::BlockBuilder::default()
                .number(number)
                .epoch(EpochNumberWithFraction::new(
                    number / 1000,
                    number % 1000,
                    10_000,
                ))
                .build()
                .as_uncle()
        })
        .collect::<Vec<_>>();

    for block in &blocks {
        candidates.insert(block.clone());
    }
    assert_eq!(candidates.len(), MAX_CANDIDATE_UNCLES);
    assert_eq!(candidates.map.keys().next().copied(), Some(3));

    candidates.clear();
    for block in blocks.iter().rev() {
        candidates.insert(block.clone());
    }
    assert_eq!(candidates.len(), MAX_CANDIDATE_UNCLES);
    assert_eq!(candidates.map.keys().next().copied(), Some(3));
}

#[test]
fn enforces_per_height_limit() {
    let mut candidates = CandidateUncles::new();
    for index in 0..(MAX_PER_HEIGHT + 3) {
        candidates.insert(
            ckb_types::core::BlockBuilder::default()
                .timestamp(index as u64)
                .build()
                .as_uncle(),
        );
    }
    assert_eq!(candidates.map.len(), 1);
    assert_eq!(candidates.len(), MAX_PER_HEIGHT);
}

/// Global capacity is a hard residency boundary. An equal-priority candidate
/// cannot grow the cache past the bound or evict an arbitrary peer; only a
/// strictly higher-height candidate may replace the complete lowest bucket.
#[test]
fn full_container_keeps_a_hard_global_bound() {
    let mut container = CandidateUncles::new();

    // Fill to capacity: height 1 has room left in its set, the others
    // are full, using the exact production bounds.
    fill_to_capacity(&mut container, true);
    assert_eq!(container.len(), MAX_CANDIDATE_UNCLES);

    assert!(!container.insert(uncle(1, 200)));
    assert_eq!(container.len(), MAX_CANDIDATE_UNCLES);

    // A strictly lower height is rejected.
    assert!(!container.insert(uncle(0, 202)));

    // A higher height evicts the whole lowest set to make room.
    let higher = container
        .map
        .keys()
        .next_back()
        .copied()
        .expect("full container has a highest bucket")
        + 1;
    assert!(container.insert(uncle(higher, 203)));
    assert!(
        !container.values().any(|u| u.header().number() == 1),
        "the lowest height set must be evicted for the higher uncle"
    );
}

#[test]
fn rejected_high_candidate_cannot_evict_lowest_height() {
    let mut duplicate_container = CandidateUncles::new();
    fill_to_capacity(&mut duplicate_container, true);
    let high = duplicate_container
        .values()
        .find(|candidate| candidate.number() > 1)
        .cloned()
        .expect("full container has a higher-height candidate");
    assert_eq!(duplicate_container.len(), MAX_CANDIDATE_UNCLES);
    assert!(!duplicate_container.insert(high));
    assert_eq!(duplicate_container.len(), MAX_CANDIDATE_UNCLES);
    assert!(
        duplicate_container
            .values()
            .any(|candidate| candidate.number() == 1),
        "a duplicate rejection cannot mutate global candidate capacity"
    );

    let mut full_bucket_container = CandidateUncles::new();
    fill_to_capacity(&mut full_bucket_container, true);
    let full_height = full_bucket_container
        .map
        .iter()
        .find_map(|(height, candidates)| (candidates.len() == MAX_PER_HEIGHT).then_some(*height))
        .expect("full container has a saturated height bucket");
    assert_eq!(full_bucket_container.len(), MAX_CANDIDATE_UNCLES);
    assert!(!full_bucket_container.insert(uncle(full_height, 250)));
    assert_eq!(full_bucket_container.len(), MAX_CANDIDATE_UNCLES);
    assert!(
        full_bucket_container
            .values()
            .any(|candidate| candidate.number() == 1),
        "a full target bucket cannot evict another height"
    );
}

#[test]
fn accepted_uncle_detaches_from_enclosing_block_backing() {
    let outer = ckb_types::core::BlockBuilder::default()
        .uncle(uncle(1, 42))
        .transaction(
            ckb_types::core::TransactionBuilder::default()
                .witness(Bytes::from(vec![7; 64 * 1024]))
                .build(),
        )
        .build();
    let outer_data = outer.data();
    let backing = outer_data.as_slice();
    let shared = outer.uncles().into_iter().next().unwrap();
    let shared_data = shared.data();
    let shared_slice = shared_data.as_slice();
    let backing_start = backing.as_ptr() as usize;
    let backing_end = backing_start + backing.len();
    let shared_start = shared_slice.as_ptr() as usize;
    assert!(shared_start >= backing_start && shared_start < backing_end);

    let mut container = CandidateUncles::new();
    assert!(container.insert(shared));
    let stored_data = container.values().next().unwrap().data();
    let stored = stored_data.as_slice();
    let stored_start = stored.as_ptr() as usize;
    assert!(
        stored_start < backing_start || stored_start >= backing_end,
        "candidate uncle must not pin the full source block"
    );
}
