use crate::{bytes::Bytes, packed, prelude::*};

impl Pack<packed::Bool> for bool {
    fn pack(&self) -> packed::Bool {
        let b = if *self { 1u8 } else { 0 };
        packed::Bool::new_unchecked(Bytes::from(vec![b]))
    }
}

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
    fn unpack(&self) -> u32 {
        let mut b = [0u8; 4];
        b.copy_from_slice(self.as_slice());
        u32::from_le_bytes(b)
    }
}
impl_conversion_for_entity_unpack!(u32, Uint32);

impl<'r> Unpack<u64> for packed::Uint64Reader<'r> {
    fn unpack(&self) -> u64 {
        let mut b = [0u8; 8];
        b.copy_from_slice(self.as_slice());
        u64::from_le_bytes(b)
    }
}
impl_conversion_for_entity_unpack!(u64, Uint64);

impl<'r> Unpack<u128> for packed::Uint128Reader<'r> {
    fn unpack(&self) -> u128 {
        let mut b = [0u8; 16];
        b.copy_from_slice(self.as_slice());
        u128::from_le_bytes(b)
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

impl Pack<packed::BeUint32> for u32 {
    fn pack(&self) -> packed::BeUint32 {
        packed::BeUint32::new_unchecked(Bytes::from(self.to_be_bytes().to_vec()))
    }
}

impl Pack<packed::BeUint64> for u64 {
    fn pack(&self) -> packed::BeUint64 {
        packed::BeUint64::new_unchecked(Bytes::from(self.to_be_bytes().to_vec()))
    }
}

impl Pack<packed::BeUint32> for usize {
    fn pack(&self) -> packed::BeUint32 {
        (*self as u32).pack()
    }
}

impl<'r> Unpack<u32> for packed::BeUint32Reader<'r> {
    fn unpack(&self) -> u32 {
        let mut b = [0u8; 4];
        b.copy_from_slice(self.as_slice());
        u32::from_be_bytes(b)
    }
}
impl_conversion_for_entity_unpack!(u32, BeUint32);

impl<'r> Unpack<u64> for packed::BeUint64Reader<'r> {
    fn unpack(&self) -> u64 {
        let mut b = [0u8; 8];
        b.copy_from_slice(self.as_slice());
        u64::from_be_bytes(b)
    }
}
impl_conversion_for_entity_unpack!(u64, BeUint64);

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
    /// Converts self to a string slice.
    pub fn as_utf8(&self) -> Result<&str, ::std::str::Utf8Error> {
        ::std::str::from_utf8(self.raw_data())
    }

    /// Converts self to a string slice without checking that the string contains valid UTF-8.
    ///
    /// # Safety
    ///
    /// This function is unsafe because it does not check that the bytes passed to
    /// it are valid UTF-8. If this constraint is violated, undefined behavior
    /// results, as the rest of Rust assumes that [`&str`]s are valid UTF-8.
    pub unsafe fn as_utf8_unchecked(&self) -> &str {
        ::std::str::from_utf8_unchecked(self.raw_data())
    }

    /// Checks whether self is contains valid UTF-8 binary data.
    pub fn is_utf8(&self) -> bool {
        self.as_utf8().is_ok()
    }
}

impl Pack<packed::Bytes> for String {
    fn pack(&self) -> packed::Bytes {
        self.as_str().pack()
    }
}

impl_conversion_for_option!(bool, BoolOpt, BoolOptReader);
impl_conversion_for_vector!(u32, Uint32Vec, Uint32VecReader);
impl_conversion_for_vector!(usize, Uint32Vec, Uint32VecReader);
impl_conversion_for_vector!(u64, Uint64Vec, Uint64VecReader);
impl_conversion_for_option_pack!(&str, BytesOpt);
impl_conversion_for_option_pack!(String, BytesOpt);
impl_conversion_for_option_pack!(Bytes, BytesOpt);
