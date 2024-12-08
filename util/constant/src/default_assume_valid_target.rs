/// Get default assume valid targets

/// mainnet
pub mod mainnet {
    use crate::latest_assume_valid_target;

    /// get mainnet related default assume valid targets
    pub fn default_assume_valid_targets() -> Vec<&'static str> {
        vec![
            //
            // height: 500,000; https://explorer.nervos.org/block/0xb72f4d9758a36a2f9d4b8aea5a11d232e3e48332b76ec350f0a375fac10317a4
            "0xb72f4d9758a36a2f9d4b8aea5a11d232e3e48332b76ec350f0a375fac10317a4",
            //
            // height: 1,000,000; https://explorer.nervos.org/block/0x7544e2a9db2054fbe42215ece2e5d31f175972cfeccaa7597c8ff3ec5c8b7d67
            "0x7544e2a9db2054fbe42215ece2e5d31f175972cfeccaa7597c8ff3ec5c8b7d67",
            //
            // height: 2,000,000; https://explorer.nervos.org/block/0xc0c1ca7dcfa5862b9d2afeb5ea94db14744b8146c9005982879030f01e1f47cb
            "0xc0c1ca7dcfa5862b9d2afeb5ea94db14744b8146c9005982879030f01e1f47cb",
            //
            // height: 4,000,000; https://explorer.nervos.org/block/0xcd925c9baa8c3110980546c916dad122dc69111780e49b50c3bb407ab7b6aa1c
            "0xcd925c9baa8c3110980546c916dad122dc69111780e49b50c3bb407ab7b6aa1c",
            //
            // height: 8,000,000; https://explorer.nervos.org/block/0x063ccfcdbad01922792914f0bd61e47930bbb4a531f711013a24210638c0174a
            "0x063ccfcdbad01922792914f0bd61e47930bbb4a531f711013a24210638c0174a",
            latest_assume_valid_target::mainnet::DEFAULT_ASSUME_VALID_TARGET,
        ]
    }
}

/// testnet
pub mod testnet {
    use crate::latest_assume_valid_target;

    /// get testnet related default assume valid targets
    pub fn default_assume_valid_targets() -> Vec<&'static str> {
        vec![
            //
            // height: 500,000; https://pudge.explorer.nervos.org/block/0xf9c73f3db9a7c6707c3c6800a9a0dbd5a2edf69e3921832f65275dcd71f7871c
            "0xf9c73f3db9a7c6707c3c6800a9a0dbd5a2edf69e3921832f65275dcd71f7871c",
            //
            // height: 1,000,000; https://pudge.explorer.nervos.org/block/0x935a48f2660fd141121114786edcf17ef5789c6c2fe7aca04ea27813b30e1fa3
            "0x935a48f2660fd141121114786edcf17ef5789c6c2fe7aca04ea27813b30e1fa3",
            //
            // height: 2,000,000; https://pudge.explorer.nervos.org/block/0xf4d1648131b7bc4a0c9dbc442d240395c89a0c77b0cc197dce8794cd93669b32
            "0xf4d1648131b7bc4a0c9dbc442d240395c89a0c77b0cc197dce8794cd93669b32",
            //
            // height: 4,000,000; https://pudge.explorer.nervos.org/block/0xb33c0e0a649003ab65062e93a3126a2235f6e7c3ca1b16fe9938816d846bb14f
            "0xb33c0e0a649003ab65062e93a3126a2235f6e7c3ca1b16fe9938816d846bb14f",
            //
            // height: 8,000,000; https://pudge.explorer.nervos.org/block/0x2be0f327e78032f495f90da159883da84f2efd5025fde106a6a7590b8fca6647
            "0x2be0f327e78032f495f90da159883da84f2efd5025fde106a6a7590b8fca6647",
            latest_assume_valid_target::testnet::DEFAULT_ASSUME_VALID_TARGET,
        ]
    }
}
