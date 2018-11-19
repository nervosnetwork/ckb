use bigint::{H256, U256};
use global::*;
use header::Header;
use std::cmp;

// new_diff = parent_diff +
//            parent_diff // DIFFICULTY_BOUND_DIVISOR *
//            max(THRESHOLD - (block_timestamp - parent_timestamp) // INCREMENT_DIVISOR, -LIMIT)
// INCREMENT_DIVISOR: expect period ms
pub fn cal_difficulty(pre_header: &Header, current_time: u64) -> U256 {
    if pre_header.number == 0 {
        return U256::from(MIN_DIFFICULTY);
    }

    let diff_bound_div = U256::from(DIFFICULTY_BOUND_DIVISOR);
    if current_time <= pre_header.timestamp {
        error!(target: "core", "diff increment: current_time={}, pre_header.timestamp={}",
               current_time, pre_header.timestamp);
    }
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

pub fn difficulty_to_boundary(difficulty: &U256) -> H256 {
    if *difficulty <= U256::one() {
        U256::max_value().into()
    } else {
        (((U256::one() << 255) / *difficulty) << 1).into()
    }
}

#[cfg(test)]
mod tests {
    use super::{boundary_to_difficulty, cal_difficulty};
    use bigint::{H256, U256};
    use header::{Header, RawHeader, Seal};
    use std::str::FromStr;

    fn gen_test_header(timestamp: u64, difficulty: u64) -> Header {
        Header {
            raw: RawHeader {
                version: 0,
                parent_hash: H256::from(0),
                timestamp,
                txs_commit: H256::from(0),
                difficulty: U256::from(difficulty),
                number: 3500000,
                cellbase_id: H256::from(0),
                uncles_hash: H256::from(0),
            },
            seal: Seal {
                nonce: 0,
                mix_hash: H256::from(0),
            },
        }
    }

    #[test]
    fn test_cal_difficulty() {
        let timestamp = 1452838500_000u64;
        let h1 = gen_test_header(timestamp, 0x6F62EAF8D3Cu64);
        assert_eq!(
            cal_difficulty(&h1, timestamp + 20_000),
            U256::from_str("6F54FE9B74B").unwrap()
        );
        assert_eq!(
            cal_difficulty(&h1, timestamp + 5_000),
            U256::from_str("6F70D75632D").unwrap()
        );
        assert_eq!(
            cal_difficulty(&h1, timestamp + 80_000),
            U256::from_str("6F01746B3A5").unwrap()
        );
    }

    #[test]
    fn test_boundary_to_difficulty() {
        let h1 = H256::from(4096);
        let h2: H256 = boundary_to_difficulty(&h1).into();
        assert_eq!(boundary_to_difficulty(&h2), U256::from(4096));
    }
}
