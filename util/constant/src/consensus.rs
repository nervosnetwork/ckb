use phf::{Set, phf_set};

/// Dampening factor.
pub const TAU: u64 = 2;

/// Enabled script_hash_type
pub static ENABLED_SCRIPT_HASH_TYPE: Set<u8> = phf_set! {
    0u8, // ScriptHashType::Data
    1u8, // ScriptHashType::Type
    2u8, // ScriptHashType::Data1
    4u8, // ScriptHashType::Data2
};
