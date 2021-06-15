use ckb_error::OtherError;
use std::convert::TryFrom;

use crate::packed;

/// TODO(doc): @quake
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScriptHashType {
    /// TODO(doc): @quake
    Data(u8),
    /// TODO(doc): @quake
    Type,
}

impl Default for ScriptHashType {
    fn default() -> Self {
        ScriptHashType::Data(0)
    }
}

impl TryFrom<packed::Byte> for ScriptHashType {
    type Error = OtherError;

    fn try_from(v: packed::Byte) -> Result<Self, Self::Error> {
        match Into::<u8>::into(v) {
            x if x % 2 == 0 => Ok(ScriptHashType::Data(x / 2)),
            1 => Ok(ScriptHashType::Type),
            _ => Err(OtherError::new(format!("Invalid script hash type {}", v))),
        }
    }
}

impl ScriptHashType {
    #[inline]
    pub(crate) fn verify_value(v: u8) -> bool {
        v % 2 == 0 || v == 1
    }
}

impl Into<u8> for ScriptHashType {
    #[inline]
    fn into(self) -> u8 {
        match self {
            Self::Data(v) => v * 2,
            Self::Type => 1,
        }
    }
}

impl Into<packed::Byte> for ScriptHashType {
    #[inline]
    fn into(self) -> packed::Byte {
        Into::<u8>::into(self).into()
    }
}

/// TODO(doc): @quake
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum DepType {
    /// TODO(doc): @quake
    Code = 0,
    /// TODO(doc): @quake
    DepGroup = 1,
}

impl Default for DepType {
    fn default() -> Self {
        DepType::Code
    }
}

impl TryFrom<packed::Byte> for DepType {
    type Error = OtherError;

    fn try_from(v: packed::Byte) -> Result<Self, Self::Error> {
        match Into::<u8>::into(v) {
            0 => Ok(DepType::Code),
            1 => Ok(DepType::DepGroup),
            _ => Err(OtherError::new(format!("Invalid dep type {}", v))),
        }
    }
}

impl Into<u8> for DepType {
    #[inline]
    fn into(self) -> u8 {
        self as u8
    }
}

impl Into<packed::Byte> for DepType {
    #[inline]
    fn into(self) -> packed::Byte {
        (self as u8).into()
    }
}

impl DepType {
    #[inline]
    pub(crate) fn verify_value(v: u8) -> bool {
        v <= 1
    }
}
