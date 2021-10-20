mod impls;
mod serde;
mod std_cmp;
mod std_fmt;
mod std_str;

macro_rules! add_basic_tests {
    ($test_name:ident, $type:ident) => {
        #[test]
        fn $test_name() {
            let zeros = crate::$type::default();
            let zeros_clone = zeros.clone();
            assert_eq!(zeros, zeros_clone);
        }
    };
}

add_basic_tests!(test_h160, H160);
add_basic_tests!(test_h256, H256);
add_basic_tests!(test_h512, H512);
add_basic_tests!(test_h520, H520);
