use serde::{Deserialize, Serialize};

// The inner is the amount of `Shannons`.
#[derive(
    Debug, Clone, Copy, Default, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct Capacity(u64);

#[derive(Clone, PartialEq, Debug, Eq, Copy, Deserialize, Serialize)]
pub struct Ratio(pub u64, pub u64);

impl Ratio {
    pub fn numer(&self) -> u64 {
        self.0
    }

    pub fn denom(&self) -> u64 {
        self.1
    }
}

pub trait AsCapacity {
    fn as_capacity(self) -> Capacity;
}

impl AsCapacity for Capacity {
    fn as_capacity(self) -> Capacity {
        self
    }
}

impl AsCapacity for u64 {
    fn as_capacity(self) -> Capacity {
        Capacity::shannons(self)
    }
}

impl AsCapacity for u32 {
    fn as_capacity(self) -> Capacity {
        Capacity::shannons(u64::from(self))
    }
}

impl AsCapacity for u16 {
    fn as_capacity(self) -> Capacity {
        Capacity::shannons(u64::from(self))
    }
}

impl AsCapacity for u8 {
    fn as_capacity(self) -> Capacity {
        Capacity::shannons(u64::from(self))
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

    pub fn safe_add<C: AsCapacity>(self, rhs: C) -> Result<Self> {
        self.0
            .checked_add(rhs.as_capacity().0)
            .map(Capacity::shannons)
            .ok_or(Error::Overflow)
    }

    pub fn safe_sub<C: AsCapacity>(self, rhs: C) -> Result<Self> {
        self.0
            .checked_sub(rhs.as_capacity().0)
            .map(Capacity::shannons)
            .ok_or(Error::Overflow)
    }

    pub fn safe_mul<C: AsCapacity>(self, rhs: C) -> Result<Self> {
        self.0
            .checked_mul(rhs.as_capacity().0)
            .map(Capacity::shannons)
            .ok_or(Error::Overflow)
    }

    pub fn safe_mul_ratio(self, ratio: Ratio) -> Result<Self> {
        self.0
            .checked_mul(ratio.numer())
            .and_then(|ret| ret.checked_div(ratio.denom()))
            .map(Capacity::shannons)
            .ok_or(Error::Overflow)
    }
}

impl ::std::str::FromStr for Capacity {
    type Err = ::std::num::ParseIntError;

    fn from_str(s: &str) -> ::std::result::Result<Self, Self::Err> {
        Ok(Capacity(s.parse::<u64>()?))
    }
}

impl ::std::fmt::Display for Capacity {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        self.0.fmt(f)
    }
}

impl ::std::fmt::LowerHex for Capacity {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        self.0.fmt(f)
    }
}
