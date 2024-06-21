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
    /// height: 12265885
    /// hash: 0x9264d3b444e765d2801d13e800adb520865523a09cd9895bdaeae2c87403fd7f
    /// date: Wed Feb 21 03:50:51 AM CET 2024
    /// you can view this block in https://explorer.nervos.org/block/0x9264d3b444e765d2801d13e800adb520865523a09cd9895bdaeae2c87403fd7f
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0x9264d3b444e765d2801d13e800adb520865523a09cd9895bdaeae2c87403fd7f";
}
/// sync config related to testnet
pub mod testnet {
    /// Default assume valid target for testnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in testnet: the 60 days ago block is:
    /// height: 12347855
    /// hash: 0x40e4479ef397e98f226b469ae1fd3d0a064433100f610fb409f0ebc49ccc284e
    /// date: Wed Feb 21 03:51:50 AM CET 2024
    /// you can view this block in https://pudge.explorer.nervos.org/block/0x40e4479ef397e98f226b469ae1fd3d0a064433100f610fb409f0ebc49ccc284e
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0x40e4479ef397e98f226b469ae1fd3d0a064433100f610fb409f0ebc49ccc284e";
}
