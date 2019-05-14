use ckb_core::extras::{DaoStats, EpochExt};
use ckb_core::transaction::CellOutput;
use ckb_core::{BlockNumber, Capacity};
use failure::Error as FailureError;
use occupied_capacity::OccupiedCapacity;
use std::cmp::max;

pub fn calculate_dao_data(
    parent_block_number: BlockNumber,
    parent_block_epoch_ext: &EpochExt,
    parent_dao_stats: &DaoStats,
    secondary_epoch_reward: Capacity,
) -> Result<(u64, Capacity), FailureError> {
    let parent_c = Capacity::shannons(parent_dao_stats.accumulated_capacity);
    let parent_g2 = calculate_g2(
        parent_block_number,
        parent_block_epoch_ext,
        secondary_epoch_reward,
    )?;
    let parent_g = parent_block_epoch_ext
        .block_reward(parent_block_number)?
        .safe_add(parent_g2)?;
    let current_c = parent_c.safe_add(parent_g)?;

    let parent_ar = parent_dao_stats.accumulated_rate;
    let current_ar = u128::from(parent_ar) * u128::from((parent_c.safe_add(parent_g2)?).as_u64())
        / (max(u128::from(parent_c.as_u64()), 1));

    Ok((current_ar as u64, current_c))
}

pub fn calculate_maximum_withdraw(
    output: &CellOutput,
    deposit_dao_stats: &DaoStats,
    withdraw_dao_stats: &DaoStats,
) -> Result<Capacity, FailureError> {
    let occupied_capacity = output.occupied_capacity()?;
    let counted_capacity = output.capacity.safe_sub(occupied_capacity)?;

    let withdraw_counted_capacity = u128::from(counted_capacity.as_u64())
        * u128::from(withdraw_dao_stats.accumulated_rate)
        / u128::from(deposit_dao_stats.accumulated_rate);

    let withdraw_capacity =
        Capacity::shannons(withdraw_counted_capacity as u64).safe_add(occupied_capacity)?;
    Ok(withdraw_capacity)
}

