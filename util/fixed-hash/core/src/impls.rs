use crate::{error::FromSliceError, H160, H256, H512, H520};

macro_rules! impl_methods {
    ($name:ident, $bytes_size:expr) => {
        impl $name {
            /// Converts `Self` to a byte slice.
            #[inline]
            pub fn as_bytes(&self) -> &[u8] {
                &self.0[..]
            }
            /// To convert the byte slice back into `Self`.
            #[inline]
            pub fn from_slice(input: &[u8]) -> Result<Self, FromSliceError> {
                if input.len() != $bytes_size {
                    Err(FromSliceError::InvalidLength(input.len()))
                } else {
                    let mut ret = Self::default();
                    ret.0[..].copy_from_slice(input);
                    Ok(ret)
                }
            }
        }
    };
}

impl_methods!(H160, 20);
impl_methods!(H256, 32);
impl_methods!(H512, 64);
impl_methods!(H520, 65);
