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
    // TODO ckb2021 Update all rfc numbers and fix all links, after all proposals are merged.
    /// Ref: [CKB RFC xxxx](https://github.com/nervosnetwork/rfcs/tree/master/rfcs/xxxx-rfc-title)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rfc_pr_0221: Option<EpochNumber>,
    /// Ref: [CKB RFC xxxx](https://github.com/nervosnetwork/rfcs/tree/master/rfcs/xxxx-rfc-title)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rfc_pr_0222: Option<EpochNumber>,
    /// Ref: [CKB RFC xxxx](https://github.com/nervosnetwork/rfcs/tree/master/rfcs/xxxx-rfc-title)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rfc_pr_0223: Option<EpochNumber>,
    /// Ref: [CKB RFC xxxx](https://github.com/nervosnetwork/rfcs/tree/master/rfcs/xxxx-rfc-title)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rfc_pr_0224: Option<EpochNumber>,
    /// Ref: [CKB RFC xxxx](https://github.com/nervosnetwork/rfcs/tree/master/rfcs/xxxx-rfc-title)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rfc_pr_0234: Option<EpochNumber>,
    /// Ref: [CKB RFC xxxx](https://github.com/nervosnetwork/rfcs/tree/master/rfcs/xxxx-rfc-title)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rfc_pr_0237: Option<EpochNumber>,
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
            .rfc_pr_0221(check_default!(self, rfc_pr_0221, ckb2021))
            .rfc_pr_0222(check_default!(self, rfc_pr_0222, ckb2021))
            .rfc_pr_0223(check_default!(self, rfc_pr_0223, ckb2021))
            .rfc_pr_0224(check_default!(self, rfc_pr_0224, ckb2021))
            .rfc_pr_0234(check_default!(self, rfc_pr_0234, ckb2021))
            .rfc_pr_0237(check_default!(self, rfc_pr_0237, ckb2021));
        Ok(builder)
    }

    /// Converts to a hard fork switch.
    ///
    /// Enable features which are set to `None` at the user provided epoch.
    pub fn complete_with_default(&self, default: EpochNumber) -> Result<HardForkSwitch, String> {
        HardForkSwitch::new_builder()
            .rfc_pr_0221(self.rfc_pr_0221.unwrap_or(default))
            .rfc_pr_0222(self.rfc_pr_0222.unwrap_or(default))
            .rfc_pr_0223(self.rfc_pr_0223.unwrap_or(default))
            .rfc_pr_0224(self.rfc_pr_0224.unwrap_or(default))
            .rfc_pr_0234(self.rfc_pr_0234.unwrap_or(default))
            .rfc_pr_0237(self.rfc_pr_0237.unwrap_or(default))
            .build()
    }
}
