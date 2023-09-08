//! The essential rust types for CKB contracts.

#![allow(clippy::from_over_into)]

use crate::packed;

/// Specifies how the script `code_hash` is used to match the script code and how to run the code.
/// The hash type is split into the high 7 bits and the low 1 bit,
/// when the low 1 bit is 1, it indicates the type,
/// when the low 1 bit is 0, it indicates the data,
/// and then it relies on the high 7 bits to indicate
/// that the data actually corresponds to the version.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScriptHashType {
    /// Type "data" matches script code via cell data hash, and run the script code in v0 CKB VM.
    #[default]
    Data = 0,
    /// Type "type" matches script code via cell type script hash.
    Type = 1,
    /// Type "data1" matches script code via cell data hash, and run the script code in v1 CKB VM.
    Data1 = 2,
    /// Type "data2" matches script code via cell data hash, and run the script code in v2 CKB VM.
    Data2 = 4,
}

impl ScriptHashType {
    #[inline]
    pub(crate) fn verify_value(v: u8) -> bool {
        v <= 4 && v != 3
    }
}

impl Into<u8> for ScriptHashType {
    #[inline]
    fn into(self) -> u8 {
        match self {
            Self::Data => 0,
            Self::Type => 1,
            Self::Data1 => 2,
            Self::Data2 => 4,
        }
    }
}

impl Into<packed::Byte> for ScriptHashType {
    #[inline]
    fn into(self) -> packed::Byte {
        Into::<u8>::into(self).into()
    }
}

#[cfg(feature = "std")]
mod std_mod {
    use crate::{core::ScriptHashType, packed};
    use ckb_error::OtherError;

    impl TryFrom<u8> for ScriptHashType {
        type Error = OtherError;

        fn try_from(v: u8) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(ScriptHashType::Data),
                1 => Ok(ScriptHashType::Type),
                2 => Ok(ScriptHashType::Data1),
                4 => Ok(ScriptHashType::Data2),
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
}
