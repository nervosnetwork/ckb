macro_rules! define_methods {
    ($name_struct:ident, $feature:ident, $name_getter:ident,
     $name_if_enabled:ident, $name_disable:ident, $rfc_name:literal) => {
        paste! {
            impl $name_struct {
                #[doc = "Return the first epoch number when the [" $rfc_name "](struct." $name_struct "Builder.html#structfield." $feature ") is enabled."]
                #[inline]
                pub fn $feature(&self) -> EpochNumber {
                    self.$feature
                }
                #[doc = "An alias for the method [" $feature "(&self)](#method." $feature ") to let the code to be more readable."]
                #[inline]
                pub fn $name_getter(&self) -> EpochNumber {
                    self.$feature
                }
                #[doc = "If the [" $rfc_name "](struct." $name_struct "Builder.html#structfield." $feature ") is enabled at the provided epoch."]
                #[inline]
                pub fn $name_if_enabled(&self, epoch_number: EpochNumber) -> bool {
                    epoch_number >= self.$feature
                }
            }

            impl [< $name_struct Builder >] {
                #[doc = "Set the first epoch number of the [" $rfc_name "](struct." $name_struct "Builder.html#structfield." $feature ")."]
                #[inline]
                pub fn $feature(mut self, epoch_number: EpochNumber) -> Self {
                    self.$feature = Some(epoch_number);
                    self
                }
                #[doc = "Never enable the [" $rfc_name "](struct." $name_struct "Builder.html#structfield." $feature ")."]
                #[inline]
                pub fn $name_disable(mut self) -> Self {
                    self.$feature = Some(EpochNumber::MAX);
                    self
                }
            }
        }
    }
}

macro_rules! try_find {
    ($self:ident, $feature:ident) => {
        $self.$feature.ok_or_else(|| {
            concat!("The feature ", stringify!($feature), " isn't configured.").to_owned()
        })?
    };
}
