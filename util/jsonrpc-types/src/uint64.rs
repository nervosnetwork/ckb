use ckb_types::prelude::{Pack, Unpack};
use ckb_types::{core, packed};
use serde::de::{Error, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

#[derive(Copy, Clone, Default, PartialEq, Eq, Hash, Debug)]
pub struct Uint64(u64);

impl Uint64 {
    pub fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Uint64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:x}", self.value())
    }
}

impl From<core::Capacity> for Uint64 {
    fn from(value: core::Capacity) -> Self {
        Uint64(value.as_u64())
    }
}

impl From<core::DetailedEpochNumber> for Uint64 {
    fn from(value: core::DetailedEpochNumber) -> Self {
        Uint64(value.full_value())
    }
}

impl From<Uint64> for core::Capacity {
    fn from(value: Uint64) -> Self {
        core::Capacity::shannons(value.value())
    }
}

impl From<u64> for Uint64 {
    fn from(value: u64) -> Self {
        Uint64(value)
    }
}

impl From<Uint64> for u64 {
    fn from(value: Uint64) -> Self {
        value.value()
    }
}

impl Pack<packed::Uint64> for Uint64 {
    fn pack(&self) -> packed::Uint64 {
        self.value().pack()
    }
}

impl Unpack<Uint64> for packed::Uint64 {
    fn unpack(&self) -> Uint64 {
        Uint64(Unpack::<u64>::unpack(self))
    }
}

impl Serialize for Uint64 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'a> Deserialize<'a> for Uint64 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'a>,
    {
        deserializer.deserialize_any(Uint64Visitor)
    }
}

struct Uint64Visitor;

impl<'a> Visitor<'a> for Uint64Visitor {
    type Value = Uint64;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(formatter, "a hex-encoded, 0x-prefixed uint64")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: Error,
    {
        if !value.starts_with("0x") {
            return Err(Error::custom(format!(
                "Invalid uint64 {}: without `0x` prefix",
                value
            )));
        }

        let number = u64::from_str_radix(&value[2..], 16)
            .map(Uint64)
            .map_err(|e| Error::custom(format!("Invalid uint64 {}: {}", value, e)))?;
        if number.to_string() != value {
            return Err(Error::custom(format!(
                "Invalid uint64 {}: with redundant leading zeros, expected: {}",
                value,
                number.to_string(),
            )));
        }

        Ok(number)
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: Error,
    {
        self.visit_str(&value)
    }
}

#[cfg(test)]
mod tests {
    use crate::uint64::Uint64;

    #[test]
    fn serialize() {
        assert_eq!(r#""0xd""#, serde_json::to_string(&Uint64(13)).unwrap());
        assert_eq!(r#""0x0""#, serde_json::to_string(&Uint64(0)).unwrap());
    }

    #[test]
    fn deserialize_heximal() {
        let s = r#""0xa""#;
        let deserialized: Uint64 = serde_json::from_str(s).unwrap();
        assert_eq!(deserialized, Uint64(10));
    }

    #[test]
    fn deserialize_decimal() {
        let s = r#""10""#;
        let ret: Result<Uint64, _> = serde_json::from_str(s);
        assert!(ret.is_err(), ret);
    }

    #[test]
    fn deserialize_with_redundant_leading_zeros() {
        let cases = vec![r#""0x01""#, r#""0x00""#];
        for s in cases {
            let ret: Result<Uint64, _> = serde_json::from_str(s);
            assert!(ret.is_err(), ret);

            let err = ret.unwrap_err();
            assert!(
                err.to_string().contains("with redundant leading zeros"),
                err,
            );
        }
    }

    #[test]
    fn deserialize_too_large() {
        let s = format!(r#""0x{:x}""#, u128::from(u64::max_value()) + 1);
        let ret: Result<Uint64, _> = serde_json::from_str(&s);
        assert!(ret.is_err(), ret);

        let err = ret.unwrap_err();
        assert!(
            err.to_string()
                .contains("number too large to fit in target type"),
            err,
        );
    }
}
