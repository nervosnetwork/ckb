/// Get default assume valid targets

/// mainnet
pub mod mainnet {
    use crate::latest_assume_valid_target;

    /// get mainnet related default assume valid targets
    pub fn default_assume_valid_targets() -> Vec<&'static str> {
        vec![
            // height: 500000; https://explorer.nervos.org/block/0xb72f4d9758a36a2f9d4b8aea5a11d232e3e48332b76ec350f0a375fac10317a4
            "0xb72f4d9758a36a2f9d4b8aea5a11d232e3e48332b76ec350f0a375fac10317a4",
            // height: 1000000; https://explorer.nervos.org/block/0x7544e2a9db2054fbe42215ece2e5d31f175972cfeccaa7597c8ff3ec5c8b7d67
            "0x7544e2a9db2054fbe42215ece2e5d31f175972cfeccaa7597c8ff3ec5c8b7d67",
            // height: 2000000; https://explorer.nervos.org/block/0xc0c1ca7dcfa5862b9d2afeb5ea94db14744b8146c9005982879030f01e1f47cb
            "0xc0c1ca7dcfa5862b9d2afeb5ea94db14744b8146c9005982879030f01e1f47cb",
            // height: 3000000; https://explorer.nervos.org/block/0x36ff0ea1100e7892367b5004a362780c14c85fc2812bb6bd511e1c3a131c3fda
            "0x36ff0ea1100e7892367b5004a362780c14c85fc2812bb6bd511e1c3a131c3fda",
            // height: 4000000; https://explorer.nervos.org/block/0xcd925c9baa8c3110980546c916dad122dc69111780e49b50c3bb407ab7b6aa1c
            "0xcd925c9baa8c3110980546c916dad122dc69111780e49b50c3bb407ab7b6aa1c",
            // height: 5000000; https://explorer.nervos.org/block/0x10898dd0307ef95e9086794ae7070d2f960725d1dd1e0800044eb8d8b2547da6
            "0x10898dd0307ef95e9086794ae7070d2f960725d1dd1e0800044eb8d8b2547da6",
            // height: 6000000; https://explorer.nervos.org/block/0x0d78219b6972c21f33350958882da3e961c2ebbddc4521bf45ee47139b331333
            "0x0d78219b6972c21f33350958882da3e961c2ebbddc4521bf45ee47139b331333",
            // height: 7000000; https://explorer.nervos.org/block/0x1c280be16bf3366cf890cd5a8c5dc4eeed8c6ddeeb988a482d7feabb3bd014c6
            "0x1c280be16bf3366cf890cd5a8c5dc4eeed8c6ddeeb988a482d7feabb3bd014c6",
            // height: 8000000; https://explorer.nervos.org/block/0x063ccfcdbad01922792914f0bd61e47930bbb4a531f711013a24210638c0174a
            "0x063ccfcdbad01922792914f0bd61e47930bbb4a531f711013a24210638c0174a",
            // height: 9000000; https://explorer.nervos.org/block/0xcf95c190a0054ce2404ad70d9befb5ec78579dd0a9ddb95776c5ac1bc5ddeed1
            "0xcf95c190a0054ce2404ad70d9befb5ec78579dd0a9ddb95776c5ac1bc5ddeed1",
            // height: 10000000; https://explorer.nervos.org/block/0xe784f617bf1e13a3ac1a564e361b7e6298364193246e11cd328243f329f3592d
            "0xe784f617bf1e13a3ac1a564e361b7e6298364193246e11cd328243f329f3592d",
            // height: 11000000; https://explorer.nervos.org/block/0xe9b97767424dd04aa65a1f7ad562b0faf8dd0fbf2a213d1586ea7969160f5996
            "0xe9b97767424dd04aa65a1f7ad562b0faf8dd0fbf2a213d1586ea7969160f5996",
            // height: 12000000; https://explorer.nervos.org/block/0x2210a9bd5a292888f79ec7547ac3ea79c731df8bfe2049934f3206cabdc07f54
            "0x2210a9bd5a292888f79ec7547ac3ea79c731df8bfe2049934f3206cabdc07f54",
            // height: 13000000; https://explorer.nervos.org/block/0xcffc6a0a1f363db8fdbe2fea916ab5cd8851dd479bc04003dab88c9379dca1d0
            "0xcffc6a0a1f363db8fdbe2fea916ab5cd8851dd479bc04003dab88c9379dca1d0",
            // height: 14000000; https://explorer.nervos.org/block/0xf283cacaa21556957b9621b8ac303a0b2c06434c26a1b53b1e590219d2c7313a
            "0xf283cacaa21556957b9621b8ac303a0b2c06434c26a1b53b1e590219d2c7313a",
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
            // height: 500000; https://testnet.explorer.nervos.org/block/0xf9c73f3db9a7c6707c3c6800a9a0dbd5a2edf69e3921832f65275dcd71f7871c
            "0xf9c73f3db9a7c6707c3c6800a9a0dbd5a2edf69e3921832f65275dcd71f7871c",
            // height: 1000000; https://testnet.explorer.nervos.org/block/0x935a48f2660fd141121114786edcf17ef5789c6c2fe7aca04ea27813b30e1fa3
            "0x935a48f2660fd141121114786edcf17ef5789c6c2fe7aca04ea27813b30e1fa3",
            // height: 2000000; https://testnet.explorer.nervos.org/block/0xf4d1648131b7bc4a0c9dbc442d240395c89a0c77b0cc197dce8794cd93669b32
            "0xf4d1648131b7bc4a0c9dbc442d240395c89a0c77b0cc197dce8794cd93669b32",
            // height: 3000000; https://testnet.explorer.nervos.org/block/0x1d1bd2a6a50d9532b7131c5d0b05c006fb354a0341a504e54eaf39b27acc620d
            "0x1d1bd2a6a50d9532b7131c5d0b05c006fb354a0341a504e54eaf39b27acc620d",
            // height: 4000000; https://testnet.explorer.nervos.org/block/0xb33c0e0a649003ab65062e93a3126a2235f6e7c3ca1b16fe9938816d846bb14f
            "0xb33c0e0a649003ab65062e93a3126a2235f6e7c3ca1b16fe9938816d846bb14f",
            // height: 5000000; https://testnet.explorer.nervos.org/block/0xff4f979d8ab597a5836c533828d5253021c05f2614470fd8a4df7724ff8ec5e1
            "0xff4f979d8ab597a5836c533828d5253021c05f2614470fd8a4df7724ff8ec5e1",
            // height: 6000000; https://testnet.explorer.nervos.org/block/0xfdb427f18e03cee68947609db1f592ee2651181528da35fb62b64d4d4d5d749a
            "0xfdb427f18e03cee68947609db1f592ee2651181528da35fb62b64d4d4d5d749a",
            // height: 7000000; https://testnet.explorer.nervos.org/block/0xf9e1c6398f524c10b358dca7e000f59992004fda68c801453ed4da06bc3c6ecc
            "0xf9e1c6398f524c10b358dca7e000f59992004fda68c801453ed4da06bc3c6ecc",
            // height: 8000000; https://testnet.explorer.nervos.org/block/0x2be0f327e78032f495f90da159883da84f2efd5025fde106a6a7590b8fca6647
            "0x2be0f327e78032f495f90da159883da84f2efd5025fde106a6a7590b8fca6647",
            // height: 9000000; https://testnet.explorer.nervos.org/block/0xba1e8db7d162445979f2c73392208b882ea01c7627a8a98be82789d6f130ce35
            "0xba1e8db7d162445979f2c73392208b882ea01c7627a8a98be82789d6f130ce35",
            // height: 10000000; https://testnet.explorer.nervos.org/block/0xf64c95cfa813e0aa1ae2e0e28af4723134263c9862979c953842511381b7d8c6
            "0xf64c95cfa813e0aa1ae2e0e28af4723134263c9862979c953842511381b7d8c6",
            // height: 11000000; https://testnet.explorer.nervos.org/block/0x0a9e4de75031163fefc5e7c0d40adadb2d7cb23eb9b1b2dae46872e921f4bcf1
            "0x0a9e4de75031163fefc5e7c0d40adadb2d7cb23eb9b1b2dae46872e921f4bcf1",
            // height: 12000000; https://testnet.explorer.nervos.org/block/0x9f24177a181798b7ad63dfc8e0b89fe0ce60c099e86743675070f428ca1037b4
            "0x9f24177a181798b7ad63dfc8e0b89fe0ce60c099e86743675070f428ca1037b4",
            // height: 13000000; https://testnet.explorer.nervos.org/block/0xc884fb5ca8cc2acddf6ce4888dc7fe0f583bb0dd4f80c5be31bed87268b1ca2f
            "0xc884fb5ca8cc2acddf6ce4888dc7fe0f583bb0dd4f80c5be31bed87268b1ca2f",
            // height: 14000000; https://testnet.explorer.nervos.org/block/0xfb7da0ff926540463e3a9168cf0cd73113c24e4692a561525554c87c62aa3475
            "0xfb7da0ff926540463e3a9168cf0cd73113c24e4692a561525554c87c62aa3475",
            latest_assume_valid_target::testnet::DEFAULT_ASSUME_VALID_TARGET,
        ]
    }
}
