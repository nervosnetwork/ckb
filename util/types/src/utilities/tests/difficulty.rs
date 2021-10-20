// #[allow(clippy::unreadable_literal, clippy::cognitive_complexity)]

use numext_fixed_uint::{u256, U256};
use proptest::prelude::*;

use crate::utilities::{
    compact_to_difficulty, compact_to_target, difficulty_to_compact, target_to_compact,
};

#[test]
fn test_extremes() {
    {
        let compact_when_target_is_one = 0x1010000;

        let compact = target_to_compact(U256::one());
        assert_eq!(compact, compact_when_target_is_one);

        let difficulty = compact_to_difficulty(compact);
        assert_eq!(difficulty, U256::max_value());

        let compact_from_difficulty = difficulty_to_compact(difficulty);
        assert_eq!(compact, compact_from_difficulty);
    }
    {
        let compact_when_target_is_max = 0x20ffffff;

        let compact = target_to_compact(U256::max_value());
        assert_eq!(compact, compact_when_target_is_max);

        let difficulty = compact_to_difficulty(compact);
        assert_eq!(difficulty, U256::one());

        let compact_from_difficulty = difficulty_to_compact(difficulty);
        assert_eq!(compact, compact_from_difficulty);
    }
    {
        let compact_cause_overflow = 0xff123456;

        let (_, overflow) = compact_to_target(compact_cause_overflow);
        assert_eq!(overflow, true);

        let difficulty = compact_to_difficulty(compact_cause_overflow);
        assert_eq!(difficulty, U256::zero());
    }
}

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
