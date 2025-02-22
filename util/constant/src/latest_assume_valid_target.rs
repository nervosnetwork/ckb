//! The mod mainnet and mod testnet's codes are generated
//! by script: ./devtools/release/update_default_valid_target.sh
//! Please don't modify them manually.


/// sync config related to mainnet
pub mod mainnet {
    /// Default assume valid target for mainnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in mainnet: the 60 days ago block is:
    /// height: 14502091
    /// hash: 0x16b0d58543751a21c2cdb7be5d7077fbbcbc2097031e8c72b24dc5bd02a492f9
    /// date: Fri Nov  8 02:26:34 PM CST 2024
    /// you can view this block in https://explorer.nervos.org/block/0x16b0d58543751a21c2cdb7be5d7077fbbcbc2097031e8c72b24dc5bd02a492f9
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0x16b0d58543751a21c2cdb7be5d7077fbbcbc2097031e8c72b24dc5bd02a492f9";
}
/// sync config related to testnet
pub mod testnet {
    /// Default assume valid target for testnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in testnet: the 60 days ago block is:
    /// height: 15166546
    /// hash: 0x4400bcbdf652827f21e86ca88d50aa0ac3340968ff19eba6b183fd95164bc8f8
    /// date: Fri Nov  8 02:27:19 PM CST 2024
    /// you can view this block in https://pudge.explorer.nervos.org/block/0x4400bcbdf652827f21e86ca88d50aa0ac3340968ff19eba6b183fd95164bc8f8
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0x4400bcbdf652827f21e86ca88d50aa0ac3340968ff19eba6b183fd95164bc8f8";
}
