use crate::component::sort_key::EvictKey;
use ckb_types::{
    core::{Capacity, FeeRate},
    packed::ProposalShortId,
};

#[test]
fn eviction_order_uses_fee_then_descendants_age_and_id() {
    // Retain the fee/weight examples and cover every successive tie breaker.
    // Each row is (fee, weight, descendants, timestamp, id).
    let cases = [
        (
            vec![(500, 10, 0, 30, 1), (10, 10, 0, 31, 2), (100, 10, 0, 32, 3)],
            vec![2, 3, 1],
        ),
        (
            vec![
                (500, 10, 0, 30, 1),
                (500, 10, 0, 31, 2),
                (500, 10, 0, 32, 3),
            ],
            vec![1, 2, 3],
        ),
        (
            vec![
                (500, 10, 0, 30, 1),
                (500, 12, 0, 31, 2),
                (500, 13, 0, 32, 3),
            ],
            vec![3, 2, 1],
        ),
        (
            vec![
                (10, 10, 99, 99, 1),
                (100, 10, 0, 99, 2),
                (100, 10, 1, 30, 3),
                (100, 10, 1, 31, 4),
                (100, 10, 1, 31, 5),
                (100, 10, 2, 0, 6),
            ],
            vec![1, 2, 3, 4, 5, 6],
        ),
    ];
    for (rows, expected) in cases {
        let keys: Vec<_> = rows
            .into_iter()
            .map(|(fee, weight, descendants_count, timestamp, id)| EvictKey {
                fee_rate: FeeRate::calculate(Capacity::shannons(fee), weight),
                timestamp,
                descendants_count,
                id: ProposalShortId::new([id; ProposalShortId::TOTAL_SIZE]),
            })
            .collect();
        let expected: Vec<_> = expected
            .into_iter()
            .map(|id| ProposalShortId::new([id; ProposalShortId::TOTAL_SIZE]))
            .collect();
        let mut sorted = keys;
        sorted.sort();
        assert_eq!(
            sorted.iter().map(|key| key.id.clone()).collect::<Vec<_>>(),
            expected
        );
        // The explicit ID order fixes the rank independently of the comparator.
        for (left, a) in sorted.iter().enumerate() {
            for (right, b) in sorted.iter().enumerate() {
                assert_eq!(a.cmp(b), left.cmp(&right));
                assert_eq!(a.partial_cmp(b), Some(left.cmp(&right)));
            }
        }
    }
}
