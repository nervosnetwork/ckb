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
    rfc_pr_0221: EpochNumber,
    rfc_pr_0222: EpochNumber,
    rfc_pr_0223: EpochNumber,
    rfc_pr_0224: EpochNumber,
    rfc_pr_0234: EpochNumber,
    rfc_pr_0237: EpochNumber,
}

/// Builder for [`HardForkSwitch`].
///
/// [`HardForkSwitch`]:  struct.HardForkSwitch.html
#[derive(Debug, Clone, Default)]
pub struct HardForkSwitchBuilder {
    // TODO ckb2021 Update all rfc numbers and fix all links, after all proposals are merged.
    /// Use the input cell creation block timestamp as start time in the
    /// "relative since timestamp".
    ///
    /// Ref: [CKB RFC xxxx](https://github.com/nervosnetwork/rfcs/tree/master/rfcs/xxxx-rfc-title)
    pub rfc_pr_0221: Option<EpochNumber>,
    /// Allow script multiple matches on identical data for type hash-type scripts.
    ///
    /// Ref: [CKB RFC xxxx](https://github.com/nervosnetwork/rfcs/tree/master/rfcs/xxxx-rfc-title)
    pub rfc_pr_0222: Option<EpochNumber>,
    /// In the "since epoch", the index should be less than length and
    /// the length should be greater than zero.
    ///
    /// Ref: [CKB RFC xxxx](https://github.com/nervosnetwork/rfcs/tree/master/rfcs/xxxx-rfc-title)
    pub rfc_pr_0223: Option<EpochNumber>,
    /// Reuse `uncles_hash` in the header as `extra_hash`.
    ///
    /// Ref: [CKB RFC xxxx](https://github.com/nervosnetwork/rfcs/tree/master/rfcs/xxxx-rfc-title)
    pub rfc_pr_0224: Option<EpochNumber>,
    /// P2P network switch.
    ///
    /// Ref: [CKB RFC xxxx](https://github.com/nervosnetwork/rfcs/tree/master/rfcs/xxxx-rfc-title)
    pub rfc_pr_0234: Option<EpochNumber>,
    /// CKB VM version selection, vm version 1 and syscalls 2.
    ///
    /// Ref: [CKB RFC xxxx](https://github.com/nervosnetwork/rfcs/tree/master/rfcs/xxxx-rfc-title)
    pub rfc_pr_0237: Option<EpochNumber>,
}

impl HardForkSwitch {
    /// Creates a new builder to build an instance.
    pub fn new_builder() -> HardForkSwitchBuilder {
        Default::default()
    }

    /// Creates a new builder based on the current instance.
    pub fn as_builder(&self) -> HardForkSwitchBuilder {
        Self::new_builder()
            .rfc_pr_0221(self.rfc_pr_0221())
            .rfc_pr_0222(self.rfc_pr_0222())
            .rfc_pr_0223(self.rfc_pr_0223())
            .rfc_pr_0224(self.rfc_pr_0224())
            .rfc_pr_0234(self.rfc_pr_0234())
            .rfc_pr_0237(self.rfc_pr_0237())
    }

    /// Creates a new instance that all hard fork features are disabled forever.
    pub fn new_without_any_enabled() -> Self {
        // Use a builder to ensure all features are set manually.
        Self::new_builder()
            .disable_rfc_pr_0221()
            .disable_rfc_pr_0222()
            .disable_rfc_pr_0223()
            .disable_rfc_pr_0224()
            .disable_rfc_pr_0234()
            .disable_rfc_pr_0237()
            .build()
            .unwrap()
    }

    /// Returns a vector of epoch numbers, and there are new features which
    /// require refrese tx-pool caches will be enabled at those epochs.
    pub fn script_result_changed_at(&self) -> Vec<EpochNumber> {
        let mut epochs = vec![self.rfc_pr_0237()];
        // In future, there could be more than one epoch,
        // we should merge the same epochs and sort all epochs.
        //epochs.sort_unstable();
        //epochs.dedup();
        epochs.retain(|&x| x != 0);
        epochs
    }
}

define_methods!(
    rfc_pr_0221,
    block_ts_as_relative_since_start,
    is_block_ts_as_relative_since_start_enabled,
    disable_rfc_pr_0221,
    "RFC PR 0221"
);
define_methods!(
    rfc_pr_0222,
    allow_multiple_matches_on_identical_data,
    is_allow_multiple_matches_on_identical_data_enabled,
    disable_rfc_pr_0222,
    "RFC PR 0222"
);
define_methods!(
    rfc_pr_0223,
    check_length_in_epoch_since,
    is_check_length_in_epoch_since_enabled,
    disable_rfc_pr_0223,
    "RFC PR 0223"
);
define_methods!(
    rfc_pr_0224,
    reuse_uncles_hash_as_extra_hash,
    is_reuse_uncles_hash_as_extra_hash_enabled,
    disable_rfc_pr_0224,
    "RFC PR 0224"
);
define_methods!(
    rfc_pr_0237,
    vm_version_1_and_syscalls_2,
    is_vm_version_1_and_syscalls_2_enabled,
    disable_rfc_pr_0237,
    "RFC PR 0237"
);

define_methods!(
    rfc_pr_0234,
    p2p_network_switch,
    is_p2p_network_switch_enabled,
    disable_rfc_pr_0234,
    "RFC PR 0234"
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
        let rfc_pr_0221 = try_find!(rfc_pr_0221);
        let rfc_pr_0222 = try_find!(rfc_pr_0222);
        let rfc_pr_0223 = try_find!(rfc_pr_0223);
        let rfc_pr_0224 = try_find!(rfc_pr_0224);
        let rfc_pr_0234 = try_find!(rfc_pr_0234);
        let rfc_pr_0237 = try_find!(rfc_pr_0237);

        if rfc_pr_0234 != rfc_pr_0237 {
            return Err(format!(
                "net fork epoch {} must eq vm fork epoch {}",
                rfc_pr_0234, rfc_pr_0237
            ));
        }

        Ok(HardForkSwitch {
            rfc_pr_0221,
            rfc_pr_0222,
            rfc_pr_0223,
            rfc_pr_0224,
            rfc_pr_0234,
            rfc_pr_0237,
        })
    }
}
