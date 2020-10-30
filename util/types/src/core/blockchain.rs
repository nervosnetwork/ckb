use failure::{err_msg, Error as FailureError};
use std::convert::TryFrom;

use crate::packed;

/// TODO(doc): @quake
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScriptHashType {
    /// TODO(doc): @quake
    Data = 0,
    /// TODO(doc): @quake
    Type = 1,
}

impl Default for ScriptHashType {
    fn default() -> Self {
        ScriptHashType::Data
    }
}

impl TryFrom<packed::Byte> for ScriptHashType {
    type Error = FailureError;

    fn try_from(v: packed::Byte) -> Result<Self, Self::Error> {
        match Into::<u8>::into(v) {
            0 => Ok(ScriptHashType::Data),
            1 => Ok(ScriptHashType::Type),
            _ => Err(err_msg(format!("Invalid script hash type {}", v))),
        }
    }
}

impl ScriptHashType {
    #[inline]
    pub(crate) fn verify_value(v: u8) -> bool {
        v <= 1
    }
}

impl Into<u8> for ScriptHashType {
    #[inline]
    fn into(self) -> u8 {
        self as u8
    }
}

impl Into<packed::Byte> for ScriptHashType {
    #[inline]
    fn into(self) -> packed::Byte {
        (self as u8).into()
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
    type Error = FailureError;

    fn try_from(v: packed::Byte) -> Result<Self, Self::Error> {
        match Into::<u8>::into(v) {
            0 => Ok(DepType::Code),
            1 => Ok(DepType::DepGroup),
            _ => Err(err_msg(format!("Invalid dep type {}", v))),
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
