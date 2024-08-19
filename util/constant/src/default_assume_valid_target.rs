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
    /// height: 13266076
    /// hash: 0xd07c7f0fcea6e2a20a51dabbf0caae7ebff49723bcd8bac81a8b8c021a546a32
    /// date: Wed Jun 19 02:31:36 AM UTC 2024
    /// you can view this block in https://explorer.nervos.org/block/0xd07c7f0fcea6e2a20a51dabbf0caae7ebff49723bcd8bac81a8b8c021a546a32
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0xd07c7f0fcea6e2a20a51dabbf0caae7ebff49723bcd8bac81a8b8c021a546a32";
}
/// sync config related to testnet
pub mod testnet {
    /// Default assume valid target for testnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in testnet: the 60 days ago block is:
    /// height: 13632642
    /// hash: 0xab6f81474ccbc1cdd5f42cda7029a6f6e52fc242c3487861e8abb62b6a6ea8a9
    /// date: Wed Jun 19 02:32:01 AM UTC 2024
    /// you can view this block in https://pudge.explorer.nervos.org/block/0xab6f81474ccbc1cdd5f42cda7029a6f6e52fc242c3487861e8abb62b6a6ea8a9
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0xab6f81474ccbc1cdd5f42cda7029a6f6e52fc242c3487861e8abb62b6a6ea8a9";
}
