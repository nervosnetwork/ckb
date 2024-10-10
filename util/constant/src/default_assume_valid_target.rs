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
    /// height: 13735790
    /// hash: 0x1dc6ebf09bf066b6d4c6b9bf1ded8e4c692c55b14f98bff231a4cb26720412cd
    /// date: Sun Aug 11 07:55:39 AM CST 2024
    /// you can view this block in https://explorer.nervos.org/block/0x1dc6ebf09bf066b6d4c6b9bf1ded8e4c692c55b14f98bff231a4cb26720412cd
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0x1dc6ebf09bf066b6d4c6b9bf1ded8e4c692c55b14f98bff231a4cb26720412cd";
}
/// sync config related to testnet
pub mod testnet {
    /// Default assume valid target for testnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in testnet: the 60 days ago block is:
    /// height: 14203467
    /// hash: 0xa13450d53528d80fb5886f35386cf0119eea74cc63092c1138c38971416fe445
    /// date: Sun Aug 11 07:56:19 AM CST 2024
    /// you can view this block in https://pudge.explorer.nervos.org/block/0xa13450d53528d80fb5886f35386cf0119eea74cc63092c1138c38971416fe445
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0xa13450d53528d80fb5886f35386cf0119eea74cc63092c1138c38971416fe445";
}
