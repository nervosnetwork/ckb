/// The mod mainnet and mod testnet's codes are generated
/// by script: ./devtools/release/update_default_valid_target.sh
/// Please don't modify them manually.
///

/// sync config related to mainnet
pub mod mainnet {
    /// Default assume valid target for mainnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in mainnet: the 60 days ago block is:
    /// height: 13881224
    /// hash: 0x191da7c29c644a10525a85ca71c02fc7ba162e8badb3ebbc2076d4119a70479b
    /// date: Wed Aug 28 04:25:45 PM CST 2024
    /// you can view this block in https://explorer.nervos.org/block/0x191da7c29c644a10525a85ca71c02fc7ba162e8badb3ebbc2076d4119a70479b
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0x191da7c29c644a10525a85ca71c02fc7ba162e8badb3ebbc2076d4119a70479b";
}
/// sync config related to testnet
pub mod testnet {
    /// Default assume valid target for testnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in testnet: the 60 days ago block is:
    /// height: 14390774
    /// hash: 0x15d8685e20d9abad55c2fe63b3ce3dc3d83481ade3c2b65b3bcab34452837a6f
    /// date: Wed Aug 28 04:26:10 PM CST 2024
    /// you can view this block in https://pudge.explorer.nervos.org/block/0x15d8685e20d9abad55c2fe63b3ce3dc3d83481ade3c2b65b3bcab34452837a6f
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0x15d8685e20d9abad55c2fe63b3ce3dc3d83481ade3c2b65b3bcab34452837a6f";
}
