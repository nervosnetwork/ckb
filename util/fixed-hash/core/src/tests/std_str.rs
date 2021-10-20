use crate::{H160, H256, H512, H520};
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
