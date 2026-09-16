use ckb_types::core::Capacity;

use crate::component::sort_key::AncestorsScoreSortKey;

#[test]
fn minimum_score_uses_the_lower_of_transaction_and_ancestor_fee_rate() {
    let result = vec![
        (0, 0, 0, 0),
        (1, 0, 1, 0),
        (500, 10, 1000, 30),
        (10, 500, 30, 1000),
        (500, 10, 1000, 20),
        (u64::MAX, 0, u64::MAX, 0),
        (u64::MAX, 100, u64::MAX, 2000),
        (u64::MAX, u64::MAX, u64::MAX, u64::MAX),
    ]
    .into_iter()
    .map(|(fee, weight, ancestors_fee, ancestors_weight)| {
        AncestorsScoreSortKey {
            fee: Capacity::shannons(fee),
            weight,
            ancestors_fee: Capacity::shannons(ancestors_fee),
            ancestors_weight,
        }
        .min_fee_and_weight()
    })
    .collect::<Vec<_>>();

    assert_eq!(
        result,
        vec![
            (Capacity::shannons(0), 0),
            (Capacity::shannons(1), 0),
            (Capacity::shannons(1000), 30),
            (Capacity::shannons(10), 500),
            (Capacity::shannons(1000), 20),
            (Capacity::shannons(u64::MAX), 0),
            (Capacity::shannons(u64::MAX), 2000),
            (Capacity::shannons(u64::MAX), u64::MAX),
        ]
    );
}

#[test]
fn ancestor_score_order_is_deterministic_at_extreme_weights() {
    let table = vec![
        (0, 0, 0, 0),
        (1, 0, 1, 0),
        (500, 10, 1000, 30),
        (10, 500, 30, 1000),
        (500, 10, 1000, 30),
        (10, 500, 30, 1000),
        (500, 10, 1000, 20),
        (u64::MAX, 0, u64::MAX, 0),
        (u64::MAX, 100, u64::MAX, 2000),
        (u64::MAX, u64::MAX, u64::MAX, u64::MAX),
    ];
    let mut keys = table
        .iter()
        .copied()
        .map(
            |(fee, weight, ancestors_fee, ancestors_weight)| AncestorsScoreSortKey {
                fee: Capacity::shannons(fee),
                weight,
                ancestors_fee: Capacity::shannons(ancestors_fee),
                ancestors_weight,
            },
        )
        .collect::<Vec<_>>();
    keys.sort();

    let actual = keys
        .into_iter()
        .map(|key| (key.fee, key.weight, key.ancestors_fee, key.ancestors_weight))
        .collect::<Vec<_>>();
    let expected = [0, 3, 5, 9, 2, 4, 6, 8, 1, 7]
        .iter()
        .map(|&index| {
            let key = table[index];
            (
                Capacity::shannons(key.0),
                key.1,
                Capacity::shannons(key.2),
                key.3,
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(actual, expected);
}
