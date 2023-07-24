#[cfg(feature = "std")]
mod std_env;
#[cfg(feature = "std")]
pub use std_env::*;

use crate::packed;

pub type BlockNumber = u64;

/// Specifies how the script `code_hash` is used to match the script code and how to run the code.
/// The hash type is split into the high 7 bits and the low 1 bit,
/// when the low 1 bit is 1, it indicates the type,
/// when the low 1 bit is 0, it indicates the data,
/// and then it relies on the high 7 bits to indicate
/// that the data actually corresponds to the version.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScriptHashType {
    /// Type "data" matches script code via cell data hash, and run the script code in v0 CKB VM.
    Data = 0,
    /// Type "type" matches script code via cell type script hash.
    Type = 1,
    /// Type "data1" matches script code via cell data hash, and run the script code in v1 CKB VM.
    Data1 = 2,
    /// Type "data2" matches script code via cell data hash, and run the script code in v2 CKB VM.
    Data2 = 4,
}

impl Default for ScriptHashType {
    fn default() -> Self {
        ScriptHashType::Data
    }
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