use ckb_types::packed;
use ckb_types::prelude::{Pack, Unpack};
use serde::de::{Error, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

#[derive(Copy, Clone, Default, PartialEq, Eq, Hash, Debug)]
pub struct Uint128(u128);

impl Uint128 {
    pub fn value(self) -> u128 {
        self.0
    }
}

impl fmt::Display for Uint128 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:x}", self.value())
    }
}

impl From<u128> for Uint128 {
    fn from(value: u128) -> Self {
        Uint128(value)
    }
}

impl From<Uint128> for u128 {
    fn from(value: Uint128) -> Self {
        value.value()
    }
}

impl Pack<packed::Uint128> for Uint128 {
    fn pack(&self) -> packed::Uint128 {
        self.value().pack()
    }
}

impl Unpack<Uint128> for packed::Uint128 {
    fn unpack(&self) -> Uint128 {
        Uint128(Unpack::<u128>::unpack(self))
    }
}

impl Serialize for Uint128 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'a> Deserialize<'a> for Uint128 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'a>,
    {
        deserializer.deserialize_any(Uint128Visitor)
    }
}

struct Uint128Visitor;

impl<'a> Visitor<'a> for Uint128Visitor {
    type Value = Uint128;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(formatter, "a hex-encoded, 0x-prefixed uint128")
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

        let number = u128::from_str_radix(&value[2..], 16)
            .map(Uint128)
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
    use crate::uint128::Uint128;

    #[test]
    fn serialize() {
        assert_eq!(r#""0xd""#, serde_json::to_string(&Uint128(13)).unwrap());
        assert_eq!(r#""0x0""#, serde_json::to_string(&Uint128(0)).unwrap());
    }

    #[test]
    fn deserialize_heximal() {
        let s = r#""0xa""#;
        let deserialized: Uint128 = serde_json::from_str(s).unwrap();
        assert_eq!(deserialized, Uint128(10));
    }

    #[test]
    fn deserialize_decimal() {
        let s = r#""10""#;
        let ret: Result<Uint128, _> = serde_json::from_str(s);
        assert!(ret.is_err(), ret);
    }

    #[test]
    fn deserialize_with_redundant_leading_zeros() {
        let cases = vec![r#""0x01""#, r#""0x00""#];
        for s in cases {
            let ret: Result<Uint128, _> = serde_json::from_str(s);
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
        let s = format!(r#""0x{:x}0""#, u128::max_value());
        let ret: Result<Uint128, _> = serde_json::from_str(&s);
        assert!(ret.is_err(), ret);
        let err = ret.unwrap_err();

        assert!(
            err.to_string()
                .contains("number too large to fit in target type"),
            err,
        );
    }
}
