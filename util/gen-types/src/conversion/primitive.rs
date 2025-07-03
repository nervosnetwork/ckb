#[cfg(not(feature = "std"))]
use alloc::{borrow::ToOwned, str, string::String};
#[cfg(feature = "std")]
use std::str;

use crate::{bytes::Bytes, generated::packed, prelude::*, vec, vec::Vec};

impl Pack<packed::Bool> for bool {
    fn pack(&self) -> packed::Bool {
        let b = u8::from(*self);
        packed::Bool::new_unchecked(Bytes::from(vec![b]))
    }
}

impl From<bool> for packed::Bool {
    fn from(value: bool) -> Self {
        (&value).into()
    }
}

impl From<&bool> for packed::Bool {
    fn from(value: &bool) -> Self {
        let b = u8::from(*value);
        packed::Bool::new_unchecked(Bytes::from(vec![b]))
    }
}

impl<'r> From<packed::BoolReader<'r>> for bool {
    fn from(value: packed::BoolReader<'r>) -> bool {
        match value.as_slice()[0] {
            0 => false,
            1 => true,
            _ => unreachable!(),
        }
    }
}
impl_conversion_for_entity_from!(bool, Bool);
impl<'r> Unpack<bool> for packed::BoolReader<'r> {
    fn unpack(&self) -> bool {
        match self.as_slice()[0] {
            0 => false,
            1 => true,
            _ => unreachable!(),
        }
    }
}
impl_conversion_for_entity_unpack!(bool, Bool);

impl Pack<packed::Uint32> for u32 {
    fn pack(&self) -> packed::Uint32 {
        packed::Uint32::new_unchecked(Bytes::from(self.to_le_bytes().to_vec()))
    }
}

impl From<u32> for packed::Uint32 {
    fn from(value: u32) -> Self {
        (&value).into()
    }
}

impl From<&u32> for packed::Uint32 {
    fn from(value: &u32) -> Self {
        packed::Uint32::new_unchecked(Bytes::from(value.to_le_bytes().to_vec()))
    }
}

impl Pack<packed::Uint64> for u64 {
    fn pack(&self) -> packed::Uint64 {
        packed::Uint64::new_unchecked(Bytes::from(self.to_le_bytes().to_vec()))
    }
}

impl From<u64> for packed::Uint64 {
    fn from(value: u64) -> Self {
        (&value).into()
    }
}

impl From<&u64> for packed::Uint64 {
    fn from(value: &u64) -> Self {
        packed::Uint64::new_unchecked(Bytes::from(value.to_le_bytes().to_vec()))
    }
}

impl Pack<packed::Uint128> for u128 {
    fn pack(&self) -> packed::Uint128 {
        packed::Uint128::new_unchecked(Bytes::from(self.to_le_bytes().to_vec()))
    }
}

impl From<u128> for packed::Uint128 {
    fn from(value: u128) -> Self {
        (&value).into()
    }
}

impl From<&u128> for packed::Uint128 {
    fn from(value: &u128) -> Self {
        packed::Uint128::new_unchecked(Bytes::from(value.to_le_bytes().to_vec()))
    }
}

impl Pack<packed::Uint32> for usize {
    fn pack(&self) -> packed::Uint32 {
        (*self as u32).into()
    }
}

impl From<usize> for packed::Uint32 {
    fn from(value: usize) -> Self {
        (value as u32).into()
    }
}

impl From<&usize> for packed::Uint32 {
    fn from(value: &usize) -> Self {
        (*value as u32).into()
    }
}

impl<'r> From<packed::Uint32Reader<'r>> for u32 {
    fn from(value: packed::Uint32Reader<'r>) -> u32 {
        let mut b = [0u8; 4];
        b.copy_from_slice(value.as_slice());
        u32::from_le_bytes(b)
    }
}
impl_conversion_for_entity_from!(u32, Uint32);
impl<'r> Unpack<u32> for packed::Uint32Reader<'r> {
    fn unpack(&self) -> u32 {
        let mut b = [0u8; 4];
        b.copy_from_slice(self.as_slice());
        u32::from_le_bytes(b)
    }
}
impl_conversion_for_entity_unpack!(u32, Uint32);

impl<'r> From<packed::Uint64Reader<'r>> for u64 {
    fn from(value: packed::Uint64Reader<'r>) -> u64 {
        let mut b = [0u8; 8];
        b.copy_from_slice(value.as_slice());
        u64::from_le_bytes(b)
    }
}
impl_conversion_for_entity_from!(u64, Uint64);
impl<'r> Unpack<u64> for packed::Uint64Reader<'r> {
    fn unpack(&self) -> u64 {
        let mut b = [0u8; 8];
        b.copy_from_slice(self.as_slice());
        u64::from_le_bytes(b)
    }
}
impl_conversion_for_entity_unpack!(u64, Uint64);

