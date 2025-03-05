//! The mod mainnet and mod testnet's codes are generated
//! by script: ./devtools/release/update_default_valid_target.sh
//! Please don't modify them manually.

/// sync config related to mainnet
pub mod mainnet {
    /// Default assume valid target for mainnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in mainnet: the 60 days ago block is:
    /// height: 14908742
    /// hash: 0x216095bfc3bb68e7509db4b3f98b105ac5565586876a795a9c5c3d0dfe134cb5
    /// date: Sun Dec 22 03:04:27 PM CST 2024
    /// you can view this block in https://explorer.nervos.org/block/0x216095bfc3bb68e7509db4b3f98b105ac5565586876a795a9c5c3d0dfe134cb5
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0x216095bfc3bb68e7509db4b3f98b105ac5565586876a795a9c5c3d0dfe134cb5";
}
/// sync config related to testnet
pub mod testnet {
    /// Default assume valid target for testnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in testnet: the 60 days ago block is:
    /// height: 15641938
    /// hash: 0xd92fe833fd53c6e0c7f05516609c3bbf4777aa05d016523cf1ff8aeaeec6fc13
    /// date: Sun Dec 22 03:09:16 PM CST 2024
    /// you can view this block in https://pudge.explorer.nervos.org/block/0xd92fe833fd53c6e0c7f05516609c3bbf4777aa05d016523cf1ff8aeaeec6fc13
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0xd92fe833fd53c6e0c7f05516609c3bbf4777aa05d016523cf1ff8aeaeec6fc13";
}
