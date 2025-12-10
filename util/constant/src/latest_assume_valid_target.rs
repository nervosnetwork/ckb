//! The mod mainnet and mod testnet's codes are generated
//! by script: ./devtools/release/update_default_valid_target.sh
//! Please don't modify them manually.

/// sync config related to mainnet
pub mod mainnet {
    /// Default assume valid target for mainnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in mainnet: the 60 days ago block is:
    /// height: 17504094
    /// hash: 0xeeb23a01dba88365d84d713945288eeb408fae3036e0b34b32f32b43f728aced
    /// date: Fri Oct 10 08:44:43 PM CST 2025
    /// you can view this block in https://explorer.nervos.org/block/0xeeb23a01dba88365d84d713945288eeb408fae3036e0b34b32f32b43f728aced
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0xeeb23a01dba88365d84d713945288eeb408fae3036e0b34b32f32b43f728aced";
}
/// sync config related to testnet
pub mod testnet {
    /// Default assume valid target for testnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in testnet: the 60 days ago block is:
    /// height: 18788387
    /// hash: 0x10e595bdb28b3484996dccacaca8c3077743219a01953aa29796055cc5a55ebc
    /// date: Fri Oct 10 08:44:51 PM CST 2025
    /// you can view this block in https://testnet.explorer.nervos.org/block/0x10e595bdb28b3484996dccacaca8c3077743219a01953aa29796055cc5a55ebc
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0x10e595bdb28b3484996dccacaca8c3077743219a01953aa29796055cc5a55ebc";
}
