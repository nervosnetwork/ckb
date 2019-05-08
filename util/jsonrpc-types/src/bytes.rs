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
        if v.len() < 2 || &v[0..2] != "0x" || v.len() & 1 != 0 {
            return Err(E::invalid_value(serde::de::Unexpected::Str(v), &self));
        }
        let bytes = &v.as_bytes()[2..];
        if bytes.is_empty() {
            return Ok(JsonBytes::default());
        }
        let mut buffer = vec![0; bytes.len() / 2]; // we checked length
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
