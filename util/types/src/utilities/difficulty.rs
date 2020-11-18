use numext_fixed_uint::prelude::UintConvert;
use numext_fixed_uint::{u512, U256, U512};

/// TODO(doc): @doitian
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

/// TODO(doc): @doitian
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

/// TODO(doc): @doitian
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

/// TODO(doc): @doitian
pub fn compact_to_difficulty(compact: u32) -> U256 {
    let (target, overflow) = compact_to_target(compact);
    if target.is_zero() || overflow {
        return U256::zero();
    }
    target_to_difficulty(&target)
}

/// TODO(doc): @doitian
pub fn difficulty_to_compact(difficulty: U256) -> u32 {
    let target = difficulty_to_target(&difficulty);
    target_to_compact(target)
}

#[cfg(test)]
#[allow(clippy::unreadable_literal, clippy::cognitive_complexity)]
mod tests {
    use super::*;
    use numext_fixed_uint::{u256, U256};
    use proptest::prelude::*;

    fn _test_compact_overflowing(target: U256) {
        let compact = target_to_compact(target);
        let (_, overflow) = compact_to_target(compact);
        assert_eq!(overflow, false, "should not overflow");
    }

    #[test]
    fn test_compact_convert() {
        let (ret, overflow) = compact_to_target(0);
        let compact = target_to_compact(u256!("0x0"));
        assert_eq!(ret, u256!("0x0"));
        assert_eq!(overflow, false);
        assert_eq!(compact, 0);

        let (ret, overflow) = compact_to_target(0x123456);
        assert_eq!(ret, u256!("0x0"));
        assert_eq!(overflow, false);

        let (ret, overflow) = compact_to_target(0x1003456);
        assert_eq!(ret, u256!("0x0"));
        assert_eq!(overflow, false);

        let (ret, overflow) = compact_to_target(0x2000056);
        assert_eq!(ret, u256!("0x0"));
        assert_eq!(overflow, false);

        let (ret, overflow) = compact_to_target(0x3000000);
        assert_eq!(ret, u256!("0x0"));
        assert_eq!(overflow, false);

        let (ret, overflow) = compact_to_target(0x4000000);
        assert_eq!(ret, u256!("0x0"));
        assert_eq!(overflow, false);

        let (ret, overflow) = compact_to_target(0x923456);
        assert_eq!(ret, u256!("0x0"));
        assert_eq!(overflow, false);

        let (ret, overflow) = compact_to_target(0x1803456);
        assert_eq!(ret, u256!("0x80"));
        assert_eq!(overflow, false);

        let (ret, overflow) = compact_to_target(0x2800056);
        assert_eq!(ret, u256!("0x8000"));
        assert_eq!(overflow, false);

        let (ret, overflow) = compact_to_target(0x3800000);
        assert_eq!(ret, u256!("0x800000"));
        assert_eq!(overflow, false);

        let (ret, overflow) = compact_to_target(0x4800000);
        assert_eq!(ret, u256!("0x80000000"));
        assert_eq!(overflow, false);

        let (ret, overflow) = compact_to_target(0x1020000);
        let compact = target_to_compact(u256!("0x2"));
        assert_eq!(ret, u256!("0x2"));
        assert_eq!(overflow, false);
        assert_eq!(compact, 0x1020000);

        let (ret, overflow) = compact_to_target(0x1fedcba);
        let compact = target_to_compact(u256!("0xfe"));
        assert_eq!(ret, u256!("0xfe"));
        assert_eq!(overflow, false);
        assert_eq!(compact, 0x1fe0000);

        let (ret, overflow) = compact_to_target(0x2123456);
        let compact = target_to_compact(u256!("0x1234"));
        assert_eq!(ret, u256!("0x1234"));
        assert_eq!(overflow, false);
        assert_eq!(compact, 0x2123400);

        let (ret, overflow) = compact_to_target(0x3123456);
        assert_eq!(ret, u256!("0x123456"));
        let compact = target_to_compact(u256!("0x123456"));
        assert_eq!(overflow, false);
        assert_eq!(compact, 0x3123456);

        let (ret, overflow) = compact_to_target(0x4123456);
        assert_eq!(ret, u256!("0x12345600"));
        assert_eq!(overflow, false);
        let compact = target_to_compact(u256!("0x12345600"));
        assert_eq!(compact, 0x4123456);

        let (ret, overflow) = compact_to_target(0x4923456);
        assert_eq!(ret, u256!("0x92345600"));
        assert_eq!(overflow, false);
        let compact = target_to_compact(u256!("0x92345600"));
        assert_eq!(compact, 0x4923456);

        let (ret, overflow) = compact_to_target(0x4923400);
        assert_eq!(ret, u256!("0x92340000"));
        assert_eq!(overflow, false);
        let compact = target_to_compact(u256!("0x92340000"));
        assert_eq!(compact, 0x4923400);

        let (ret, overflow) = compact_to_target(0x20123456);
        assert_eq!(
            ret,
            u256!("0x1234560000000000000000000000000000000000000000000000000000000000")
        );
        assert_eq!(overflow, false);
        let compact = target_to_compact(u256!(
            "0x1234560000000000000000000000000000000000000000000000000000000000"
        ));
        assert_eq!(compact, 0x20123456);

        let (_, overflow) = compact_to_target(0xff123456);
        assert_eq!(overflow, true);
    }

    #[test]
    fn test_compact_overflowing2() {
        _test_compact_overflowing(U256::max_value());

        let (_, overflow) = compact_to_target(0x21000001);
        assert_eq!(overflow, true, "should overflow");
        let (_, overflow) = compact_to_target(0x22000001);
        assert_eq!(overflow, true, "should overflow");
        let (_, overflow) = compact_to_target(0x23000001);
        assert_eq!(overflow, true, "should overflow");
    }

    proptest! {
        #[test]
        fn test_compact_overflowing1(s in "[0-9a-f]{64}") {
            let _  = U256::from_hex_str(&s).map(|target| {
                _test_compact_overflowing(target)
            });
        }
    }
}
