//! The mod mainnet and mod testnet's codes are generated
//! by script: ./devtools/release/update_default_valid_target.sh
//! Please don't modify them manually.

/// sync config related to mainnet
pub mod mainnet {
    /// Default assume valid target for mainnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in mainnet: the 60 days ago block is:
    /// height: 15836361
    /// hash: 0x404b0dde051c49ea989fbc85c86aac6aba0f9ce38f5cdbfdec23493fb8b52e80
    /// date: Sun 06 Apr 2025 09:48:35 AM UTC
    /// you can view this block in https://explorer.nervos.org/block/0x404b0dde051c49ea989fbc85c86aac6aba0f9ce38f5cdbfdec23493fb8b52e80
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0x404b0dde051c49ea989fbc85c86aac6aba0f9ce38f5cdbfdec23493fb8b52e80";
}
/// sync config related to testnet
pub mod testnet {
    /// Default assume valid target for testnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in testnet: the 60 days ago block is:
    /// height: 16776647
    /// hash: 0xe68d17c4c2b5f5fba7b9af2875d40f2da4b506e9b46930e9774f18a3d9b79381
    /// date: Sun 06 Apr 2025 09:48:40 AM UTC
    /// you can view this block in https://pudge.explorer.nervos.org/block/0xe68d17c4c2b5f5fba7b9af2875d40f2da4b506e9b46930e9774f18a3d9b79381
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0xe68d17c4c2b5f5fba7b9af2875d40f2da4b506e9b46930e9774f18a3d9b79381";
}
