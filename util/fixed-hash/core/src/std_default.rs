use crate::{H160, H256, H512, H520};

macro_rules! impl_std_default_default {
    ($name:ident, $bytes_size:expr) => {
        impl ::std::default::Default for $name {
            #[inline]
            fn default() -> Self {
                $name([0u8; $bytes_size])
            }
        }
    };
}

impl_std_default_default!(H160, 20);
impl_std_default_default!(H256, 32);
impl_std_default_default!(H512, 64);
impl_std_default_default!(H520, 65);
