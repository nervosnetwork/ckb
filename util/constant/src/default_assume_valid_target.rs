/// sync config related to mainnet
pub mod mainnet {
    /// Default assume valid target for mainnet, expect to be a block 60days ago.
    /// Need to update when CKB's new release
    /// mainnet: the 60 days ago block is:
    /// height: 11105799
    /// hash: 0xe0775add5e6e08a4e171cc629001249c66d3a5c76d519a5fade973265c82dcd9
    /// date: in Wed Oct  4 12:55:35 PM CST 2023
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0xe0775add5e6e08a4e171cc629001249c66d3a5c76d519a5fade973265c82dcd9";
}

/// sync config related to testnet
pub mod testnet {
    /// Default assume valid target for mainnet, expect to be a block 60days ago.
    /// Need to update when CKB's new release
    /// testnet: the 60 days ago block is:
    /// height: 10838525
    /// hash: 0x56fa20718d9c51b73f744e5535d2d7612cb1a2b518c964c9919ce6a12e4e83ca
    /// date: in Wed Oct  4 12:55:35 PM CST 2023
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0x56fa20718d9c51b73f744e5535d2d7612cb1a2b518c964c9919ce6a12e4e83ca";
}
