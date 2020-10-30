use ckb_types::{bytes::Bytes, packed, prelude::*};
use faster_hex::{hex_decode, hex_encode};
use std::fmt;

/// Variable-length binary encoded as a 0x-prefixed hex string in JSON.
///
/// ## Example
///
/// | JSON       | Binary                               |
/// | ---------- | ------------------------------------ |
/// | "0x"       | Empty binary                         |
/// | "0x00"     | Single byte 0                        |
/// | "0x636b62" | 3 bytes, UTF-8 encoding of ckb       |
/// | "00"       | Invalid, 0x is required              |
/// | "0x0"      | Invalid, each byte requires 2 digits |
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct JsonBytes(Bytes);

impl JsonBytes {
    /// TODO(doc): @doitian
    pub fn from_bytes(bytes: Bytes) -> Self {
        JsonBytes(bytes)
    }

    /// TODO(doc): @doitian
    pub fn from_vec(vec: Vec<u8>) -> Self {
        JsonBytes(Bytes::from(vec))
    }

    /// TODO(doc): @doitian
    pub fn into_bytes(self) -> Bytes {
        let JsonBytes(bytes) = self;
        bytes
    }

    /// TODO(doc): @doitian
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// TODO(doc): @doitian
    pub fn is_empty(&self) -> bool {
        0 == self.len()
    }

    /// TODO(doc): @doitian
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl From<packed::Bytes> for JsonBytes {
    fn from(input: packed::Bytes) -> Self {
        JsonBytes::from_bytes(input.raw_data())
    }
}

impl From<JsonBytes> for packed::Bytes {
    fn from(input: JsonBytes) -> Self {
        input.0.pack()
    }
}

struct BytesVisitor;

impl<'b> serde::de::Visitor<'b> for BytesVisitor {
    type Value = JsonBytes;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(formatter, "a 0x-prefixed hex string")
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        if v.len() < 2 || &v.as_bytes()[0..2] != b"0x" {
            return Err(E::invalid_value(serde::de::Unexpected::Str(v), &self));
        }

        if v.len() & 1 != 0 {
            return Err(E::invalid_length(v.len(), &"even length"));
        }

        let bytes = &v.as_bytes()[2..];
        if bytes.is_empty() {
            return Ok(JsonBytes::default());
        }
        let mut buffer = vec![0; bytes.len() >> 1]; // we checked length
        hex_decode(bytes, &mut buffer).map_err(|e| E::custom(format_args!("{:?}", e)))?;
        Ok(JsonBytes::from_vec(buffer))
    }

    fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        self.visit_str(&v)
    }
}

impl serde::Serialize for JsonBytes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut buffer = vec![0u8; self.len() * 2 + 2];
        buffer[0] = b'0';
        buffer[1] = b'x';
        hex_encode(&self.as_bytes(), &mut buffer[2..])
            .map_err(|e| serde::ser::Error::custom(&format!("{}", e)))?;
        serializer.serialize_str(unsafe { ::std::str::from_utf8_unchecked(&buffer) })
    }
}

impl<'de> serde::Deserialize<'de> for JsonBytes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(BytesVisitor)
    }
}

#[cfg(test)]
mod test {
    use super::JsonBytes;
    use regex::Regex;
    use serde::{Deserialize, Serialize};

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
                    "invalid value: string \"{}\", \
                     expected a 0x-prefixed hex string at line {} column {}",
                    input, num_pattern, num_pattern
                ),
                ErrorKind::InvalidLength => format!(
                    "invalid length {}, \
                     expected even length at line {} column {}",
                    num_pattern, num_pattern, num_pattern
                ),
                ErrorKind::InvalidCharacter => format!(
                    "Invalid character at line {} column {}",
                    num_pattern, num_pattern
                ),
            }
        }

        fn test_de_error_for(kind: ErrorKind, input: &str) {
            let full_string = format!(r#"{{"bytes": "{}"}}"#, input);
            let full_error_pattern = format_error_pattern(kind, input);
            let error = serde_json::from_str::<Test>(&full_string)
                .unwrap_err()
                .to_string();
            let re = Regex::new(&full_error_pattern).unwrap();
            assert!(
                re.is_match(&error),
                "kind = {:?}, input = {}, error = {}",
                kind,
                input,
                error
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
}