impl<'r> From<packed::Uint128Reader<'r>> for u128 {
    fn from(value: packed::Uint128Reader<'r>) -> u128 {
        let mut b = [0u8; 16];
        b.copy_from_slice(value.as_slice());
        u128::from_le_bytes(b)
    }
}
impl_conversion_for_entity_from!(u128, Uint128);
impl<'r> Unpack<u128> for packed::Uint128Reader<'r> {
    fn unpack(&self) -> u128 {
        let mut b = [0u8; 16];
        b.copy_from_slice(self.as_slice());
        u128::from_le_bytes(b)
    }
}
impl_conversion_for_entity_unpack!(u128, Uint128);

impl<'r> From<packed::Uint32Reader<'r>> for usize {
    fn from(value: packed::Uint32Reader<'r>) -> usize {
        let x: u32 = value.into();
        x as usize
    }
}
impl_conversion_for_entity_from!(usize, Uint32);
impl<'r> Unpack<usize> for packed::Uint32Reader<'r> {
    fn unpack(&self) -> usize {
        let x: u32 = self.unpack();
        x as usize
    }
}
impl_conversion_for_entity_unpack!(usize, Uint32);

impl Pack<packed::BeUint32> for u32 {
    fn pack(&self) -> packed::BeUint32 {
        packed::BeUint32::new_unchecked(Bytes::from(self.to_be_bytes().to_vec()))
    }
}

impl From<u32> for packed::BeUint32 {
    fn from(value: u32) -> Self {
        (&value).into()
    }
}

impl From<&u32> for packed::BeUint32 {
    fn from(value: &u32) -> Self {
        packed::BeUint32::new_unchecked(Bytes::from(value.to_be_bytes().to_vec()))
    }
}

impl Pack<packed::BeUint64> for u64 {
    fn pack(&self) -> packed::BeUint64 {
        packed::BeUint64::new_unchecked(Bytes::from(self.to_be_bytes().to_vec()))
    }
}

impl From<u64> for packed::BeUint64 {
    fn from(value: u64) -> Self {
        (&value).into()
    }
}

impl From<&u64> for packed::BeUint64 {
    fn from(value: &u64) -> Self {
        packed::BeUint64::new_unchecked(Bytes::from(value.to_be_bytes().to_vec()))
    }
}

impl Pack<packed::BeUint32> for usize {
    fn pack(&self) -> packed::BeUint32 {
        (*self as u32).into()
    }
}

impl From<usize> for packed::BeUint32 {
    fn from(value: usize) -> Self {
        (value as u32).into()
    }
}

impl From<&usize> for packed::BeUint32 {
    fn from(value: &usize) -> Self {
        (*value as u32).into()
    }
}

impl<'r> From<packed::BeUint32Reader<'r>> for u32 {
    fn from(value: packed::BeUint32Reader<'r>) -> u32 {
        let mut b = [0u8; 4];
        b.copy_from_slice(value.as_slice());
        u32::from_be_bytes(b)
    }
}
impl_conversion_for_entity_from!(u32, BeUint32);
impl<'r> Unpack<u32> for packed::BeUint32Reader<'r> {
    fn unpack(&self) -> u32 {
        let mut b = [0u8; 4];
        b.copy_from_slice(self.as_slice());
        u32::from_be_bytes(b)
    }
}
impl_conversion_for_entity_unpack!(u32, BeUint32);

impl<'r> From<packed::BeUint64Reader<'r>> for u64 {
    fn from(value: packed::BeUint64Reader<'r>) -> u64 {
        let mut b = [0u8; 8];
        b.copy_from_slice(value.as_slice());
        u64::from_be_bytes(b)
    }
}
impl_conversion_for_entity_from!(u64, BeUint64);
impl<'r> Unpack<u64> for packed::BeUint64Reader<'r> {
    fn unpack(&self) -> u64 {
        let mut b = [0u8; 8];
        b.copy_from_slice(self.as_slice());
        u64::from_be_bytes(b)
    }
}
impl_conversion_for_entity_unpack!(u64, BeUint64);

impl<'r> From<packed::BeUint32Reader<'r>> for usize {
    fn from(value: packed::BeUint32Reader<'r>) -> usize {
        let x: u32 = value.into();
        x as usize
    }
}
impl_conversion_for_entity_from!(usize, BeUint32);
impl<'r> Unpack<usize> for packed::BeUint32Reader<'r> {
    fn unpack(&self) -> usize {
        let x: u32 = self.unpack();
        x as usize
    }
}
impl_conversion_for_entity_unpack!(usize, BeUint32);

impl Pack<packed::Bytes> for [u8] {
    fn pack(&self) -> packed::Bytes {
        let len = self.len();
        let mut vec: Vec<u8> = Vec::with_capacity(4 + len);
        vec.extend_from_slice(&(len as u32).to_le_bytes()[..]);
        vec.extend_from_slice(self);
        packed::Bytes::new_unchecked(Bytes::from(vec))
    }
}

impl From<&[u8]> for packed::Bytes {
    fn from(value: &[u8]) -> Self {
        let len = value.len();
        let mut vec: Vec<u8> = Vec::with_capacity(4 + len);
        vec.extend_from_slice(&(len as u32).to_le_bytes()[..]);
        vec.extend_from_slice(value);
        packed::Bytes::new_unchecked(Bytes::from(vec))
    }
}

