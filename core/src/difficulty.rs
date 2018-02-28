use bigint::{H256, U256};
use block::Header;
use global::*;
use std::cmp;

// new_diff = parent_diff +
//            parent_diff // DIFFICULTY_BOUND_DIVISOR *
//            max(THRESHOLD - (block_timestamp - parent_timestamp) // INCREMENT_DIVISOR, -LIMIT)
pub fn calculate_difficulty(header: &Header, parent: &Header) -> U256 {
    let diff_bound_div = U256::from(DIFFICULTY_BOUND_DIVISOR);
    let diff_inc = (header.timestamp - parent.timestamp) / INCREMENT_DIVISOR;
    if diff_inc <= THRESHOLD {
        parent.difficulty + parent.difficulty / diff_bound_div * U256::from(THRESHOLD - diff_inc)
    } else {
        let multiplier: U256 = cmp::min(diff_inc - THRESHOLD, LIMIT).into();
        parent
            .difficulty
            .saturating_sub(parent.difficulty / diff_bound_div * multiplier)
    }
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
