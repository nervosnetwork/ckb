//! Hard forks parameters.

use ckb_constant::hardfork::{mainnet, testnet};
use ckb_types::core::{
    hardfork::{HardForkSwitch, HardForkSwitchBuilder},
    EpochNumber,
};
use serde::{Deserialize, Serialize};

/// Hard forks parameters for spec.
#[derive(Default, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct HardForkConfig {
    /// Just a dummy field to test hard fork feature.
    pub rfc_0000: Option<EpochNumber>,
}

macro_rules! check_default {
    ($config:ident, $feature:ident, $expected:expr) => {
        match $config.$feature {
            Some(input) if input != $expected => {
                let errmsg = format!(
                    "The value for hard fork feature \"{}\" is incorrect, actual: {}, expected: {}.
                    Don't set it for mainnet or testnet, or set it as a correct value.",
                    stringify!($feature),
                    input,
                    $expected,
                );
                Err(errmsg)
            },
            _ => Ok($expected),
        }?
    };
}

impl HardForkConfig {
    /// If all parameters which have been set are correct for mainnet, then
    /// sets all `None` to default values, otherwise, return an `Err`.
    pub fn complete_mainnet(&self) -> Result<HardForkSwitch, String> {
        let mut b = HardForkSwitch::new_builder();
        b = self.update_builder_via_edition(b, mainnet::CKB2021_START_EPOCH)?;
        b.build()
    }

    /// If all parameters which have been set are correct for testnet, then
    /// sets all `None` to default values, otherwise, return an `Err`.
    pub fn complete_testnet(&self) -> Result<HardForkSwitch, String> {
        let mut b = HardForkSwitch::new_builder();
        b = self.update_builder_via_edition(b, testnet::CKB2021_START_EPOCH)?;
        b.build()
    }

    fn update_builder_via_edition(
        &self,
        builder: HardForkSwitchBuilder,
        ckb2021: EpochNumber,
    ) -> Result<HardForkSwitchBuilder, String> {
        let builder = builder.rfc_0000(check_default!(self, rfc_0000, ckb2021));
        Ok(builder)
    }

    /// Converts to a hard fork switch.
    ///
    /// Enable features which are set to `None` at the user provided epoch.
    pub fn complete_with_default(&self, default: EpochNumber) -> Result<HardForkSwitch, String> {
        HardForkSwitch::new_builder()
            .rfc_0000(self.rfc_0000.unwrap_or(default))
            .build()
    }
}
