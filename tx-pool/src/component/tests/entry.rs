use ckb_types::core::{Capacity, FeeRate};

use crate::component::entry::EvictKey;

#[test]
fn test_min_fee_and_weight_evict() {
    let mut result = vec![(500, 10, 30), (10, 10, 31), (100, 10, 32)]
        .into_iter()
        .map(|(fee, weight, timestamp)| EvictKey {
            fee_rate: FeeRate::calculate(Capacity::shannons(fee), weight),
            timestamp,
            descendants_count: 0,
        })
        .collect::<Vec<_>>();
    result.sort();
    assert_eq!(
        result.iter().map(|key| key.timestamp).collect::<Vec<_>>(),
        vec![31, 32, 30]
    );
}

#[test]
fn test_min_timestamp_evict() {
    let mut result = vec![(500, 10, 30), (500, 10, 31), (500, 10, 32)]
        .into_iter()
        .map(|(fee, weight, timestamp)| EvictKey {
            fee_rate: FeeRate::calculate(Capacity::shannons(fee), weight),
            timestamp,
            descendants_count: 0,
        })
        .collect::<Vec<_>>();
    result.sort();
    assert_eq!(
        result.iter().map(|key| key.timestamp).collect::<Vec<_>>(),
        vec![30, 31, 32]
    );
}

#[test]
fn test_min_weight_evict() {
    let mut result = vec![(500, 10, 30), (500, 12, 31), (500, 13, 32)]
        .into_iter()
        .map(|(fee, weight, timestamp)| EvictKey {
            fee_rate: FeeRate::calculate(Capacity::shannons(fee), weight),
            timestamp,
            descendants_count: 0,
        })
        .collect::<Vec<_>>();
    result.sort();
    assert_eq!(
        result.iter().map(|key| key.timestamp).collect::<Vec<_>>(),
        vec![32, 31, 30]
    );
}
