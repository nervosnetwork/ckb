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
    /// height: 12442141
    /// hash: 0xf6fcfc9cd76afc75fbf2dcaf8e7c6f5000f531c73d74b096e5f4411ce3019dae
    /// date: Wed Mar 13 01:39:08 AM CST 2024
    /// you can view this block in https://explorer.nervos.org/block/0xf6fcfc9cd76afc75fbf2dcaf8e7c6f5000f531c73d74b096e5f4411ce3019dae
    pub const DEFAULT_ASSUME_VALID_TARGETS: [&str; 14] = [
        "0xb72f4d9758a36a2f9d4b8aea5a11d232e3e48332b76ec350f0a375fac10317a4", // height: 500000
        "0xe67a35e9de9c64e5839a5811db5c60a65c6a429a2a1bce1b61b1b7b4e01dab92", // height: 650000
        "0xf7240ed30b325b67fa88edd4c0c1cd42ad87e74c49ecacb150a5b3f54cda7b8c", // height: 840000
        "0xa27189d16fae2af8db2604d23788a3cc2680b798be99b8d133986a3519c771e4", // height: 1090000
        "0xab0c8b0aac8860bb3f21e7521a7a951663dbba7ab8ac00e41d883f9941d94b9f", // height: 1410000
        "0x76cb2cf17de61785a49ccdc5a0b86fbd1d6f9af91e9f27c954d46565b13630fb", // height: 1830000
        "0x12739a7d45aaddac7d68b1e21ab5cb4d982a9b519f15b8ac9619750b05eb8b33", // height: 2370000
        "0x16dc5a14efdebb5540efad40ad31ee1cf3c4c183d64038563bf3489e18804521", // height: 3080000
        "0xcd925c9baa8c3110980546c916dad122dc69111780e49b50c3bb407ab7b6aa1c", // height: 4000000
        "0xf74a2525451fde4b698b8b8132b66ae0b8d37fe82e352d187e1a8aa70d4ad208", // height: 5200000
        "0x5dcdc92a4adc225b64b4f12538a80dc8db47cc6e7fd005fdacb4b9ad1cc05780", // height: 6760000
        "0x1e16b3079422c122acd30eaef0525a17f9828be5a3ceba79c72e9c7d69b75245", // height: 8780000
        "0x1ba827a0a9abd3fa4dedc3f5c636859665e49cb4b579c26ac114313c48e2afa8", // height: 11410000
        "0xf6fcfc9cd76afc75fbf2dcaf8e7c6f5000f531c73d74b096e5f4411ce3019dae", // height: 12442141
    ];
}
/// sync config related to testnet
pub mod testnet {
    /// Default assume valid target for testnet, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in testnet: the 60 days ago block is:
    /// height: 12570558
    /// hash: 0x8bcda32f1dfe6e711ddefff25bcee37a38bb7f7722e1ee76d80eb421d5d51419
    /// date: Wed Mar 13 01:40:05 AM CST 2024
    /// you can view this block in https://pudge.explorer.nervos.org/block/0x8bcda32f1dfe6e711ddefff25bcee37a38bb7f7722e1ee76d80eb421d5d51419
    pub const DEFAULT_ASSUME_VALID_TARGETS: [&str; 14] = [
        "0xf9c73f3db9a7c6707c3c6800a9a0dbd5a2edf69e3921832f65275dcd71f7871c", // height: 500000
        "0x1e9f949eccc44743f01234e06c5aa4549e6a6094290c4812e5bc64017a349346", // height: 650000
        "0xde0f5f7083ebdf1e05c65eb65caded469edaea6be8155f54ec851ff1c33db23f", // height: 840000
        "0xe0200d6c055abf18d5c4bf17fc5d035a482eba4a679013e68bf3defc1a984e7f", // height: 1090000
        "0x5b7f107911205e692faa049a0fd0a4df3431b55142f0d5735b430cd723fded3b", // height: 1410000
        "0xcc2f4e56fe30104dc76e21a3a1dfbefd7019e4c0c56275dae9ffcadb7082e3a7", // height: 1830000
        "0xac3d26d46076207f03e8069fbf5c480bd1c33d92c14c05dd7fb1a41ce7aec627", // height: 2370000
        "0xbe4a6b3454b3cd493ec37387541ef6b3989f67f996f19b3c1ab7b06c9605900f", // height: 3080000
        "0xb33c0e0a649003ab65062e93a3126a2235f6e7c3ca1b16fe9938816d846bb14f", // height: 4000000
        "0x98905db5da608ccf45b9f071122f8898916524f7141c9f71301d8d8cc21f60a6", // height: 5200000
        "0x97399fbf606d90f122115cda101805a30ab9009097dfd09badd6962d60556857", // height: 6760000
        "0x073d1436b9cdceada5159ba5f695ba51de8ba536e37d73a2c6aa6b2e5379ccdc", // height: 8780000
        "0x3495b73477a9cbeca59d69c4af8902367293fdde8b4a83a45891b7497ef8a0da", // height: 11410000
        "0x8bcda32f1dfe6e711ddefff25bcee37a38bb7f7722e1ee76d80eb421d5d51419", // height: 12570558
    ];
}
