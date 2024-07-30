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
    /// height: 12990091
    /// hash: 0xbff55baf83d738892474ecb815b771e1619d6c5b0a691089e46f882d7f575212
    /// date: Fri May 17 01:58:18 PM CST 2024
    /// you can view this block in https://explorer.nervos.org/block/0xbff55baf83d738892474ecb815b771e1619d6c5b0a691089e46f882d7f575212
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0xbff55baf83d738892474ecb815b771e1619d6c5b0a691089e46f882d7f575212";
}
/// sync config related to testnet
pub mod testnet {
    /// Default assume valid target for testnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in testnet: the 60 days ago block is:
    /// height: 13277762
    /// hash: 0x5ed229b24c4fdc3a578481c04165ca991d2a54e6fccd47d2406d66570a897b63
    /// date: Fri May 17 01:59:10 PM CST 2024
    /// you can view this block in https://pudge.explorer.nervos.org/block/0x5ed229b24c4fdc3a578481c04165ca991d2a54e6fccd47d2406d66570a897b63
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0x5ed229b24c4fdc3a578481c04165ca991d2a54e6fccd47d2406d66570a897b63";
}
