use crate::{H160, H256, H512, H520};

macro_rules! impl_std_convert {
    ($name:ident, $bytes_size:expr) => {
        impl ::std::convert::AsRef<[u8]> for $name {
            #[inline]
            fn as_ref(&self) -> &[u8] {
                &self.0[..]
            }
        }
        impl ::std::convert::AsMut<[u8]> for $name {
            #[inline]
            fn as_mut(&mut self) -> &mut [u8] {
                &mut self.0[..]
            }
        }
        impl ::std::convert::From<[u8; $bytes_size]> for $name {
            #[inline]
            fn from(bytes: [u8; $bytes_size]) -> Self {
                $name(bytes)
            }
        }
        impl ::std::convert::From<$name> for [u8; $bytes_size] {
            #[inline]
            fn from(hash: $name) -> Self {
                hash.0
            }
        }
    };
}

impl_std_convert!(H160, 20);
impl_std_convert!(H256, 32);
impl_std_convert!(H512, 64);
impl_std_convert!(H520, 65);
