use crate::generated::packed;

pub type BlockNumber = u64;

/// Specifies how the script `code_hash` is used to match the script code and how to run the code.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ScriptHashType {
    /// Type "data" matches script code via cell data hash, and run the script code in v0 CKB VM.
    Data = 0,
    /// Type "type" matches script code via cell type script hash.
    Type = 1,
    /// Type "data1" matches script code via cell data hash, and run the script code in v1 CKB VM.
    Data1 = 2,
    /// Type "data2" matches script code via cell data hash, and run the script code in v2 CKB VM.
    #[cfg(feature = "ckb2023")]
    Data2 = 3,
}

impl Into<u8> for ScriptHashType {
    fn into(self) -> u8 {
        self as u8
    }
}

impl Into<packed::Byte> for ScriptHashType {
    fn into(self) -> packed::Byte {
        (self as u8).into()
    }
}
