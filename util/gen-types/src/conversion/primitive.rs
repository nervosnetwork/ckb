use crate::{bytes::Bytes, prelude::*};

use crate::generated::packed;
#[cfg(not(feature = "std"))]
use alloc::{borrow::ToOwned, str, string::String, vec::Vec};
#[cfg(feature = "std")]
use std::str;

impl Pack<packed::Uint32> for u32 {
    fn pack(&self) -> packed::Uint32 {
        packed::Uint32::new_unchecked(Bytes::from(self.to_le_bytes().to_vec()))
    }
}

impl Pack<packed::Uint64> for u64 {
    fn pack(&self) -> packed::Uint64 {
        packed::Uint64::new_unchecked(Bytes::from(self.to_le_bytes().to_vec()))
    }
}

impl Pack<packed::Uint128> for u128 {
    fn pack(&self) -> packed::Uint128 {
        packed::Uint128::new_unchecked(Bytes::from(self.to_le_bytes().to_vec()))
    }
}

impl Pack<packed::Uint32> for usize {
    fn pack(&self) -> packed::Uint32 {
        (*self as u32).pack()
    }
}

impl<'r> Unpack<u32> for packed::Uint32Reader<'r> {
    #[allow(clippy::cast_ptr_alignment)]
    fn unpack(&self) -> u32 {
        let le = self.as_slice().as_ptr() as *const u32;
        u32::from_le(unsafe { *le })
    }
}
impl_conversion_for_entity_unpack!(u32, Uint32);

impl<'r> Unpack<u64> for packed::Uint64Reader<'r> {
    #[allow(clippy::cast_ptr_alignment)]
    fn unpack(&self) -> u64 {
        let le = self.as_slice().as_ptr() as *const u64;
        u64::from_le(unsafe { *le })
    }
}
impl_conversion_for_entity_unpack!(u64, Uint64);

impl<'r> Unpack<u128> for packed::Uint128Reader<'r> {
    #[allow(clippy::cast_ptr_alignment)]
    fn unpack(&self) -> u128 {
        let le = self.as_slice().as_ptr() as *const u128;
        u128::from_le(unsafe { *le })
    }
}
impl_conversion_for_entity_unpack!(u128, Uint128);

impl<'r> Unpack<usize> for packed::Uint32Reader<'r> {
    fn unpack(&self) -> usize {
        let x: u32 = self.unpack();
        x as usize
    }
}
impl_conversion_for_entity_unpack!(usize, Uint32);

impl Pack<packed::Bytes> for [u8] {
    fn pack(&self) -> packed::Bytes {
        let len = self.len();
        let mut vec: Vec<u8> = Vec::with_capacity(4 + len);
        vec.extend_from_slice(&(len as u32).to_le_bytes()[..]);
        vec.extend_from_slice(self);
        packed::Bytes::new_unchecked(Bytes::from(vec))
    }
}

impl<'r> Unpack<Vec<u8>> for packed::BytesReader<'r> {
    fn unpack(&self) -> Vec<u8> {
        self.raw_data().to_owned()
    }
}
impl_conversion_for_entity_unpack!(Vec<u8>, Bytes);

impl Pack<packed::Bytes> for str {
    fn pack(&self) -> packed::Bytes {
        self.as_bytes().pack()
    }
}

impl<'r> packed::BytesReader<'r> {
    pub fn as_utf8(&self) -> Result<&str, str::Utf8Error> {
        str::from_utf8(self.raw_data())
    }

    #[allow(clippy::missing_safety_doc)]
    pub unsafe fn as_utf8_unchecked(&self) -> &str {
        str::from_utf8_unchecked(self.raw_data())
    }

    pub fn is_utf8(&self) -> bool {
        self.as_utf8().is_ok()
    }
}

impl Pack<packed::Bytes> for String {
    fn pack(&self) -> packed::Bytes {
        self.as_str().pack()
    }
}

impl_conversion_for_option_pack!(&str, BytesOpt);
impl_conversion_for_option_pack!(String, BytesOpt);
impl_conversion_for_option_pack!(Bytes, BytesOpt);
