use bigint::{H256, U256};
use block::Header;
use global::*;
use std::cmp;

// new_diff = parent_diff +
//            parent_diff // DIFFICULTY_BOUND_DIVISOR *
//            max(THRESHOLD - (block_timestamp - parent_timestamp) // INCREMENT_DIVISOR, -LIMIT)
pub fn cal_difficulty(pre_header: &Header, current_time: u64) -> U256 {
    if pre_header.height == 0 {
        return U256::from(MIN_DIFFICULTY);
    }
    let diff_bound_div = U256::from(DIFFICULTY_BOUND_DIVISOR);
    let diff_inc = (current_time - pre_header.timestamp) / INCREMENT_DIVISOR;
    let target = if diff_inc <= THRESHOLD {
        pre_header.difficulty
            + pre_header.difficulty / diff_bound_div * U256::from(THRESHOLD - diff_inc)
    } else {
        let multiplier: U256 = cmp::min(diff_inc - THRESHOLD, LIMIT).into();
        pre_header
            .difficulty
            .saturating_sub(pre_header.difficulty / diff_bound_div * multiplier)
    };

    cmp::max(U256::from(MIN_DIFFICULTY), target)
}

/// f(x) = 2^256 / x
pub fn boundary_to_difficulty(boundary: &H256) -> U256 {
    let d = U256::from(*boundary);
    if d <= U256::one() {
        U256::max_value()
    } else {
        ((U256::one() << 255) / d) << 1
    }
}

#[cfg(test)]
mod tests {
    use super::{boundary_to_difficulty, cal_difficulty};
    use bigint::{H256, H520, U256};
    use block::{Header, RawHeader};
    use proof::Proof;

    fn gen_test_header(timestamp: u64, difficulty: u64) -> Header {
        let raw = RawHeader {
            pre_hash: H256::from(0),
            timestamp: timestamp,
            transactions_root: H256::from(0),
            difficulty: U256::from(difficulty),
            challenge: H256::from(0),
            proof: Proof::default(),
            height: 10,
        };

        Header::new(raw, U256::from(0), Some(H520::from(0)))
    }

    #[test]
    fn test_cal_difficulty() {
        let h1 = gen_test_header(0, 100);

        assert_eq!(cal_difficulty(&h1, 15_000), U256::from(100));
        assert_eq!(cal_difficulty(&h1, 20_000), U256::from(88));
        assert_eq!(cal_difficulty(&h1, 8_000), U256::from(112));
    }

    #[test]
    fn test_boundary_to_difficulty() {
        let h1 = H256::from(4096);
        let h2: H256 = boundary_to_difficulty(&h1).into();
        assert_eq!(boundary_to_difficulty(&h2), U256::from(4096));
    }
}
