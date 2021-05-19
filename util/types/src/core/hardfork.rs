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
    // TODO hardfork Remove this feature after add real hardfork features.
    rfc_0000: EpochNumber,
}

/// Builder for [`HardForkSwitch`].
///
/// [`HardForkSwitch`]:  struct.HardForkSwitch.html
#[derive(Debug, Clone, Default)]
pub struct HardForkSwitchBuilder {
    rfc_0000: Option<EpochNumber>,
}

impl HardForkSwitch {
    /// Creates a new builder to build an instance.
    pub fn new_builder() -> HardForkSwitchBuilder {
        Default::default()
    }

    /// Creates a new builder based on the current instance.
    pub fn as_builder(&self) -> HardForkSwitchBuilder {
        Self::new_builder().rfc_0000(self.rfc_0000())
    }

    /// Creates a new instance that all hard fork features are disabled forever.
    pub fn new_without_any_enabled() -> Self {
        // Use a builder to ensure all features are set manually.
        Self::new_builder().disable_rfc_0000().build().unwrap()
    }
}

define_methods!(
    rfc_0000,
    dummy_feature,
    is_dummy_feature_enabled,
    disable_rfc_0000,
    "RFC 0000"
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
        let rfc_0000 = try_find!(rfc_0000);
        Ok(HardForkSwitch { rfc_0000 })
    }
}
