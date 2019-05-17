use serde_derive::{Deserialize, Serialize};

use crate::OccupiedCapacity;

// The inner is the amount of `Shannons`.
#[derive(
    Debug, Clone, Copy, Default, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct Capacity(u64);

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

impl ::std::str::FromStr for Capacity {
    type Err = ::std::num::ParseIntError;

    fn from_str(s: &str) -> ::std::result::Result<Self, Self::Err> {
        Ok(Capacity(s.parse::<u64>()?))
    }
}

impl ::std::fmt::Display for Capacity {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
