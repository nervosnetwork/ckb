use crate::{H160, H256, H512, H520};
use serde_json::json;
use std::str::FromStr;

macro_rules! add_tests {
    ($test_name:ident, $type:ident, $bytes_size:literal) => {
        #[test]
        fn $test_name() {
            let simple_str = format!("{}{:0>width$}", 0b1000, 0, width = $bytes_size * 2 - 1);
            let short_str = format!("{}{:0>width$}", 0b1000, 0, width = $bytes_size * 2 - 2);
            let long_str = format!("{}{:0>width$}", 0b1000, 0, width = $bytes_size * 2);
            // let has_invalid_char_str = format!("x{:0>width$}", 0, width = $bytes_size * 2 - 1);
            {
                let simple_0x_str = format!("0x{}", simple_str);
                let from_serde: $type = serde_json::from_value(json!(simple_0x_str)).unwrap();
                let from_str = $type::from_str(&simple_str).unwrap();
                assert_eq!(from_serde, from_str);
            }
            {
                let short_0x_str = format!("0x{}", short_str);
                let from_serde_res: Result<$type, _> = serde_json::from_value(json!(short_0x_str));
                assert!(from_serde_res.is_err());
            }
            {
                let long_0x_str = format!("0x{}", long_str);
                let from_serde_res: Result<$type, _> = serde_json::from_value(json!(long_0x_str));
                assert!(from_serde_res.is_err());
            }
            {
                let invalid_str = format!("0y{}", simple_str);
                let from_serde_res: Result<$type, _> = serde_json::from_value(json!(invalid_str));
                assert!(from_serde_res.is_err());
            }
            {
                let invalid_0x_str = format!("0x{}y", short_str);
                let from_serde_res: Result<$type, _> =
                    serde_json::from_value(json!(invalid_0x_str));
                assert!(from_serde_res.is_err());
            }
        }
    };
}

add_tests!(test_h160, H160, 20);
add_tests!(test_h256, H256, 32);
add_tests!(test_h512, H512, 64);
add_tests!(test_h520, H520, 65);
