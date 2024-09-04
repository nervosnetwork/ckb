use ckb_fixed_hash::H256;
use ckb_occupied_capacity::Capacity;
use numext_fixed_uint::U256;

use crate::{packed, prelude::*};

impl Pack<packed::Uint64> for Capacity {
    fn pack(&self) -> packed::Uint64 {
        self.as_u64().pack()
    }
}

impl From<Capacity> for packed::Uint64 {
    fn from(value: Capacity) -> Self {
        (&value).into()
    }
}

impl From<&Capacity> for packed::Uint64 {
    fn from(value: &Capacity) -> Self {
        value.as_u64().into()
    }
}

impl<'r> Unpack<Capacity> for packed::Uint64Reader<'r> {
    fn unpack(&self) -> Capacity {
        Capacity::shannons(self.unpack())
    }
}
impl_conversion_for_entity_unpack!(Capacity, Uint64);

impl<'r> From<packed::Uint64Reader<'r>> for Capacity {
    fn from(value: packed::Uint64Reader<'r>) -> Capacity {
        Capacity::shannons(value.into())
    }
}
impl_conversion_for_entity_from!(Capacity, Uint64);

impl Pack<packed::Uint256> for U256 {
    fn pack(&self) -> packed::Uint256 {
        packed::Uint256::from_slice(&self.to_le_bytes()[..]).expect("impossible: fail to pack U256")
    }
}

impl From<U256> for packed::Uint256 {
    fn from(value: U256) -> Self {
        (&value).into()
    }
}

impl From<&U256> for packed::Uint256 {
    fn from(value: &U256) -> Self {
        packed::Uint256::from_slice(&value.to_le_bytes()[..])
            .expect("impossible: fail to pack U256")
    }
}

impl<'r> Unpack<U256> for packed::Uint256Reader<'r> {
    fn unpack(&self) -> U256 {
        U256::from_little_endian(self.as_slice()).expect("internal error: fail to unpack U256")
    }
}
impl_conversion_for_entity_unpack!(U256, Uint256);

impl<'r> From<packed::Uint256Reader<'r>> for U256 {
    fn from(value: packed::Uint256Reader<'r>) -> U256 {
        U256::from_little_endian(value.as_slice()).expect("internal error: fail to unpack U256")
    }
}
impl_conversion_for_entity_from!(U256, Uint256);

impl Pack<packed::Byte32> for H256 {
    fn pack(&self) -> packed::Byte32 {
        packed::Byte32::from_slice(self.as_bytes()).expect("impossible: fail to pack H256")
    }
}

impl From<H256> for packed::Byte32 {
    fn from(value: H256) -> Self {
        (&value).into()
    }
}

impl From<&H256> for packed::Byte32 {
    fn from(value: &H256) -> Self {
        packed::Byte32::from_slice(value.as_bytes()).expect("impossible: fail to pack H256")
    }
}

impl<'r> Unpack<H256> for packed::Byte32Reader<'r> {
    fn unpack(&self) -> H256 {
        H256::from_slice(self.as_slice()).expect("internal error: fail to unpack H256")
    }
}
impl_conversion_for_entity_unpack!(H256, Byte32);

impl<'r> From<packed::Byte32Reader<'r>> for H256 {
    fn from(value: packed::Byte32Reader<'r>) -> H256 {
        H256::from_slice(value.as_slice()).expect("internal error: fail to unpack H256")
    }
}
impl_conversion_for_entity_from!(H256, Byte32);

impl_conversion_for_option!(H256, Byte32Opt, Byte32OptReader);
impl_conversion_for_vector!(Capacity, Uint64Vec, Uint64VecReader);

impl_conversion_for_option_from_into!(H256, Byte32Opt, Byte32OptReader, Byte32);
impl_conversion_for_vector_from_into!(Capacity, Uint64Vec, Uint64VecReader);
