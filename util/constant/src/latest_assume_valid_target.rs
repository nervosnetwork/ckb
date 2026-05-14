//! The mod mainnet and mod testnet's codes are generated
//! by script: ./devtools/release/update_default_valid_target.sh
//! Please don't modify them manually.

/// sync config related to mainnet
pub mod mainnet {
    /// Default assume valid target for mainnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in mainnet: the 60 days ago block is:
    /// height: 18685287
    /// hash: 0x89670f609956f08e46bba05b1cd13675578cf2397e5d7eb22f6689f5000996c8
    /// date: Mon Feb 23 09:26:04 AM CST 2026
    /// you can view this block in https://explorer.nervos.org/block/0x89670f609956f08e46bba05b1cd13675578cf2397e5d7eb22f6689f5000996c8
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0x89670f609956f08e46bba05b1cd13675578cf2397e5d7eb22f6689f5000996c8";
}
/// sync config related to testnet
pub mod testnet {
    /// Default assume valid target for testnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in testnet: the 60 days ago block is:
    /// height: 20254372
    /// hash: 0xff760ab51618978772f005240db3a1db275cb0365421fc2bc77e342b5f310b3a
    /// date: Thu Feb 26 03:20:10 PM CST 2026
    /// you can view this block in https://testnet.explorer.nervos.org/block/0xff760ab51618978772f005240db3a1db275cb0365421fc2bc77e342b5f310b3a
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0xff760ab51618978772f005240db3a1db275cb0365421fc2bc77e342b5f310b3a";
}
