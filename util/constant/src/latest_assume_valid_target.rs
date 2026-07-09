//! The mod mainnet and mod testnet's codes are generated
//! by script: ./devtools/release/update_default_valid_target.sh
//! Please don't modify them manually.

/// sync config related to mainnet
pub mod mainnet {
    /// Default assume valid target for mainnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in mainnet: the 60 days ago block is:
    /// height: 19274114
    /// hash: 0xd81931b857abe0521203372f0175ef692aab344865b5563e1e287ff46aaa09f6
    /// date: Sat May  9 13:56:13 CST 2026
    /// you can view this block in https://explorer.nervos.org/block/0xd81931b857abe0521203372f0175ef692aab344865b5563e1e287ff46aaa09f6
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0xd81931b857abe0521203372f0175ef692aab344865b5563e1e287ff46aaa09f6";
}
/// sync config related to testnet
pub mod testnet {
    /// Default assume valid target for testnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in testnet: the 60 days ago block is:
    /// height: 21030885
    /// hash: 0x4ec3242ee50da3ef5f7a03dde9ae2b69fe11e75be020c2bc2184231ab426c1b1
    /// date: Sat May  9 13:56:55 CST 2026
    /// you can view this block in https://testnet.explorer.nervos.org/block/0x4ec3242ee50da3ef5f7a03dde9ae2b69fe11e75be020c2bc2184231ab426c1b1
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0x4ec3242ee50da3ef5f7a03dde9ae2b69fe11e75be020c2bc2184231ab426c1b1";
}
