use crate::core::{capacity_bytes, Capacity, FeeRate};

#[test]

fn test_fee_rate_calculate() {
    let fee_rate = FeeRate::calculate(capacity_bytes!(0), 0);
    assert_eq!(fee_rate.as_u64(), 0);

    let fee_rate = FeeRate::calculate(capacity_bytes!(100), 0);
    assert_eq!(fee_rate.as_u64(), 0);

    let fee_rate = FeeRate::calculate(capacity_bytes!(100), 100);
    assert_eq!(fee_rate.as_u64(), 100_000_000_000);
}