impl<const N: usize> From<[u8; N]> for packed::Bytes {
    fn from(value: [u8; N]) -> Self {
        (&value[..]).into()
    }
}

impl<const N: usize> From<&[u8; N]> for packed::Bytes {
    fn from(value: &[u8; N]) -> Self {
        (&value[..]).into()
    }
}

impl<'r> From<packed::BytesReader<'r>> for Vec<u8> {
    fn from(value: packed::BytesReader<'r>) -> Vec<u8> {
        value.raw_data().to_owned()
    }
}
impl_conversion_for_entity_from!(Vec<u8>, Bytes);
impl<'r> Unpack<Vec<u8>> for packed::BytesReader<'r> {
    fn unpack(&self) -> Vec<u8> {
        self.raw_data().to_owned()
    }
}
impl_conversion_for_entity_unpack!(Vec<u8>, Bytes);

impl Pack<packed::Bytes> for str {
    fn pack(&self) -> packed::Bytes {
        self.as_bytes().into()
    }
}

impl From<&str> for packed::Bytes {
    fn from(value: &str) -> Self {
        value.as_bytes().into()
    }
}

impl<'r> packed::BytesReader<'r> {
    /// Converts self to a string slice.
    pub fn as_utf8(&self) -> Result<&str, str::Utf8Error> {
        str::from_utf8(self.raw_data())
    }

    /// Converts self to a string slice without checking that the string contains valid UTF-8.
    ///
    /// # Safety
    ///
    /// This function is unsafe because it does not check that the bytes passed to
    /// it are valid UTF-8. If this constraint is violated, undefined behavior
    /// results, as the rest of Rust assumes that [`&str`]s are valid UTF-8.
    pub unsafe fn as_utf8_unchecked(&self) -> &str {
        str::from_utf8_unchecked(self.raw_data())
    }

    /// Checks whether self is contains valid UTF-8 binary data.
    pub fn is_utf8(&self) -> bool {
        self.as_utf8().is_ok()
    }
}

impl Pack<packed::Bytes> for String {
    fn pack(&self) -> packed::Bytes {
        self.as_str().into()
    }
}

impl From<String> for packed::Bytes {
    fn from(value: String) -> Self {
        value.as_str().into()
    }
}

impl<'r> Unpack<Option<Vec<u64>>> for packed::Uint64VecOptReader<'r> {
    fn unpack(&self) -> Option<Vec<u64>> {
        self.to_opt().map(|x| x.unpack())
    }
}

impl<'r> From<packed::Uint64VecOptReader<'r>> for Option<Vec<u64>> {
    fn from(value: packed::Uint64VecOptReader<'r>) -> Option<Vec<u64>> {
        value.to_opt().map(|x| x.into())
    }
}

impl_conversion_for_entity_unpack!(Option<Vec<u64>>, Uint64VecOpt);

impl Pack<packed::Uint64VecOpt> for Option<Vec<u64>> {
    fn pack(&self) -> packed::Uint64VecOpt {
        if let Some(inner) = self.as_ref() {
            packed::Uint64VecOptBuilder::default()
                .set(Some(inner.pack()))
                .build()
        } else {
            packed::Uint64VecOpt::default()
        }
    }
}

impl From<Option<Vec<u64>>> for packed::Uint64VecOpt {
    fn from(value: Option<Vec<u64>>) -> Self {
        (&value).into()
    }
}

impl From<&Option<Vec<u64>>> for packed::Uint64VecOpt {
    fn from(value: &Option<Vec<u64>>) -> Self {
        if let Some(inner) = value {
            packed::Uint64VecOptBuilder::default()
                .set(Some(inner.as_slice().into()))
                .build()
        } else {
            packed::Uint64VecOpt::default()
        }
    }
}

impl_conversion_for_option!(bool, BoolOpt, BoolOptReader);
impl_conversion_for_vector!(u32, Uint32Vec, Uint32VecReader);
impl_conversion_for_vector!(usize, Uint32Vec, Uint32VecReader);
impl_conversion_for_vector!(u64, Uint64Vec, Uint64VecReader);
impl_conversion_for_option_pack!(&str, BytesOpt);
impl_conversion_for_option_pack!(String, BytesOpt);
impl_conversion_for_option_pack!(Bytes, BytesOpt);
impl_conversion_for_packed_optional_pack!(Bytes, BytesOpt);

impl_conversion_for_option_from_into!(bool, BoolOpt, BoolOptReader, Bool);
impl_conversion_for_vector_from_into!(u32, Uint32Vec, Uint32VecReader);
impl_conversion_for_vector_from_into!(usize, Uint32Vec, Uint32VecReader);
impl_conversion_for_vector_from_into!(u64, Uint64Vec, Uint64VecReader);

impl_conversion_for_option_from!(&str, BytesOpt, Bytes);
impl_conversion_for_option_from!(String, BytesOpt, Bytes);
impl_conversion_for_option_from!(Bytes, BytesOpt, Bytes);
impl_conversion_for_packed_optional_from!(Bytes, BytesOpt);
