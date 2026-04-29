//! The schema include constants define the low level database column families.

pub mod legacy;
pub mod v1;

pub use v1::*;

/// Column families alias type.
pub type Col = &'static str;

/// META_TIP_HEADER_KEY tracks the latest known best block header
pub const META_TIP_HEADER_KEY: &[u8] = b"TIP_HEADER";
/// META_CURRENT_EPOCH_KEY tracks the latest known epoch
pub const META_CURRENT_EPOCH_KEY: &[u8] = b"CURRENT_EPOCH";
/// META_FILTER_DATA_KEY tracks the latest built filter data block hash
pub const META_LATEST_BUILT_FILTER_DATA_KEY: &[u8] = b"LATEST_BUILT_FILTER_DATA";

/// CHAIN_SPEC_HASH_KEY tracks the hash of chain spec which created current database
pub const CHAIN_SPEC_HASH_KEY: &[u8] = b"chain-spec-hash";
/// MIGRATION_VERSION_KEY tracks the current database version.
pub const MIGRATION_VERSION_KEY: &[u8] = b"db-version";
