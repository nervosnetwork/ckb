use crate::{H160, H256, H512, H520};
use std::str::FromStr;

macro_rules! add_tests {
    ($test_name:ident, $type:ident, $bytes_size:literal) => {
        #[test]
        fn $test_name() {
            let zeros = $type([0; $bytes_size]);
            let zeros_str = format!("{:0>width$}", 0, width = $bytes_size * 2);
            let only_lowest_bit_is_one_str =
                format!("{:0>width$}{}", 0, 0b0001, width = $bytes_size * 2 - 1);
            let only_highest_bit_is_one_str =
                format!("{}{:0>width$}", 0b1000, 0, width = $bytes_size * 2 - 1);

            let from_zeros = $type::from_str(&zeros_str).unwrap();
            let only_lowest_bit_is_one = $type::from_str(&only_lowest_bit_is_one_str).unwrap();
            let only_highest_bit_is_one = $type::from_str(&only_highest_bit_is_one_str).unwrap();

            assert!(zeros == from_zeros);
            assert!(zeros >= from_zeros);
            assert!(zeros <= from_zeros);

            assert!(from_zeros < only_lowest_bit_is_one);
            assert!(from_zeros <= only_lowest_bit_is_one);
            assert!(from_zeros != only_lowest_bit_is_one);
            assert!(only_lowest_bit_is_one > from_zeros);
            assert!(only_lowest_bit_is_one >= from_zeros);
            assert!(only_lowest_bit_is_one != from_zeros);

            assert!(from_zeros < only_highest_bit_is_one);
            assert!(from_zeros <= only_highest_bit_is_one);
            assert!(from_zeros != only_highest_bit_is_one);
            assert!(only_highest_bit_is_one > from_zeros);
            assert!(only_highest_bit_is_one >= from_zeros);
            assert!(only_highest_bit_is_one != from_zeros);

            assert!(only_lowest_bit_is_one < only_highest_bit_is_one);
            assert!(only_lowest_bit_is_one <= only_highest_bit_is_one);
            assert!(only_lowest_bit_is_one != only_highest_bit_is_one);
            assert!(only_highest_bit_is_one > only_lowest_bit_is_one);
            assert!(only_highest_bit_is_one >= only_lowest_bit_is_one);
            assert!(only_highest_bit_is_one != only_lowest_bit_is_one);
        }
    };
}

add_tests!(test_h160, H160, 20);
add_tests!(test_h256, H256, 32);
add_tests!(test_h512, H512, 64);
add_tests!(test_h520, H520, 65);