fn calculate_g2(
    block_number: BlockNumber,
    current_epoch_ext: &EpochExt,
    secondary_epoch_reward: Capacity,
) -> Result<Capacity, FailureError> {
    let epoch_length = current_epoch_ext.length();
    let mut g2 = Capacity::shannons(secondary_epoch_reward.as_u64() / epoch_length);
    if current_epoch_ext.start_number() == block_number {
        g2 = g2.safe_add(Capacity::shannons(
            secondary_epoch_reward.as_u64() % epoch_length,
        ))?;
    }
    Ok(g2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_core::script::Script;
    use ckb_core::{capacity_bytes, Bytes};
    use numext_fixed_hash::{h256, H256};
    use numext_fixed_uint::U256;

    #[test]
    fn check_dao_data_calculation() {
        let parent_dao_stats = DaoStats {
            accumulated_rate: 10_000_000_000_123_456,
            accumulated_capacity: 500_000_000_123_000,
        };
        let parent_epoch_ext = EpochExt::new(
            12345,
            Capacity::shannons(50_000_000_000),
            Capacity::shannons(1_000_128),
            h256!("0x1"),
            12340,
            2000,
            U256::from(1u64),
        );

        let result = calculate_dao_data(
            12345,
            &parent_epoch_ext,
            &parent_dao_stats,
            Capacity::shannons(123_456),
        )
        .unwrap();
        assert_eq!(
            result,
            (
                10_000_000_000_124_675,
                Capacity::shannons(500_050_000_123_061)
            )
        );
    }

    #[test]
    fn check_initial_dao_data_calculation() {
        let parent_dao_stats = DaoStats {
            accumulated_rate: 10_000_000_000_000_000,
            accumulated_capacity: 50_000_000_000_000,
        };
        let parent_epoch_ext = EpochExt::new(
            0,
            Capacity::shannons(50_000_000_000),
            Capacity::shannons(1_000_128),
            h256!("0x1"),
            0,
            2000,
            U256::from(1u64),
        );

        let result = calculate_dao_data(
            0,
            &parent_epoch_ext,
            &parent_dao_stats,
            Capacity::shannons(123_456),
        )
        .unwrap();
        assert_eq!(
            result,
            (
                10_000_000_000_303_400,
                Capacity::shannons(50_050_001_001_645)
            )
        );
    }

    #[test]
    fn check_first_epoch_block_dao_data_calculation() {
        let parent_dao_stats = DaoStats {
            accumulated_rate: 10_000_000_000_123_456,
            accumulated_capacity: 500_000_000_123_000,
        };
        let parent_epoch_ext = EpochExt::new(
            12345,
            Capacity::shannons(50_000_000_000),
            Capacity::shannons(1_000_128),
            h256!("0x1"),
            12340,
            2000,
            U256::from(1u64),
        );

        let result = calculate_dao_data(
            12340,
            &parent_epoch_ext,
            &parent_dao_stats,
            Capacity::shannons(123_456),
        )
        .unwrap();
        assert_eq!(
            result,
            (
                10_000_000_000_153_795,
                Capacity::shannons(500_050_001_124_645)
            )
        );
    }

    #[test]
    fn check_dao_data_calculation_works_on_zero_initial_capacity() {
        let parent_dao_stats = DaoStats {
            accumulated_rate: 10_000_000_000_123_456,
            accumulated_capacity: 0,
        };
        let parent_epoch_ext = EpochExt::new(
            10,
            Capacity::shannons(50_000_000_000),
            Capacity::shannons(1_000_128),
            h256!("0x1"),
            1,
            2000,
            U256::from(1u64),
        );

        let result = calculate_dao_data(
            1,
            &parent_epoch_ext,
            &parent_dao_stats,
            Capacity::shannons(123_456),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn check_dao_data_calculation_overflows() {
        let parent_dao_stats = DaoStats {
            accumulated_rate: 10_000_000_000_123_456,
            accumulated_capacity: 18_446_744_073_709_000_000,
        };
        let parent_epoch_ext = EpochExt::new(
            12345,
            Capacity::shannons(50_000_000_000),
            Capacity::shannons(1_000_128),
            h256!("0x1"),
            12340,
            2000,
            U256::from(1u64),
        );

        let result = calculate_dao_data(
            12340,
            &parent_epoch_ext,
            &parent_dao_stats,
            Capacity::shannons(123_456),
        );
        assert!(result.is_err());
    }

    #[test]
    fn check_withdraw_calculation() {
        let deposit_dao_stats = DaoStats {
            accumulated_rate: 10_000_000_000_123_456,
            ..Default::default()
        };
        let withdraw_dao_stats = DaoStats {
            accumulated_rate: 10_000_000_001_123_456,
            ..Default::default()
        };
        let output = CellOutput::new(
            capacity_bytes!(1000000),
            Bytes::from(vec![1; 10]),
            Script::default(),
            None,
        );
        let result = calculate_maximum_withdraw(&output, &deposit_dao_stats, &withdraw_dao_stats);
        assert_eq!(result.unwrap(), Capacity::shannons(100_000_000_009_999));
    }

    #[test]
    fn check_withdraw_calculation_overflows() {
        let deposit_dao_stats = DaoStats {
            accumulated_rate: 10_000_000_000_123_456,
            ..Default::default()
        };
        let withdraw_dao_stats = DaoStats {
            accumulated_rate: 10_000_000_001_123_456,
            ..Default::default()
        };
        let output = CellOutput::new(
            Capacity::shannons(18_446_744_073_709_550_000),
            Bytes::from(vec![1; 10]),
            Script::default(),
            None,
        );
        let result = calculate_maximum_withdraw(&output, &deposit_dao_stats, &withdraw_dao_stats);
        assert!(result.is_err());
    }
}
