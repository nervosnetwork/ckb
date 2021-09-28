use numext_fixed_uint::prelude::UintConvert;
use numext_fixed_uint::{u512, U256, U512};

/// The minimal difficulty that can be represented in the compact format.
pub const DIFF_TWO: u32 = 0x2080_0000;

const ONE: U256 = U256::one();
// ONE << 256
const HSPACE: U512 = u512!("0x10000000000000000000000000000000000000000000000000000000000000000");

fn target_to_difficulty(target: &U256) -> U256 {
    if target == &ONE {
        U256::max_value()
    } else {
        let (target, _): (U512, bool) = target.convert_into();
        (HSPACE / target).convert_into().0
    }
}

fn difficulty_to_target(difficulty: &U256) -> U256 {
    if difficulty == &ONE {
        U256::max_value()
    } else {
        let (difficulty, _): (U512, bool) = difficulty.convert_into();
        (HSPACE / difficulty).convert_into().0
    }
}
/**
* the original nBits implementation inherits properties from a signed data class,
* allowing the target threshold to be negative if the high bit of the significand is set.
* This is useless—the header hash is treated as an unsigned number,
* so it can never be equal to or lower than a negative target threshold.
*
*
* The "compact" format is a representation of a whole
* number N using an unsigned 32bit number similar to a
* floating point format.
* The most significant 8 bits are the unsigned exponent of base 256.
* This exponent can be thought of as "number of bytes of N".
* The lower 24 bits are the mantissa.
* N = mantissa * 256^(exponent-3)
*/
fn get_low64(target: &U256) -> u64 {
    target.0[0]
}

/// Converts PoW target into compact format of difficulty.
pub fn target_to_compact(target: U256) -> u32 {
    let bits = 256 - target.leading_zeros();
    let exponent = u64::from((bits + 7) / 8);
    let mut compact = if exponent <= 3 {
        get_low64(&target) << (8 * (3 - exponent))
    } else {
        get_low64(&(target >> (8 * (exponent - 3))))
    };

    compact |= exponent << 24;
    compact as u32
}

/// Converts compact format of difficulty to PoW target.
pub fn compact_to_target(compact: u32) -> (U256, bool) {
    let exponent = compact >> 24;
    let mut mantissa = U256::from(compact & 0x00ff_ffff);

    let mut ret;
    if exponent <= 3 {
        mantissa >>= 8 * (3 - exponent);
        ret = mantissa.clone();
    } else {
        ret = mantissa.clone();
        ret <<= 8 * (exponent - 3);
    }

    let overflow = !mantissa.is_zero() && (exponent > 32);
    (ret, overflow)
}

/// Converts compact format of difficulty to the decoded difficulty.
pub fn compact_to_difficulty(compact: u32) -> U256 {
    let (target, overflow) = compact_to_target(compact);
    if target.is_zero() || overflow {
        return U256::zero();
    }
    target_to_difficulty(&target)
}

/// Converts difficulty into the compact format.
pub fn difficulty_to_compact(difficulty: U256) -> u32 {
    let target = difficulty_to_target(&difficulty);
    target_to_compact(target)
}
