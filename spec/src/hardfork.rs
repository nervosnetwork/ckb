//! Hard forks parameters.

use ckb_constant::hardfork::{mainnet, testnet};
use ckb_types::core::{
    hardfork::{HardForkSwitch, HardForkSwitchBuilder},
    BlockNumber, EpochNumber,
};
use serde::{Deserialize, Serialize};

/// Hard forks parameters for spec.
#[derive(Default, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HardForkConfig {
    /// Use input cell committing block timestamp as the start time for the relative timestamp in `since`.
    ///
    /// Ref: [CKB RFC 0028](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0028-change-since-relative-timestamp/0028-change-since-relative-timestamp.md)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rfc_0028: Option<EpochNumber>,
    /// Allow Multiple Cell Dep Matches When There Is No Ambiguity.
    ///
    /// Ref: [CKB RFC 0029](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0029-allow-script-multiple-matches-on-identical-code/0029-allow-script-multiple-matches-on-identical-code.md)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rfc_0029: Option<EpochNumber>,
    /// Ensure That Index Is Less Than Length In the Input Since Field Using Epoch With Fraction.
    ///
    /// Ref: [CKB RFC 0030](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0030-ensure-index-less-than-length-in-since/0030-ensure-index-less-than-length-in-since.md)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rfc_0030: Option<EpochNumber>,
    /// Add a variable length field in the block: reuse `uncles_hash` in the header as `extra_hash`.
    ///
    /// Ref: [CKB RFC 0031](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0031-variable-length-header-field/0031-variable-length-header-field.md)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rfc_0031: Option<EpochNumber>,
    /// CKB VM Version Selection.
    ///
    /// This feature include 4 parts:
    /// - CKB VM Version Selection.
    /// - CKB VM version 1.
    /// - CKB VM Syscalls 2.
    /// - P2P protocol upgrade.
    ///
    /// Ref:
    /// - [CKB RFC 0032](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0032-ckb-vm-version-selection/0032-ckb-vm-version-selection.md)
    /// - [CKB RFC 0033](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0033-ckb-vm-version-1/0033-ckb-vm-version-1.md)
    /// - [CKB RFC 0034](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0034-vm-syscalls-2/0034-vm-syscalls-2.md)
    /// - [CKB RFC 0035](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0035-ckb2021-p2p-protocol-upgrade/0035-ckb2021-p2p-protocol-upgrade.md)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rfc_0032: Option<EpochNumber>,
    /// Remove Header Deps Immature Rule.
    ///
    /// Ref: [CKB RFC 0036](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0036-remove-header-deps-immature-rule/0036-remove-header-deps-immature-rule.md)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rfc_0036: Option<EpochNumber>,
    // TODO ckb2021 update the description and the rfc link
    /// Disallow over the max dep expansion limit.
    ///
    /// Ref: CKB RFC 0038
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rfc_0038: Option<EpochNumber>,

    // TODO(light-client) update the description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rfc_tmp1: Option<BlockNumber>,
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
        b = self.update_builder_via_edition(
            b,
            mainnet::CKB2021_START_EPOCH,
            mainnet::RFC0028_START_EPOCH,
        )?;
        b = self.update_builder_for_light_client(b, mainnet::MMR_ACTIVATED_BLOCK)?;
        b.build()
    }

    /// If all parameters which have been set are correct for testnet, then
    /// sets all `None` to default values, otherwise, return an `Err`.
    pub fn complete_testnet(&self) -> Result<HardForkSwitch, String> {
        let mut b = HardForkSwitch::new_builder();
        b = self.update_builder_via_edition(
            b,
            testnet::CKB2021_START_EPOCH,
            testnet::RFC0028_START_EPOCH,
        )?;
        b = self.update_builder_for_light_client(b, testnet::MMR_ACTIVATED_BLOCK)?;
        b.build()
    }

    fn update_builder_for_edition_2021(
        &self,
        builder: HardForkSwitchBuilder,
        ckb2021: EpochNumber,
        rfc_0028_start: EpochNumber,
    ) -> Result<HardForkSwitchBuilder, String> {
        let builder = builder
            .rfc_0028(check_default!(self, rfc_0028, rfc_0028_start))
            .rfc_0029(check_default!(self, rfc_0029, ckb2021))
            .rfc_0030(check_default!(self, rfc_0030, ckb2021))
            .rfc_0031(check_default!(self, rfc_0031, ckb2021))
            .rfc_0032(check_default!(self, rfc_0032, ckb2021))
            .rfc_0036(check_default!(self, rfc_0036, ckb2021))
            .rfc_0038(check_default!(self, rfc_0038, ckb2021));
        Ok(builder)
    }

    fn update_builder_for_light_client(
        &self,
        builder: HardForkSwitchBuilder,
        mmr_activated_number: BlockNumber,
    ) -> Result<HardForkSwitchBuilder, String> {
        let builder = builder.rfc_tmp1(check_default!(self, rfc_tmp1, mmr_activated_number));
        Ok(builder)
    }

    /// Converts to a hard fork switch.
    ///
    /// Enable features which are set to `None` at the user provided epoch (or block).
    pub fn complete_with_default(&self) -> Result<HardForkSwitch, String> {
        HardForkSwitch::new_builder()
            .rfc_0028(self.rfc_0028.unwrap_or(EpochNumber::MAX))
            .rfc_0029(self.rfc_0029.unwrap_or(EpochNumber::MAX))
            .rfc_0030(self.rfc_0030.unwrap_or(EpochNumber::MAX))
            .rfc_0031(self.rfc_0031.unwrap_or(EpochNumber::MAX))
            .rfc_0032(self.rfc_0032.unwrap_or(EpochNumber::MAX))
            .rfc_0036(self.rfc_0036.unwrap_or(EpochNumber::MAX))
            .rfc_0038(self.rfc_0038.unwrap_or(EpochNumber::MAX))
            .rfc_tmp1(self.rfc_tmp1.unwrap_or(BlockNumber::MAX))
            .build()
    }
}
