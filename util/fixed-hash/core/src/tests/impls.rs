use crate::{error::FromSliceError, H160, H256, H512, H520};

macro_rules! add_tests {
    ($test_name:ident, $type:ident, $bytes_size:literal) => {
        #[test]
        fn $test_name() {
            let original = $type::from_trimmed_str("1").unwrap();
            {
                let expected_bytes = {
                    let mut v = vec![0; $bytes_size];
                    v[$bytes_size - 1] = 1;
                    v
                };
                assert_eq!(original.as_bytes(), &expected_bytes);

                let new = $type::from_slice(original.as_bytes()).unwrap();
                assert_eq!(original, new);
            }
            {
                let short_bytes = vec![0; $bytes_size - 1];
                let expected = FromSliceError::InvalidLength($bytes_size - 1);
                let actual = $type::from_slice(&short_bytes).unwrap_err();
                assert_eq!(expected, actual);
            }
            {
                let long_bytes = vec![0; $bytes_size + 1];
                let expected = FromSliceError::InvalidLength($bytes_size + 1);
                let actual = $type::from_slice(&long_bytes).unwrap_err();
                assert_eq!(expected, actual);
            }
        }
    };
}

add_tests!(test_h160, H160, 20);
add_tests!(test_h256, H256, 32);
add_tests!(test_h512, H512, 64);
add_tests!(test_h520, H520, 65);
