use ckb_core::Bytes;
use faster_hex::{hex_decode, hex_encode};
use std::fmt;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct JsonBytes(Bytes);

impl Default for JsonBytes {
    fn default() -> Self {
        JsonBytes(Bytes::default())
    }
}

impl JsonBytes {
    pub fn from_bytes(bytes: Bytes) -> Self {
        JsonBytes(bytes)
    }

    pub fn from_vec(vec: Vec<u8>) -> Self {
        JsonBytes(Bytes::from(vec))
    }

    pub fn into_bytes(self) -> Bytes {
        let JsonBytes(bytes) = self;
        bytes
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        0 == self.len()
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
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
    use serde_derive::{Deserialize, Serialize};
    use serde_json;

    #[test]
    fn test_toml_de_error() {
        #[derive(Deserialize, Serialize, Debug)]
        struct Test {
            bytes: JsonBytes,
        }

        let invalid_prefixed = r#"{"bytes": "2143"}"#;
        let e = serde_json::from_str::<Test>(invalid_prefixed);
        assert_eq!(
            "invalid value: string \"2143\", \
             expected a 0x-prefixed hex string at line 1 column 16",
            format!("{}", e.unwrap_err())
        );

        let invalid_prefixed = r#"{"bytes": "换个行会死吗"}"#;
        let e = serde_json::from_str::<Test>(invalid_prefixed);
        assert_eq!(
            "invalid value: string \"换个行会死吗\", \
             expected a 0x-prefixed hex string at line 1 column 30",
            format!("{}", e.unwrap_err())
        );

        let invalid_length = r#"{"bytes" : "0x0"}"#;
        let e = serde_json::from_str::<Test>(invalid_length);
        assert_eq!(
            r#"invalid length 3, expected even length at line 1 column 16"#,
            format!("{}", e.unwrap_err())
        );

        let invalid_length = r#"{"bytes":"0x这个测试写的真垃圾，需要用正则重写"}"#;
        let e = serde_json::from_str::<Test>(invalid_length);
        assert_eq!(
            r#"invalid length 53, expected even length at line 1 column 64"#,
            format!("{}", e.unwrap_err())
        );

        let illegal_char = r#"{"bytes":"0xgh"}"#;
        let e = serde_json::from_str::<Test>(illegal_char);
        assert_eq!(
            r#"Invalid character at line 1 column 15"#,
            format!("{}", e.unwrap_err())
        );

        let illegal_char = r#"{"bytes":"0x一二三四"}"#;
        let e = serde_json::from_str::<Test>(illegal_char);
        assert_eq!(
            r#"Invalid character at line 1 column 25"#,
            format!("{}", e.unwrap_err())
        );
    }
}
