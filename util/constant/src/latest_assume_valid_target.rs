//! The mod mainnet and mod testnet's codes are generated
//! by script: ./devtools/release/update_default_valid_target.sh
//! Please don't modify them manually.

/// sync config related to mainnet
pub mod mainnet {
    /// Default assume valid target for mainnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in mainnet: the 60 days ago block is:
    /// height: 19449388
    /// hash: 0xcbd7aa3718d41063c808710daff1ef24a61b29ecfa33812b65cee48011ec161a
    /// date: Fri May 29 11:18:56 CST 2026
    /// you can view this block in https://explorer.nervos.org/block/0xcbd7aa3718d41063c808710daff1ef24a61b29ecfa33812b65cee48011ec161a
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0xcbd7aa3718d41063c808710daff1ef24a61b29ecfa33812b65cee48011ec161a";
}
/// sync config related to testnet
pub mod testnet {
    /// Default assume valid target for testnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in testnet: the 60 days ago block is:
    /// height: 21245661
    /// hash: 0xb624f8d4a069df05775d68a31e4a81317c4fcdcd9b3d4b43bb5356ba4ed06cdc
    /// date: Fri May 29 11:19:22 CST 2026
    /// you can view this block in https://testnet.explorer.nervos.org/block/0xb624f8d4a069df05775d68a31e4a81317c4fcdcd9b3d4b43bb5356ba4ed06cdc
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0xb624f8d4a069df05775d68a31e4a81317c4fcdcd9b3d4b43bb5356ba4ed06cdc";
}
