use serde;

use crate::OccupiedCapacity;

// The inner is the amount of `Shannons`.
#[derive(Debug, Clone, Copy, Default, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Capacity(u64);

impl serde::Serialize for Capacity {
    fn serialize<S>(&self, serializer: S) -> ::std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_newtype_struct("Capacity", &(self.0 / 100_000_000))
    }
}

impl<'de> serde::Deserialize<'de> for Capacity {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_u64(CapacityVisitor)
    }
}

struct CapacityVisitor;

impl<'de> serde::de::Visitor<'de> for CapacityVisitor {
    type Value = Capacity;

    fn expecting(&self, formatter: &mut ::std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("an integer between 0 and 2^64")
    }

    fn visit_u8<E>(self, value: u8) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(Capacity(u64::from(value) * 100_000_000))
    }

    fn visit_u16<E>(self, value: u16) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(Capacity(u64::from(value) * 100_000_000))
    }

    fn visit_u32<E>(self, value: u32) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(Capacity(u64::from(value) * 100_000_000))
    }

    fn visit_u64<E>(self, value: u64) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(Capacity(value * 100_000_000))
    }

    fn visit_i8<E>(self, value: i8) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(Capacity((value as u64) * 100_000_000))
    }

    fn visit_i16<E>(self, value: i16) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(Capacity((value as u64) * 100_000_000))
    }

    fn visit_i32<E>(self, value: i32) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(Capacity((value as u64) * 100_000_000))
    }

    fn visit_i64<E>(self, value: i64) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(Capacity((value as u64) * 100_000_000))
    }
}

// Be careful: if the inner type of `Capacity` was changed, update this!
impl OccupiedCapacity for Capacity {
    fn occupied_capacity(&self) -> Result<Capacity> {
        self.0.occupied_capacity()
    }
}

// A `Byte` contains how many `Shannons`.
const BYTE_SHANNONS: u64 = 100_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Overflow,
}

impl ::std::fmt::Display for Error {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        write!(f, "OccupiedCapacity: overflow")
    }
}

impl ::std::error::Error for Error {}

pub type Result<T> = ::std::result::Result<T, Error>;

impl Capacity {
    pub const fn zero() -> Self {
        Capacity(0)
    }

    pub const fn one() -> Self {
        Capacity(1)
    }

    pub const fn shannons(val: u64) -> Self {
        Capacity(val)
    }

    pub fn bytes(val: usize) -> Result<Self> {
        (val as u64)
            .checked_mul(BYTE_SHANNONS)
            .map(Capacity::shannons)
            .ok_or(Error::Overflow)
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }

    pub fn safe_add(self, rhs: Self) -> Result<Self> {
        self.0
            .checked_add(rhs.0)
            .map(Capacity::shannons)
            .ok_or(Error::Overflow)
    }

    pub fn safe_sub(self, rhs: Self) -> Result<Self> {
        self.0
            .checked_sub(rhs.0)
            .map(Capacity::shannons)
            .ok_or(Error::Overflow)
    }
}

impl ::std::string::ToString for Capacity {
    fn to_string(&self) -> String {
        self.0.to_string()
    }
}

impl ::std::str::FromStr for Capacity {
    type Err = ::std::num::ParseIntError;

    fn from_str(s: &str) -> ::std::result::Result<Self, Self::Err> {
        Ok(Capacity(s.parse::<u64>()?))
    }
}
