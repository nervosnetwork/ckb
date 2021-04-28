use serde::{Deserialize, Serialize};

/// CKB capacity.
///
/// It is encoded as the amount of `Shannons` internally.
#[derive(
    Debug, Clone, Copy, Default, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct Capacity(u64);

/// Represents the ratio `numerator / denominator`, where `numerator` and `denominator` are both
/// unsigned 64-bit integers.
#[derive(Clone, PartialEq, Debug, Eq, Copy, Deserialize, Serialize)]
pub struct Ratio {
    /// Numerator.
    numer: u64,
    /// Denominator.
    denom: u64,
}

impl Ratio {
    /// Creates a ratio numer / denom.
    pub const fn new(numer: u64, denom: u64) -> Self {
        Self { numer, denom }
    }

    /// The numerator in ratio numerator / denominator.
    pub fn numer(&self) -> u64 {
        self.numer
    }

    /// The denominator in ratio numerator / denominator.
    pub fn denom(&self) -> u64 {
        self.denom
    }
}

/// Conversion into `Capacity`.
pub trait IntoCapacity {
    /// Converts `self` into `Capacity`.
    fn into_capacity(self) -> Capacity;
}

impl IntoCapacity for Capacity {
    fn into_capacity(self) -> Capacity {
        self
    }
}

impl IntoCapacity for u64 {
    fn into_capacity(self) -> Capacity {
        Capacity::shannons(self)
    }
}

impl IntoCapacity for u32 {
    fn into_capacity(self) -> Capacity {
        Capacity::shannons(u64::from(self))
    }
}

impl IntoCapacity for u16 {
    fn into_capacity(self) -> Capacity {
        Capacity::shannons(u64::from(self))
    }
}

impl IntoCapacity for u8 {
    fn into_capacity(self) -> Capacity {
        Capacity::shannons(u64::from(self))
    }
}

// A `Byte` contains how many `Shannons`.
const BYTE_SHANNONS: u64 = 100_000_000;

/// Numeric errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Numeric overflow.
    Overflow,
}

impl ::std::fmt::Display for Error {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        write!(f, "OccupiedCapacity: overflow")
    }
}

impl ::std::error::Error for Error {}

/// Numeric operation result.
pub type Result<T> = ::std::result::Result<T, Error>;

impl Capacity {
    /// Capacity of zero Shannons.
    pub const fn zero() -> Self {
        Capacity(0)
    }

    /// Capacity of one Shannon.
    pub const fn one() -> Self {
        Capacity(1)
    }

    /// Views the capacity as Shannons.
    pub const fn shannons(val: u64) -> Self {
        Capacity(val)
    }

    /// Views the capacity as CKBytes.
    pub fn bytes(val: usize) -> Result<Self> {
        (val as u64)
            .checked_mul(BYTE_SHANNONS)
            .map(Capacity::shannons)
            .ok_or(Error::Overflow)
    }

    /// Views the capacity as Shannons.
    pub fn as_u64(self) -> u64 {
        self.0
    }

    /// Adds self and rhs and checks overflow error.
    pub fn safe_add<C: IntoCapacity>(self, rhs: C) -> Result<Self> {
        self.0
            .checked_add(rhs.into_capacity().0)
            .map(Capacity::shannons)
            .ok_or(Error::Overflow)
    }

    /// Subtracts self and rhs and checks overflow error.
    pub fn safe_sub<C: IntoCapacity>(self, rhs: C) -> Result<Self> {
        self.0
            .checked_sub(rhs.into_capacity().0)
            .map(Capacity::shannons)
            .ok_or(Error::Overflow)
    }

    /// Multiplies self and rhs and checks overflow error.
    pub fn safe_mul<C: IntoCapacity>(self, rhs: C) -> Result<Self> {
        self.0
            .checked_mul(rhs.into_capacity().0)
            .map(Capacity::shannons)
            .ok_or(Error::Overflow)
    }

    /// Multiplies self with a ratio and checks overflow error.
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
