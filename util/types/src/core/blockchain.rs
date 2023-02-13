use ckb_error::OtherError;

use crate::packed;

/// Specifies how the script `code_hash` is used to match the script code and how to run the code.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ScriptHashType {
    /// Type "data" matches script code via cell data hash, and run the script code in v0 CKB VM.
    Data = 0,
    /// Type "type" matches script code via cell type script hash.
    Type = 1,
    /// Type "data1" matches script code via cell data hash, and run the script code in v1 CKB VM.
    Data1 = 2,
}

impl Default for ScriptHashType {
    fn default() -> Self {
        ScriptHashType::Data
    }
}

impl TryFrom<u8> for ScriptHashType {
    type Error = OtherError;

    fn try_from(v: u8) -> Result<Self, Self::Error> {
        match v {
            0 => Ok(ScriptHashType::Data),
            1 => Ok(ScriptHashType::Type),
            2 => Ok(ScriptHashType::Data1),
            _ => Err(OtherError::new(format!("Invalid script hash type {v}"))),
        }
    }
}

impl TryFrom<packed::Byte> for ScriptHashType {
    type Error = OtherError;

    fn try_from(v: packed::Byte) -> Result<Self, Self::Error> {
        Into::<u8>::into(v).try_into()
    }
}

impl ScriptHashType {
    #[inline]
    pub(crate) fn verify_value(v: u8) -> bool {
        v <= 2
    }
}

impl Into<u8> for ScriptHashType {
    #[inline]
    fn into(self) -> u8 {
        match self {
            Self::Data => 0,
            Self::Type => 1,
            Self::Data1 => 2,
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
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
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
            _ => Err(OtherError::new(format!("Invalid dep type {v}"))),
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
