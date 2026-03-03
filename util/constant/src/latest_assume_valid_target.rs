//! The mod mainnet and mod testnet's codes are generated
//! by script: ./devtools/release/update_default_valid_target.sh
//! Please don't modify them manually.

/// sync config related to mainnet
pub mod mainnet {
    /// Default assume valid target for mainnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in mainnet: the 60 days ago block is:
    /// height: 18249961
    /// hash: 0xb940531229ebdc91119044bf29dd6e51ad26385a4241c74cde378dad8f6a593b
    /// date: Wed Dec 31 12:03:43 PM CST 2025
    /// you can view this block in https://explorer.nervos.org/block/0xb940531229ebdc91119044bf29dd6e51ad26385a4241c74cde378dad8f6a593b
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0xb940531229ebdc91119044bf29dd6e51ad26385a4241c74cde378dad8f6a593b";
}
/// sync config related to testnet
pub mod testnet {
    /// Default assume valid target for testnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in testnet: the 60 days ago block is:
    /// height: 19647988
    /// hash: 0xd27d7b237455e29f27b72c46703d40b872b56ca2d2cc84f8c3a7b9dc68bdb84b
    /// date: Thu Jan  1 10:56:12 AM CST 2026
    /// you can view this block in https://testnet.explorer.nervos.org/block/0xd27d7b237455e29f27b72c46703d40b872b56ca2d2cc84f8c3a7b9dc68bdb84b
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0xd27d7b237455e29f27b72c46703d40b872b56ca2d2cc84f8c3a7b9dc68bdb84b";
}
