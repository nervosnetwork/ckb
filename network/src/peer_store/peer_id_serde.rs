use crate::PeerId;
use serde::{
    de::{self, Deserializer, Visitor},
    ser::Serializer,
};
use std::fmt;
use std::str::FromStr;

struct PeerIdVisitor;

impl<'de> Visitor<'de> for PeerIdVisitor {
    type Value = String;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(formatter, "a peer_id should be 32 bytes")
    }

    fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(s.to_owned())
    }
}

pub fn serialize<S>(peer_id: &PeerId, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&peer_id.to_base58())
}
pub fn deserialize<'de, D>(deserializer: D) -> Result<PeerId, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = deserializer.deserialize_str(PeerIdVisitor)?;
    PeerId::from_str(&s).map_err(|_| de::Error::custom("invalid peer id"))
}
