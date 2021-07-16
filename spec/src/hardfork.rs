//! Hard forks parameters.

use ckb_constant::hardfork::{mainnet, testnet};
use ckb_types::core::{
    hardfork::{HardForkSwitch, HardForkSwitchBuilder},
    EpochNumber,
};
use serde::{Deserialize, Serialize};

/// Hard forks parameters for spec.
#[derive(Default, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HardForkConfig {
    /// Ref: [CKB RFC 221](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0221-change-since-relative-timestamp/0221-change-since-relative-timestamp.md)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rfc_0221: Option<EpochNumber>,
    /// Ref: [CKB RFC 222](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0222-allow-script-multiple-matches-on-identical-code/0222-allow-script-multiple-matches-on-identical-code.md)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rfc_0222: Option<EpochNumber>,
    /// Ref: [CKB RFC 223](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0223-ensure-index-less-than-length-in-since/0223-ensure-index-less-than-length-in-since.md)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rfc_0223: Option<EpochNumber>,
    /// Ref: [CKB RFC 224](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0224-variable-length-header-field/0224-variable-length-header-field.md)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rfc_0224: Option<EpochNumber>,
    /// Ref: [CKB RFC 232](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0232-ckb-vm-version-selection/0232-ckb-vm-version-selection.md)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rfc_0232: Option<EpochNumber>,
    /// Ref: [CKB RFC 240](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0240-remove-header-deps-immature-rule/0240-remove-header-deps-immature-rule.md)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rfc_0240: Option<EpochNumber>,
}

macro_rules! check_default {
    ($config:ident, $feature:ident, $expected:expr) => {
        if $config.$feature.is_some() {
            let errmsg = format!(
                "Found the hard fork feature parameter \"{}\" is the chain specification file.
                Don't set any hard fork parameters for \"mainnet\" or \"testnet\".",
                stringify!($feature),
            );
            return Err(errmsg);
        } else {
            $expected
        }
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
        let builder = builder
            .rfc_0221(check_default!(self, rfc_0221, ckb2021))
            .rfc_0222(check_default!(self, rfc_0222, ckb2021))
            .rfc_0223(check_default!(self, rfc_0223, ckb2021))
            .rfc_0224(check_default!(self, rfc_0224, ckb2021))
            .rfc_0232(check_default!(self, rfc_0232, ckb2021))
            .rfc_0240(check_default!(self, rfc_0240, ckb2021));
        Ok(builder)
    }

    /// Converts to a hard fork switch.
    ///
    /// Enable features which are set to `None` at the user provided epoch.
    pub fn complete_with_default(&self, default: EpochNumber) -> Result<HardForkSwitch, String> {
        HardForkSwitch::new_builder()
            .rfc_0221(self.rfc_0221.unwrap_or(default))
            .rfc_0222(self.rfc_0222.unwrap_or(default))
            .rfc_0223(self.rfc_0223.unwrap_or(default))
            .rfc_0224(self.rfc_0224.unwrap_or(default))
            .rfc_0232(self.rfc_0232.unwrap_or(default))
            .rfc_0240(self.rfc_0240.unwrap_or(default))
            .build()
    }
}
