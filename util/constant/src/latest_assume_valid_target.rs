//! The mod mainnet and mod testnet's codes are generated
//! by script: ./devtools/release/update_default_valid_target.sh
//! Please don't modify them manually.

/// sync config related to mainnet
pub mod mainnet {
    /// Default assume valid target for mainnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in mainnet: the 60 days ago block is:
    /// height: 16811791
    /// hash: 0x75832aee642a94a2f8cfebe566158f5a1592e8c7625a9f9daf845dbddd2cd7d7
    /// date: Fri Jul 25 02:32:46 PM CST 2025
    /// you can view this block in https://explorer.nervos.org/block/0x75832aee642a94a2f8cfebe566158f5a1592e8c7625a9f9daf845dbddd2cd7d7
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0x75832aee642a94a2f8cfebe566158f5a1592e8c7625a9f9daf845dbddd2cd7d7";
}
/// sync config related to testnet
pub mod testnet {
    /// Default assume valid target for testnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in testnet: the 60 days ago block is:
    /// height: 17962744
    /// hash: 0xecdcb715a51a8e4e428231d8969bf473e2ba914d2c58ead0a65b892c68779822
    /// date: Fri Jul 25 02:33:12 PM CST 2025
    /// you can view this block in https://pudge.explorer.nervos.org/block/0xecdcb715a51a8e4e428231d8969bf473e2ba914d2c58ead0a65b892c68779822
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0xecdcb715a51a8e4e428231d8969bf473e2ba914d2c58ead0a65b892c68779822";
}
