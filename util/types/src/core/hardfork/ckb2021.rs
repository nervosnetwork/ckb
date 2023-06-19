use crate::core::EpochNumber;
use ckb_constant::hardfork;
use paste::paste;

/// A switch to select hard fork features base on the epoch number.
///
/// For safety, all fields are private and not allowed to update.
/// This structure can only be constructed by [`CKB2021Builder`].
///
/// [`CKB2021Builder`]:  struct.CKB2021Builder.html
#[derive(Debug, Clone)]
pub struct CKB2021 {
    rfc_0028: EpochNumber,
    rfc_0029: EpochNumber,
    rfc_0030: EpochNumber,
    rfc_0031: EpochNumber,
    rfc_0032: EpochNumber,
    rfc_0036: EpochNumber,
    rfc_0038: EpochNumber,
}

/// Builder for [`CKB2021`].
///
/// [`CKB2021`]:  struct.CKB2021.html
#[derive(Debug, Clone, Default)]
pub struct CKB2021Builder {
    /// Use input cell committing block timestamp as the start time for the relative timestamp in `since`.
    ///
    /// Ref: [CKB RFC 0028](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0028-change-since-relative-timestamp/0028-change-since-relative-timestamp.md)
    pub rfc_0028: Option<EpochNumber>,
    /// Allow Multiple Cell Dep Matches When There Is No Ambiguity.
    ///
    /// Ref: [CKB RFC 0029](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0029-allow-script-multiple-matches-on-identical-code/0029-allow-script-multiple-matches-on-identical-code.md)
    pub rfc_0029: Option<EpochNumber>,
    /// Ensure That Index Is Less Than Length In the Input Since Field Using Epoch With Fraction.
    ///
    /// Ref: [CKB RFC 0030](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0030-ensure-index-less-than-length-in-since/0030-ensure-index-less-than-length-in-since.md)
    pub rfc_0030: Option<EpochNumber>,
    /// Add a variable length field in the block: reuse `uncles_hash` in the header as `extra_hash`.
    ///
    /// Ref: [CKB RFC 0031](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0031-variable-length-header-field/0031-variable-length-header-field.md)
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
    pub rfc_0032: Option<EpochNumber>,
    /// Remove Header Deps Immature Rule.
    ///
    /// Ref: [CKB RFC 0036](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0036-remove-header-deps-immature-rule/0036-remove-header-deps-immature-rule.md)
    pub rfc_0036: Option<EpochNumber>,
    // TODO ckb2021 update the description and the rfc link
    /// Disallow over the max dep expansion limit.
    ///
    /// Ref: CKB RFC 0038
    pub rfc_0038: Option<EpochNumber>,
}

impl CKB2021 {
    /// Creates a new builder to build an instance.
    pub fn new_builder() -> CKB2021Builder {
        Default::default()
    }

    /// Creates a new builder based on the current instance.
    pub fn as_builder(&self) -> CKB2021Builder {
        Self::new_builder()
            .rfc_0028(self.rfc_0028())
            .rfc_0029(self.rfc_0029())
            .rfc_0030(self.rfc_0030())
            .rfc_0031(self.rfc_0031())
            .rfc_0032(self.rfc_0032())
            .rfc_0036(self.rfc_0036())
            .rfc_0038(self.rfc_0038())
    }

    /// Creates a new mirana instance.
    pub fn new_mirana() -> Self {
        // Use a builder to ensure all features are set manually.
        Self::new_builder()
            .rfc_0028(hardfork::mainnet::RFC0028_START_EPOCH)
            .rfc_0029(0)
            .rfc_0030(0)
            .rfc_0031(0)
            .rfc_0032(0)
            .rfc_0036(0)
            .rfc_0038(0)
            .build()
            .unwrap()
    }

    /// Creates a new dev instance.
    pub fn new_dev_default() -> Self {
        // Use a builder to ensure all features are set manually.
        Self::new_builder()
            .rfc_0028(0)
            .rfc_0029(0)
            .rfc_0030(0)
            .rfc_0031(0)
            .rfc_0032(0)
            .rfc_0036(0)
            .rfc_0038(0)
            .build()
            .unwrap()
    }

    /// Returns a vector of epoch numbers, and there are new features which
    /// require refresh tx-pool caches will be enabled at those epochs.
    pub fn script_result_changed_at(&self) -> Vec<EpochNumber> {
        let mut epochs = vec![self.rfc_0032()];
        // In future, there could be more than one epoch,
        // we should merge the same epochs and sort all epochs.
        //epochs.sort_unstable();
        //epochs.dedup();
        epochs.retain(|&x| x != 0);
        epochs
    }
}

define_methods!(
    CKB2021,
    rfc_0028,
    block_ts_as_relative_since_start,
    is_block_ts_as_relative_since_start_enabled,
    disable_rfc_0028,
    "RFC PR 0028"
);
define_methods!(
    CKB2021,
    rfc_0029,
    allow_multiple_matches_on_identical_data,
    is_allow_multiple_matches_on_identical_data_enabled,
    disable_rfc_0029,
    "RFC PR 0029"
);
define_methods!(
    CKB2021,
    rfc_0030,
    check_length_in_epoch_since,
    is_check_length_in_epoch_since_enabled,
    disable_rfc_0030,
    "RFC PR 0030"
);
define_methods!(
    CKB2021,
    rfc_0031,
    reuse_uncles_hash_as_extra_hash,
    is_reuse_uncles_hash_as_extra_hash_enabled,
    disable_rfc_0031,
    "RFC PR 0031"
);
define_methods!(
    CKB2021,
    rfc_0032,
    vm_version_1_and_syscalls_2,
    is_vm_version_1_and_syscalls_2_enabled,
    disable_rfc_0032,
    "RFC PR 0032"
);
define_methods!(
    CKB2021,
    rfc_0036,
    remove_header_deps_immature_rule,
    is_remove_header_deps_immature_rule_enabled,
    disable_rfc_0036,
    "RFC PR 0036"
);
define_methods!(
    CKB2021,
    rfc_0038,
    disallow_over_max_dep_expansion_limit,
    is_disallow_over_max_dep_expansion_limit_enabled,
    disable_rfc_0038,
    "RFC PR 0038"
);

impl CKB2021Builder {
    /// Build a new [`CKB2021`].
    ///
    /// Returns an error if failed at any check, for example, there maybe are some features depend
    /// on others.
    ///
    /// [`CKB2021`]: struct.CKB2021.html
    pub fn build(self) -> Result<CKB2021, String> {
        let rfc_0028 = try_find!(self, rfc_0028);
        let rfc_0029 = try_find!(self, rfc_0029);
        let rfc_0030 = try_find!(self, rfc_0030);
        let rfc_0031 = try_find!(self, rfc_0031);
        let rfc_0032 = try_find!(self, rfc_0032);
        let rfc_0036 = try_find!(self, rfc_0036);
        let rfc_0038 = try_find!(self, rfc_0038);

        Ok(CKB2021 {
            rfc_0028,
            rfc_0029,
            rfc_0030,
            rfc_0031,
            rfc_0032,
            rfc_0036,
            rfc_0038,
        })
    }
}
