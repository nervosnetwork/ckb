//! Hard forks related types.

use crate::core::EpochNumber;

// Defines all methods for a feature.
macro_rules! define_methods {
    ($feature:ident, $name_getter:ident,
     $name_if_enabled:ident, $name_disable:ident, $rfc_name:literal) => {
        define_methods!(
            $feature,
            $name_getter,
            $name_if_enabled,
            $name_disable,
            concat!(
                "Return the first epoch number when the [",
                $rfc_name,
                "](struct.HardForkSwitchBuilder.html#structfield.",
                stringify!($feature),
                ") is enabled."
            ),
            concat!(
                "An alias for the method [",
                stringify!($feature),
                "(&self)](#method.",
                stringify!($feature),
                ") to let the code to be more readable."
            ),
            concat!(
                "If the [",
                $rfc_name,
                "](struct.HardForkSwitchBuilder.html#structfield.",
                stringify!($feature),
                ") is enabled at the provided epoch."
            ),
            concat!(
                "Set the first epoch number of the [",
                $rfc_name,
                "](struct.HardForkSwitchBuilder.html#structfield.",
                stringify!($feature),
                ")."
            ),
            concat!(
                "Never enable the [",
                $rfc_name,
                "](struct.HardForkSwitchBuilder.html#structfield.",
                stringify!($feature),
                ")."
            )
        );
    };
    ($feature:ident, $name_getter_alias:ident,
     $name_if_enabled:ident, $name_disable:ident,
     $comment_getter:expr,$comment_getter_alias:expr, $comment_if_enabled:expr,
     $comment_setter:expr, $comment_disable:expr) => {
        impl HardForkSwitch {
            #[doc = $comment_getter]
            #[inline]
            pub fn $feature(&self) -> EpochNumber {
                self.$feature
            }
            #[doc = $comment_getter_alias]
            #[inline]
            pub fn $name_getter_alias(&self) -> EpochNumber {
                self.$feature
            }
            #[doc = $comment_if_enabled]
            #[inline]
            pub fn $name_if_enabled(&self, epoch_number: EpochNumber) -> bool {
                epoch_number >= self.$feature
            }
        }
        impl HardForkSwitchBuilder {
            #[doc = $comment_setter]
            #[inline]
            pub fn $feature(mut self, epoch_number: EpochNumber) -> Self {
                self.$feature = Some(epoch_number);
                self
            }
            #[doc = $comment_disable]
            #[inline]
            pub fn $name_disable(mut self) -> Self {
                self.$feature = Some(EpochNumber::MAX);
                self
            }
        }
    };
}

/// A switch to select hard fork features base on the epoch number.
///
/// For safety, all fields are private and not allowed to update.
/// This structure can only be constructed by [`HardForkSwitchBuilder`].
///
/// [`HardForkSwitchBuilder`]:  struct.HardForkSwitchBuilder.html
#[derive(Debug, Clone)]
pub struct HardForkSwitch {
    rfc_0028: EpochNumber,
    rfc_0029: EpochNumber,
    rfc_0030: EpochNumber,
    rfc_0031: EpochNumber,
    rfc_0032: EpochNumber,
    rfc_0036: EpochNumber,
}

/// Builder for [`HardForkSwitch`].
///
/// [`HardForkSwitch`]:  struct.HardForkSwitch.html
#[derive(Debug, Clone, Default)]
pub struct HardForkSwitchBuilder {
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
}

impl HardForkSwitch {
    /// Creates a new builder to build an instance.
    pub fn new_builder() -> HardForkSwitchBuilder {
        Default::default()
    }

    /// Creates a new builder based on the current instance.
    pub fn as_builder(&self) -> HardForkSwitchBuilder {
        Self::new_builder()
            .rfc_0028(self.rfc_0028())
            .rfc_0029(self.rfc_0029())
            .rfc_0030(self.rfc_0030())
            .rfc_0031(self.rfc_0031())
            .rfc_0032(self.rfc_0032())
            .rfc_0036(self.rfc_0036())
    }

    /// Creates a new instance that all hard fork features are disabled forever.
    pub fn new_without_any_enabled() -> Self {
        // Use a builder to ensure all features are set manually.
        Self::new_builder()
            .disable_rfc_0028()
            .disable_rfc_0029()
            .disable_rfc_0030()
            .disable_rfc_0031()
            .disable_rfc_0032()
            .disable_rfc_0036()
            .build()
            .unwrap()
    }

    /// Returns a vector of epoch numbers, and there are new features which
    /// require refrese tx-pool caches will be enabled at those epochs.
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
    rfc_0028,
    block_ts_as_relative_since_start,
    is_block_ts_as_relative_since_start_enabled,
    disable_rfc_0028,
    "RFC PR 0028"
);
define_methods!(
    rfc_0029,
    allow_multiple_matches_on_identical_data,
    is_allow_multiple_matches_on_identical_data_enabled,
    disable_rfc_0029,
    "RFC PR 0029"
);
define_methods!(
    rfc_0030,
    check_length_in_epoch_since,
    is_check_length_in_epoch_since_enabled,
    disable_rfc_0030,
    "RFC PR 0030"
);
define_methods!(
    rfc_0031,
    reuse_uncles_hash_as_extra_hash,
    is_reuse_uncles_hash_as_extra_hash_enabled,
    disable_rfc_0031,
    "RFC PR 0031"
);
define_methods!(
    rfc_0032,
    vm_version_1_and_syscalls_2,
    is_vm_version_1_and_syscalls_2_enabled,
    disable_rfc_0032,
    "RFC PR 0032"
);
define_methods!(
    rfc_0036,
    remove_header_deps_immature_rule,
    is_remove_header_deps_immature_rule_enabled,
    disable_rfc_0036,
    "RFC PR 0036"
);

impl HardForkSwitchBuilder {
    /// Build a new [`HardForkSwitch`].
    ///
    /// Returns an error if failed at any check, for example, there maybe are some features depend
    /// on others.
    ///
    /// [`HardForkSwitch`]: struct.HardForkSwitch.html
    pub fn build(self) -> Result<HardForkSwitch, String> {
        macro_rules! try_find {
            ($feature:ident) => {
                self.$feature.ok_or_else(|| {
                    concat!("The feature ", stringify!($feature), " isn't configured.").to_owned()
                })?;
            };
        }
        let rfc_0028 = try_find!(rfc_0028);
        let rfc_0029 = try_find!(rfc_0029);
        let rfc_0030 = try_find!(rfc_0030);
        let rfc_0031 = try_find!(rfc_0031);
        let rfc_0032 = try_find!(rfc_0032);
        let rfc_0036 = try_find!(rfc_0036);

        Ok(HardForkSwitch {
            rfc_0028,
            rfc_0029,
            rfc_0030,
            rfc_0031,
            rfc_0032,
            rfc_0036,
        })
    }
}
