use crate::{error::FromStrError, H160, H256, H512, H520};
use std::str::FromStr;

macro_rules! test_from_trimmed_str_one_byte {
    ($name:ident, $trimmed_str:expr, $index:expr, $value:expr) => {
        let result = $name::from_trimmed_str($trimmed_str).unwrap();
        let mut expected = $name::default();
        expected.0[$index] = $value;
        assert_eq!(result, expected);
    };
}

#[test]
fn from_trimmed_str() {
    test_from_trimmed_str_one_byte!(H160, "1", 19, 1);
    test_from_trimmed_str_one_byte!(H256, "1", 31, 1);
    test_from_trimmed_str_one_byte!(H512, "1", 63, 1);
    test_from_trimmed_str_one_byte!(H520, "1", 64, 1);
    test_from_trimmed_str_one_byte!(H160, "10", 19, 16);
    test_from_trimmed_str_one_byte!(H256, "10", 31, 16);
    test_from_trimmed_str_one_byte!(H512, "10", 63, 16);
    test_from_trimmed_str_one_byte!(H520, "10", 64, 16);
    test_from_trimmed_str_one_byte!(H160, "100", 18, 1);
    test_from_trimmed_str_one_byte!(H256, "100", 30, 1);
    test_from_trimmed_str_one_byte!(H512, "100", 62, 1);
    test_from_trimmed_str_one_byte!(H520, "100", 63, 1);
}

macro_rules! test_from_str_via_trimmed_str {
    ($name:ident, $trimmed_str:expr, $full_str:expr) => {
        let expected = $name::from_trimmed_str($trimmed_str).unwrap();
        let result = $name::from_str($full_str).unwrap();
        assert_eq!(result, expected);
    };
}

#[test]
fn from_str() {
    {
        let full_str = "0000000000000000000000000000000000000001";
        test_from_str_via_trimmed_str!(H160, "1", full_str);
    }
    {
        let full_str = "0000000000000000000000000000000000000000000000000000000000000001";
        test_from_str_via_trimmed_str!(H256, "1", full_str);
    }
    {
        let full_str = "0000000000000000000000000000000000000000000000000000000000000000\
                        0000000000000000000000000000000000000000000000000000000000000001";
        test_from_str_via_trimmed_str!(H512, "1", full_str);
    }
    {
        let full_str = "0000000000000000000000000000000000000000000000000000000000000000\
                        0000000000000000000000000000000000000000000000000000000000000000\
                        01";
        test_from_str_via_trimmed_str!(H520, "1", full_str);
    }
    {
        let full_str = "1000000000000000000000000000000000000001";
        test_from_str_via_trimmed_str!(H160, full_str, full_str);
    }
    {
        let full_str = "1000000000000000000000000000000000000000000000000000000000000001";
        test_from_str_via_trimmed_str!(H256, full_str, full_str);
    }
    {
        let full_str = "1000000000000000000000000000000000000000000000000000000000000000\
                        0000000000000000000000000000000000000000000000000000000000000001";
        test_from_str_via_trimmed_str!(H512, full_str, full_str);
    }
    {
        let full_str = "1000000000000000000000000000000000000000000000000000000000000000\
                        0000000000000000000000000000000000000000000000000000000000000000\
                        01";
        test_from_str_via_trimmed_str!(H520, full_str, full_str);
    }
}

macro_rules! add_tests {
    ($test_name:ident, $type:ident, $bytes_size:literal) => {
        #[test]
        fn $test_name() {
            let zeros = $type([0; $bytes_size]);
            let zeros_str = format!("{:0>width$}", 0, width = $bytes_size * 2);
            let short_str = format!("{:0>width$}", 0, width = $bytes_size * 2 - 1);
            let long_str = format!("{:0>width$}", 0, width = $bytes_size * 2 + 1);
            let has_invalid_char_str = format!("x{:0>width$}", 0, width = $bytes_size * 2 - 1);
            {
                let from_zeros = $type::from_str(&zeros_str).unwrap();
                assert_eq!(zeros, from_zeros);
            }
            {
                let expected = FromStrError::InvalidLength(1);
                let actual = $type::from_str("0").unwrap_err();
                assert_eq!(expected, actual);

                let expected = FromStrError::InvalidLength($bytes_size * 2 - 1);
                let actual = $type::from_str(&short_str).unwrap_err();
                assert_eq!(expected, actual);

                let expected = FromStrError::InvalidLength($bytes_size * 2 + 1);
                let actual = $type::from_str(&long_str).unwrap_err();
                assert_eq!(expected, actual);

                let expected = FromStrError::InvalidCharacter { chr: b'x', idx: 0 };
                let actual = $type::from_str(&has_invalid_char_str).unwrap_err();
                assert_eq!(expected, actual);
            }
            {
                let from_empty = $type::from_trimmed_str("0").unwrap();
                assert_eq!(zeros, from_empty);

                let from_zero = $type::from_trimmed_str("").unwrap();
                assert_eq!(zeros, from_zero);
            }
            {
                let expected = FromStrError::InvalidLength($bytes_size * 2 + 1);
                let actual = $type::from_trimmed_str(&long_str).unwrap_err();
                assert_eq!(expected, actual);

                let expected = FromStrError::InvalidCharacter { chr: b'0', idx: 0 };
                let actual = $type::from_trimmed_str(&short_str).unwrap_err();
                assert_eq!(expected, actual);

                let expected = FromStrError::InvalidCharacter { chr: b'_', idx: 8 };
                let actual = $type::from_trimmed_str("12345678_90abcdef").unwrap_err();
                assert_eq!(expected, actual);
            }
            {
                let only_lowest_bit_is_one_str =
                    format!("{:0>width$}{}", 0, 0b0001, width = $bytes_size * 2 - 1);
                let from_full = $type::from_str(&only_lowest_bit_is_one_str).unwrap();
                let from_trimmed = $type::from_trimmed_str("1").unwrap();
                assert_eq!(from_full, from_trimmed);
            }
            {
                let only_highest_bit_is_one_str =
                    format!("{}{:0>width$}", 0b1000, 0, width = $bytes_size * 2 - 1);
                let from_full = $type::from_str(&only_highest_bit_is_one_str).unwrap();
                let from_trimmed = $type::from_trimmed_str(&only_highest_bit_is_one_str).unwrap();
                assert_eq!(from_full, from_trimmed);
            }
        }
    };
}

add_tests!(test_h160, H160, 20);
add_tests!(test_h256, H256, 32);
add_tests!(test_h512, H512, 64);
add_tests!(test_h520, H520, 65);
