use ckb_types::{packed, prelude::*};
use faster_hex::{hex_decode, hex_encode};
use std::fmt;

/// Fixed-length 32 bytes binary encoded as a 0x-prefixed hex string in JSON.
///
/// ## Example
///
/// ```text
/// 0xd495a106684401001e47c0ae1d5930009449d26e32380000000721efd0030000
/// ```
#[derive(Clone, Default, PartialEq, Eq, Hash, Debug)]
pub struct Byte32(pub [u8; 32]);

impl Byte32 {
    /// TODO(doc): @doitian
    pub fn new(inner: [u8; 32]) -> Self {
        Byte32(inner)
    }
}

impl From<packed::Byte32> for Byte32 {
    fn from(packed: packed::Byte32) -> Self {
        let mut inner: [u8; 32] = Default::default();
        inner.copy_from_slice(&packed.raw_data());
        Byte32(inner)
    }
}

impl From<Byte32> for packed::Byte32 {
    fn from(json: Byte32) -> Self {
        Self::from_slice(&json.0).expect("impossible: fail to read inner array")
    }
}

struct Byte32Visitor;

impl<'b> serde::de::Visitor<'b> for Byte32Visitor {
    type Value = Byte32;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(formatter, "a 0x-prefixed hex string")
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        if v.len() < 2 || &v.as_bytes()[0..2] != b"0x" || v.len() != 66 {
            return Err(E::invalid_value(serde::de::Unexpected::Str(v), &self));
        }
        let mut buffer = [0u8; 32]; // we checked length
        hex_decode(&v.as_bytes()[2..], &mut buffer)
            .map_err(|e| E::custom(format_args!("{:?}", e)))?;
        Ok(Byte32(buffer))
    }

    fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        self.visit_str(&v)
    }
}

impl serde::Serialize for Byte32 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut buffer = [0u8; 66];
        buffer[0] = b'0';
        buffer[1] = b'x';
        hex_encode(&self.0, &mut buffer[2..])
            .map_err(|e| serde::ser::Error::custom(&format!("{}", e)))?;
        serializer.serialize_str(unsafe { ::std::str::from_utf8_unchecked(&buffer) })
    }
}

impl<'de> serde::Deserialize<'de> for Byte32 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(Byte32Visitor)
    }
}
