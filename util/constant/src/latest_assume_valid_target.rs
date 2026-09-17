//! The mod mainnet and mod testnet's codes are generated
//! by script: ./devtools/release/update_default_valid_target.sh
//! Please don't modify them manually.

/// sync config related to mainnet
pub mod mainnet {
    /// Default assume valid target for mainnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in mainnet: the 60 days ago block is:
    /// height: 19897893
    /// hash: 0xfe1047b7c0c6da39e71774fbe0241668c134abd8ee89a10f4ff52b2d69d3c6c3
    /// date: Sat Jul 18 14:07:01 CST 2026
    /// you can view this block in https://explorer.nervos.org/block/0xfe1047b7c0c6da39e71774fbe0241668c134abd8ee89a10f4ff52b2d69d3c6c3
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0xfe1047b7c0c6da39e71774fbe0241668c134abd8ee89a10f4ff52b2d69d3c6c3";
}
/// sync config related to testnet
pub mod testnet {
    /// Default assume valid target for testnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in testnet: the 60 days ago block is:
    /// height: 21786575
    /// hash: 0x3004b75aac100fe4495012d3f111357a4966601da9fbebe8d0e0e4b9fe7027f9
    /// date: Sat Jul 18 14:07:58 CST 2026
    /// you can view this block in https://testnet.explorer.nervos.org/block/0x3004b75aac100fe4495012d3f111357a4966601da9fbebe8d0e0e4b9fe7027f9
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0x3004b75aac100fe4495012d3f111357a4966601da9fbebe8d0e0e4b9fe7027f9";
}
