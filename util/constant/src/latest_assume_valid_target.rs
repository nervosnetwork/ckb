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
    /// height: 14050088
    /// hash: 0x323642363d254e830556ba670907d4455c2aeb8a38a227da7401e92a297efede
    /// date: Wed Sep 18 08:24:48 AM CST 2024
    /// you can view this block in https://explorer.nervos.org/block/0x323642363d254e830556ba670907d4455c2aeb8a38a227da7401e92a297efede
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0x323642363d254e830556ba670907d4455c2aeb8a38a227da7401e92a297efede";
}
/// sync config related to testnet
pub mod testnet {
    /// Default assume valid target for testnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in testnet: the 60 days ago block is:
    /// height: 14613282
    /// hash: 0x9674c22c68f4b65ff97944151c39ff3f8108707b4bc86378393b3312a823db77
    /// date: Wed Sep 18 08:25:27 AM CST 2024
    /// you can view this block in https://pudge.explorer.nervos.org/block/0x9674c22c68f4b65ff97944151c39ff3f8108707b4bc86378393b3312a823db77
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0x9674c22c68f4b65ff97944151c39ff3f8108707b4bc86378393b3312a823db77";
}
