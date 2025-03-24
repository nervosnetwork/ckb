//! The mod mainnet and mod testnet's codes are generated
//! by script: ./devtools/release/update_default_valid_target.sh
//! Please don't modify them manually.


/// sync config related to mainnet
pub mod mainnet {
    /// Default assume valid target for mainnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in mainnet: the 60 days ago block is:
    /// height: 15190307
    /// hash: 0xaf83c61fc14e6aa49111e18e3c466cf0923dfe1f4c8e91ab8c853625fb745c4c
    /// date: Wed Jan 22 11:30:02 AM CST 2025
    /// you can view this block in https://explorer.nervos.org/block/0xaf83c61fc14e6aa49111e18e3c466cf0923dfe1f4c8e91ab8c853625fb745c4c
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0xaf83c61fc14e6aa49111e18e3c466cf0923dfe1f4c8e91ab8c853625fb745c4c";
}
/// sync config related to testnet
pub mod testnet {
    /// Default assume valid target for testnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in testnet: the 60 days ago block is:
    /// height: 15974951
    /// hash: 0x60dce52f616ca2d98f5e97727cac16a9641e7ba388127904e993152ca51dbcb9
    /// date: Wed Jan 22 11:31:10 AM CST 2025
    /// you can view this block in https://pudge.explorer.nervos.org/block/0x60dce52f616ca2d98f5e97727cac16a9641e7ba388127904e993152ca51dbcb9
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0x60dce52f616ca2d98f5e97727cac16a9641e7ba388127904e993152ca51dbcb9";
}
