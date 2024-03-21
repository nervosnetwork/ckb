/// The `mod mainnet` and `mod testnet`'s codes are generated
/// by script: ./devtools/release/update_default_valid_target.sh

/// sync config related to mainnet
pub mod mainnet {
    // Default assume valid target for mainnet, expect to be a block 60 days ago.
    // Need to update when CKB's new release
    // in mainnet: the 60 days ago block is:
    // height: 12001914
    // hash: 0xe10a49d0937809cea4fbdb50f69203b55184fd045cd857fd52d83918a87d8b03
    // date: Sat Jan 20 09:03:29 PM CST 2024
    // you can view this block in https://explorer.nervos.org/block/0xe10a49d0937809cea4fbdb50f69203b55184fd045cd857fd52d83918a87d8b03
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0xe10a49d0937809cea4fbdb50f69203b55184fd045cd857fd52d83918a87d8b03";
}

/// sync config related to testnet
pub mod testnet {
    // Default assume valid target for testnet, expect to be a block 60 days ago.
    // Need to update when CKB's new release
    // in testnet: the 60 days ago block is:
    // height: 12007143
    // hash: 0xbb55b030e012a4d46f0cabd8f8c54841702c3273c06ebfb1e39c6760a2a5c043
    // date: Sat Jan 20 09:04:08 PM CST 2024
    // you can view this block in https://pudge.explorer.nervos.org/block/0xbb55b030e012a4d46f0cabd8f8c54841702c3273c06ebfb1e39c6760a2a5c043
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "0xbb55b030e012a4d46f0cabd8f8c54841702c3273c06ebfb1e39c6760a2a5c043";
}
