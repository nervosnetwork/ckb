use super::*;
use ckb_types::bytes::Bytes;
use ckb_types::core::UncleBlockView;
use ckb_types::packed;
use ckb_types::prelude::Entity;

impl CandidateUncles {
    /// Remove every candidate in the test-sized container.
    pub(in crate::block_assembler) fn clear(&mut self) {
        self.map.clear();
        self.count = 0;
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

/// A full container must still accept an uncle at the *boundary*
/// height (equal to the lowest stored height) into that height's
/// existing set when it has room — instead of rejecting it while
/// evicting the whole lowest set for strictly higher heights.
#[test]
fn full_container_accepts_uncle_at_lowest_existing_height() {
    let mut container = CandidateUncles::new();

    // Fill to capacity: height 1 has room left in its set, the others
    // are full. (Test constants: MAX_CANDIDATE_UNCLES=4, MAX_PER_HEIGHT=2.)
    assert!(container.insert(uncle(1, 100)));
    assert!(container.insert(uncle(2, 101)));
    assert!(container.insert(uncle(2, 102)));
    assert!(container.insert(uncle(3, 103)));
    assert_eq!(container.len(), MAX_CANDIDATE_UNCLES);

    // Boundary height with room: accepted into the existing set (the
    // soft cap may be exceeded by one set, bounded by MAX_PER_HEIGHT).
    assert!(
        container.insert(uncle(1, 200)),
        "boundary-height uncle must be accepted into the existing set"
    );
    assert!(
        !container.insert(uncle(1, 201)),
        "but the per-height cap still holds"
    );

    // A strictly lower height is rejected.
    assert!(!container.insert(uncle(0, 202)));

    // A higher height evicts the whole lowest set to make room.
    assert!(container.insert(uncle(4, 203)));
    assert!(
        !container.values().any(|u| u.header().number() == 1),
        "the lowest height set must be evicted for the higher uncle"
    );
}

#[test]
fn rejected_high_candidate_cannot_evict_lowest_height() {
    let mut duplicate_container = CandidateUncles::new();
    let high = uncle(3, 103);
    assert!(duplicate_container.insert(uncle(1, 100)));
    assert!(duplicate_container.insert(uncle(2, 101)));
    assert!(duplicate_container.insert(uncle(2, 102)));
    assert!(duplicate_container.insert(high.clone()));
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
    assert!(full_bucket_container.insert(uncle(1, 110)));
    assert!(full_bucket_container.insert(uncle(2, 111)));
    assert!(full_bucket_container.insert(uncle(3, 112)));
    assert!(full_bucket_container.insert(uncle(3, 113)));
    assert_eq!(full_bucket_container.len(), MAX_CANDIDATE_UNCLES);
    assert!(!full_bucket_container.insert(uncle(3, 114)));
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
