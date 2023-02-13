use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::bytes::JsonBytes;

#[test]
fn test_de_error() {
    #[derive(Deserialize, Serialize, Debug)]
    struct Test {
        bytes: JsonBytes,
    }

    #[allow(clippy::enum_variant_names)]
    #[derive(Debug, Clone, Copy)]
    enum ErrorKind {
        InvalidValue,
        InvalidLength,
        InvalidCharacter,
    }

    fn format_error_pattern(kind: ErrorKind, input: &str) -> String {
        let num_pattern = "[1-9][0-9]*";
        match kind {
            ErrorKind::InvalidValue => format!(
                "invalid value: string \"{input}\", \
                     expected a 0x-prefixed hex string at line {num_pattern} column {num_pattern}"
            ),
            ErrorKind::InvalidLength => format!(
                "invalid length {num_pattern}, \
                     expected even length at line {num_pattern} column {num_pattern}"
            ),
            ErrorKind::InvalidCharacter => {
                format!("Invalid character at line {num_pattern} column {num_pattern}")
            }
        }
    }

    fn test_de_error_for(kind: ErrorKind, input: &str) {
        let full_string = format!(r#"{{"bytes": "{input}"}}"#);
        let full_error_pattern = format_error_pattern(kind, input);
        let error = serde_json::from_str::<Test>(&full_string)
            .unwrap_err()
            .to_string();
        let re = Regex::new(&full_error_pattern).unwrap();
        assert!(
            re.is_match(&error),
            "kind = {kind:?}, input = {input}, error = {error}"
        );
    }

    let testcases = vec![
        (ErrorKind::InvalidValue, "1234"),
        (ErrorKind::InvalidValue, "测试非 ASCII 字符不会 panic"),
        (ErrorKind::InvalidLength, "0x0"),
        (ErrorKind::InvalidLength, "0x测试非 ASCII 字符不会 panic~"),
        (ErrorKind::InvalidCharacter, "0x0z"),
        (ErrorKind::InvalidCharacter, "0x测试非 ASCII 字符不会 panic"),
    ];

    for (kind, input) in testcases.into_iter() {
        test_de_error_for(kind, input);
    }
}
