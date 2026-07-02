//! The mod mainnet and mod testnet's codes are generated
//! by script: ./devtools/release/update_default_valid_target.sh
//! Please don't modify them manually.

/// sync config related to mainnet
pub mod mainnet {
    /// Default assume valid target for mainnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in mainnet: the 60 days ago block is:
    /// height: 19039242
    /// hash: 0x506cc9ae64f577dd41b57d5860600334132cf502cabdff5a9cc2285c81fb8520
    /// date: Thu Apr  9 11:33:11 AM CST 2026
    /// you can view this block in https://explorer.nervos.org/block/0x506cc9ae64f577dd41b57d5860600334132cf502cabdff5a9cc2285c81fb8520
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0x506cc9ae64f577dd41b57d5860600334132cf502cabdff5a9cc2285c81fb8520";
}
/// sync config related to testnet
pub mod testnet {
    /// Default assume valid target for testnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in testnet: the 60 days ago block is:
    /// height: 20705983
    /// hash: 0xb5b26b01669c4288938759c97738e07a5a7a03a07c77a518db6d3913b034c0a5
    /// date: Thu Apr  9 11:32:55 AM CST 2026
    /// you can view this block in https://testnet.explorer.nervos.org/block/0xb5b26b01669c4288938759c97738e07a5a7a03a07c77a518db6d3913b034c0a5
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0xb5b26b01669c4288938759c97738e07a5a7a03a07c77a518db6d3913b034c0a5";
}
