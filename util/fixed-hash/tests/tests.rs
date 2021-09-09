use ckb_fixed_hash::{h160, h256, h512, h520};
use std::str::FromStr;

macro_rules! assert_hashes {
    ($expected:ident, $from_trimmed_hex_str:ident, $from_full_hex_str:ident) => {
        assert_eq!(
            $expected, $from_trimmed_hex_str,
            "the value from trimmed hex str should be \n{:#x} but got \n{:#x}\n",
            $from_trimmed_hex_str, $expected
        );
        assert_hashes!($expected, $from_full_hex_str);
    };
    ($expected:ident, $from_full_hex_str:ident) => {
        assert_eq!(
            $expected, $from_full_hex_str,
            "the value from full hex str should be \n{:#x} but got \n{:#x}\n",
            $from_full_hex_str, $expected
        );
    };
}

macro_rules! add_tests {
    ($test_name:ident, $macro:ident, $type:ident, $bytes_size:literal, $zeros_str:literal,
     $only_lowest_bit_is_one_str:literal, $only_highest_bit_is_one_str:literal $(,)?) => {
        #[test]
        fn $test_name() {
            {
                let zeros_str = format!("{:0>width$}", 0, width = $bytes_size * 2);
                let from_str = ckb_fixed_hash::$type::from_str(&zeros_str).unwrap();
                let from_macro = $macro!($zeros_str);
                assert_eq!(from_str, from_macro);
            }
            {
                let expected = ckb_fixed_hash::$type([0; $bytes_size]);
                let from_trimmed = $macro!("0x0");
                let from_full = $macro!($zeros_str);
                assert_hashes!(expected, from_trimmed, from_full);
            }
            {
                let mut inner = [0; $bytes_size];
                inner[$bytes_size - 1] = 1;
                let expected = ckb_fixed_hash::$type(inner);
                let from_trimmed = $macro!("0x1");
                let from_full = $macro!($only_lowest_bit_is_one_str);
                assert_hashes!(expected, from_trimmed, from_full);
            }
            {
                let mut inner = [0; $bytes_size];
                inner[0] = 0b1000_0000;
                let expected = ckb_fixed_hash::$type(inner);
                let from_full = $macro!($only_highest_bit_is_one_str);
                assert_hashes!(expected, from_full);
            }
        }
    };
}

add_tests!(
    test_h160,
    h160,
    H160,
    20,
    "0x00000000_00000000_00000000_00000000_00000000",
    "0x00000000_00000000_00000000_00000000_00000001",
    "0x80000000_00000000_00000000_00000000_00000000",
);
add_tests!(
    test_h256,
    h256,
    H256,
    32,
    "0x00000000_00000000_00000000_00000000_00000000_00000000_00000000_00000000",
    "0x00000000_00000000_00000000_00000000_00000000_00000000_00000000_00000001",
    "0x80000000_00000000_00000000_00000000_00000000_00000000_00000000_00000000",
);
add_tests!(
    test_h512,
    h512,
    H512,
    64,
    "0x00000000_00000000_00000000_00000000_00000000_00000000_00000000_00000000\
       00000000_00000000_00000000_00000000_00000000_00000000_00000000_00000000",
    "0x00000000_00000000_00000000_00000000_00000000_00000000_00000000_00000000\
       00000000_00000000_00000000_00000000_00000000_00000000_00000000_00000001",
    "0x80000000_00000000_00000000_00000000_00000000_00000000_00000000_00000000\
       00000000_00000000_00000000_00000000_00000000_00000000_00000000_00000000",
);
add_tests!(
    test_h520,
    h520,
    H520,
    65,
    "0x00000000_00000000_00000000_00000000_00000000_00000000_00000000_00000000\
       00000000_00000000_00000000_00000000_00000000_00000000_00000000_00000000_00",
    "0x00000000_00000000_00000000_00000000_00000000_00000000_00000000_00000000\
       00000000_00000000_00000000_00000000_00000000_00000000_00000000_00000000_01",
    "0x80000000_00000000_00000000_00000000_00000000_00000000_00000000_00000000\
       00000000_00000000_00000000_00000000_00000000_00000000_00000000_00000000_00",
);
