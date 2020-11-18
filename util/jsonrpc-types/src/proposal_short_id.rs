use ckb_types::{packed, prelude::*};
use faster_hex::{hex_decode, hex_encode};
use std::fmt;

/// The 10-byte fixed-length binary encoded as a 0x-prefixed hex string in JSON.
///
/// ## Example
///
/// ```text
/// 0xa0ef4eb5f4ceeb08a4c8
/// ```
#[derive(Clone, Default, PartialEq, Eq, Hash, Debug)]
pub struct ProposalShortId(pub [u8; 10]);

impl ProposalShortId {
    /// TODO(doc): @doitian
    pub fn new(inner: [u8; 10]) -> ProposalShortId {
        ProposalShortId(inner)
    }

    /// TODO(doc): @doitian
    pub fn into_inner(self) -> [u8; 10] {
        self.0
    }
}

impl From<packed::ProposalShortId> for ProposalShortId {
    fn from(core: packed::ProposalShortId) -> ProposalShortId {
        ProposalShortId::new(core.unpack())
    }
}

impl From<ProposalShortId> for packed::ProposalShortId {
    fn from(json: ProposalShortId) -> Self {
        json.into_inner().pack()
    }
}

struct ProposalShortIdVisitor;

impl<'b> serde::de::Visitor<'b> for ProposalShortIdVisitor {
    type Value = ProposalShortId;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(formatter, "a 0x-prefixed hex string")
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        if v.len() < 2 || &v.as_bytes()[0..2] != b"0x" || v.len() != 22 {
            return Err(E::invalid_value(serde::de::Unexpected::Str(v), &self));
        }
        let mut buffer = [0u8; 10]; // we checked length
        hex_decode(&v.as_bytes()[2..], &mut buffer)
            .map_err(|e| E::custom(format_args!("{:?}", e)))?;
        Ok(ProposalShortId::new(buffer))
    }

    fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        self.visit_str(&v)
    }
}

impl serde::Serialize for ProposalShortId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut buffer = [0u8; 22];
        buffer[0] = b'0';
        buffer[1] = b'x';
        hex_encode(&self.0, &mut buffer[2..])
            .map_err(|e| serde::ser::Error::custom(&format!("{}", e)))?;
        serializer.serialize_str(unsafe { ::std::str::from_utf8_unchecked(&buffer) })
    }
}

impl<'de> serde::Deserialize<'de> for ProposalShortId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(ProposalShortIdVisitor)
    }
}
