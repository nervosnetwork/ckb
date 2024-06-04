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
    /// height: 12623864
    /// hash: 0x84ef5fe7cbf4242bdcac76326aa33f15b9cc41958e9d891157b8a6066dad0f31
    /// date: Thu Apr  4 02:32:20 AM UTC 2024
    /// you can view this block in https://explorer.nervos.org/block/0x84ef5fe7cbf4242bdcac76326aa33f15b9cc41958e9d891157b8a6066dad0f31
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0x84ef5fe7cbf4242bdcac76326aa33f15b9cc41958e9d891157b8a6066dad0f31";
}
/// sync config related to testnet
pub mod testnet {
    /// Default assume valid target for testnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in testnet: the 60 days ago block is:
    /// height: 12811906
    /// hash: 0xc39d482e1c9cba7bdef254ff13e430f42cb5407e15464c029284cd5811e4c8df
    /// date: Thu Apr  4 02:32:39 AM UTC 2024
    /// you can view this block in https://pudge.explorer.nervos.org/block/0xc39d482e1c9cba7bdef254ff13e430f42cb5407e15464c029284cd5811e4c8df
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0xc39d482e1c9cba7bdef254ff13e430f42cb5407e15464c029284cd5811e4c8df";
}
