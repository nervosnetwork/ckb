use crate::component::tests::util::build_tx;
use crate::component::{
    entry::TxEntry,
    pool_map::{PoolMap, Status},
};
use ckb_types::core::{Capacity, Cycle, FeeRate};

#[test]
fn test_estimate_fee_rate() {
    let mut pool = PoolMap::new(1000);
    for i in 0..1024 {
        let tx = build_tx(vec![(&Default::default(), i as u32)], 1);
        let entry = TxEntry::dummy_resolve(tx, i + 1, Capacity::shannons(i + 1), 1000);
        pool.add_entry(entry, Status::Pending).unwrap();
    }

    assert_eq!(
        FeeRate::from_u64(42),
        pool.estimate_fee_rate(1, usize::MAX, Cycle::MAX, FeeRate::from_u64(42))
    );

    assert_eq!(
        FeeRate::from_u64(1024),
        pool.estimate_fee_rate(1, 1000, Cycle::MAX, FeeRate::from_u64(1))
    );
    assert_eq!(
        FeeRate::from_u64(1023),
        pool.estimate_fee_rate(1, 2000, Cycle::MAX, FeeRate::from_u64(1))
    );
    assert_eq!(
        FeeRate::from_u64(1016),
        pool.estimate_fee_rate(2, 5000, Cycle::MAX, FeeRate::from_u64(1))
    );

    assert_eq!(
        FeeRate::from_u64(1024),
        pool.estimate_fee_rate(1, usize::MAX, 1, FeeRate::from_u64(1))
    );
    assert_eq!(
        FeeRate::from_u64(1023),
        pool.estimate_fee_rate(1, usize::MAX, 2047, FeeRate::from_u64(1))
    );
    assert_eq!(
        FeeRate::from_u64(1015),
        pool.estimate_fee_rate(2, usize::MAX, 5110, FeeRate::from_u64(1))
    );

    assert_eq!(
        FeeRate::from_u64(624),
        pool.estimate_fee_rate(100, 5000, 5110, FeeRate::from_u64(1))
    );
    assert_eq!(
        FeeRate::from_u64(1),
        pool.estimate_fee_rate(1000, 5000, 5110, FeeRate::from_u64(1))
    );
}
