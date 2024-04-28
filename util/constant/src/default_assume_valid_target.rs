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
    /// height: 12051310
    /// hash: 0xc394fa4c5e5032c49f3502d4fd8054ead76ff693a54ac90757e441b5119afcaf
    /// date: Fri Jan 26 04:59:07 PM CST 2024
    /// you can view this block in https://explorer.nervos.org/block/0xc394fa4c5e5032c49f3502d4fd8054ead76ff693a54ac90757e441b5119afcaf
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0xc394fa4c5e5032c49f3502d4fd8054ead76ff693a54ac90757e441b5119afcaf";
    /// Default assume valid target's height
    pub const DEFAULT_ASSUME_VALID_TARGET_HEIGHT: u64 = 12051310;
}
/// sync config related to testnet
pub mod testnet {
    /// Default assume valid target for testnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in testnet: the 60 days ago block is:
    /// height: 12069984
    /// hash: 0xb60b1fbe31f02ad58234dee525400c161c604e851c4ef839e4ef4b9422cfb445
    /// date: Fri Jan 26 04:59:59 PM CST 2024
    /// you can view this block in https://pudge.explorer.nervos.org/block/0xb60b1fbe31f02ad58234dee525400c161c604e851c4ef839e4ef4b9422cfb445
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0xb60b1fbe31f02ad58234dee525400c161c604e851c4ef839e4ef4b9422cfb445";
    /// Default assume valid target's height
    pub const DEFAULT_ASSUME_VALID_TARGET_HEIGHT: u64 = 12069984;
}
