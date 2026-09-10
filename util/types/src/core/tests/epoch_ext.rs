use crate::core::{Capacity, CapacityError, EpochExt, EpochNumberWithFraction};

fn edge_epoch() -> EpochExt {
    EpochExt::new_builder()
        .start_number(u64::MAX - 1)
        .length(3)
        .base_block_reward(Capacity::shannons(42))
        .remainder_reward(Capacity::shannons(2))
        .build()
}

#[test]
fn test_block_reward_does_not_overflow_remainder_boundary() {
    let epoch = edge_epoch();

    assert_eq!(
        epoch.block_reward(u64::MAX).unwrap(),
        Capacity::shannons(43)
    );
    assert_eq!(
        epoch.block_reward(u64::MAX - 2).unwrap(),
        Capacity::shannons(42)
    );
}

#[test]
fn test_secondary_block_issuance_does_not_overflow_remainder_boundary() {
    let epoch = edge_epoch();

    assert_eq!(
        epoch
            .secondary_block_issuance(u64::MAX, Capacity::shannons(5))
            .unwrap(),
        Capacity::shannons(2)
    );
}

#[test]
fn test_secondary_block_issuance_rejects_zero_length() {
    let epoch = EpochExt::new_builder().length(0).build();

    assert_eq!(
        epoch
            .secondary_block_issuance(0, Capacity::shannons(1))
            .unwrap_err(),
        CapacityError::Overflow
    );
}

#[test]
fn test_checked_primary_reward_rejects_overflow() {
    let epoch = EpochExt::new_builder()
        .length(2)
        .base_block_reward(Capacity::shannons(u64::MAX))
        .remainder_reward(Capacity::one())
        .build();

    assert_eq!(
        epoch.checked_primary_reward().unwrap_err(),
        CapacityError::Overflow
    );
}

#[test]
fn test_try_set_primary_reward_rejects_zero_length() {
    let mut epoch = EpochExt::new_builder().build();

    assert_eq!(
        epoch.try_set_primary_reward(Capacity::one()).unwrap_err(),
        CapacityError::Overflow
    );
}

#[test]
fn test_minimum_epoch_number_after_n_blocks_handles_large_n() {
    let epoch = EpochNumberWithFraction::new(1, 1, 10);

    assert_eq!(epoch.minimum_epoch_number_after_n_blocks(u64::MAX), 2);
}
