use crate::{
    bytes::Bytes,
    core::{self, Capacity},
    packed,
    prelude::*,
    H256, U256,
};

impl Pack<packed::Uint64> for Capacity {
    fn pack(&self) -> packed::Uint64 {
        self.as_u64().pack()
    }
}

impl<'r> Unpack<core::Capacity> for packed::Uint64Reader<'r> {
    fn unpack(&self) -> core::Capacity {
        Capacity::shannons(self.unpack())
    }
}
impl_conversion_for_entity_unpack!(Capacity, Uint64);

impl Pack<packed::Uint256> for U256 {
    fn pack(&self) -> packed::Uint256 {
        packed::Uint256::from_slice(&self.to_le_bytes()[..]).expect("impossible: fail to pack U256")
    }
}

impl<'r> Unpack<U256> for packed::Uint256Reader<'r> {
    fn unpack(&self) -> U256 {
        U256::from_little_endian(self.as_slice()).expect("internal error: fail to unpack U256")
    }
}
impl_conversion_for_entity_unpack!(U256, Uint256);

impl Pack<packed::Byte32> for H256 {
    fn pack(&self) -> packed::Byte32 {
        packed::Byte32::from_slice(self.as_bytes()).expect("impossible: fail to pack H256")
    }
}

impl<'r> Unpack<H256> for packed::Byte32Reader<'r> {
    fn unpack(&self) -> H256 {
        H256::from_slice(self.as_slice()).expect("internal error: fail to unpack H256")
    }
}
impl_conversion_for_entity_unpack!(H256, Byte32);

impl Pack<packed::Byte32> for [u8; 32] {
    fn pack(&self) -> packed::Byte32 {
        packed::Byte32::from_slice(&self[..]).expect("impossible: fail to pack [u8; 32]")
    }
}

impl<'r> Unpack<[u8; 32]> for packed::Byte32Reader<'r> {
    fn unpack(&self) -> [u8; 32] {
        let ptr = self.as_slice().as_ptr() as *const [u8; 32];
        unsafe { *ptr }
    }
}
impl_conversion_for_entity_unpack!([u8; 32], Byte32);

impl Pack<packed::ProposalShortId> for [u8; 10] {
    fn pack(&self) -> packed::ProposalShortId {
        packed::ProposalShortId::from_slice(&self[..])
            .expect("impossible: fail to pack to ProposalShortId")
    }
}

impl<'r> Unpack<[u8; 10]> for packed::ProposalShortIdReader<'r> {
    fn unpack(&self) -> [u8; 10] {
        let ptr = self.as_slice().as_ptr() as *const [u8; 10];
        unsafe { *ptr }
    }
}
impl_conversion_for_entity_unpack!([u8; 10], ProposalShortId);

impl Pack<packed::Bytes> for Bytes {
    fn pack(&self) -> packed::Bytes {
        let len = (self.len() as u32).to_le_bytes();
        let mut v = Vec::with_capacity(4 + self.len());
        v.extend_from_slice(&len[..]);
        v.extend_from_slice(&self[..]);
        packed::Bytes::new_unchecked(v.into())
    }
}

impl<'r> Unpack<Bytes> for packed::BytesReader<'r> {
    fn unpack(&self) -> Bytes {
        Bytes::from(self.raw_data().to_owned())
    }
}

impl Unpack<Bytes> for packed::Bytes {
    fn unpack(&self) -> Bytes {
        self.raw_data()
    }
}

impl Pack<packed::Uint64> for core::EpochNumberWithFraction {
    fn pack(&self) -> packed::Uint64 {
        self.full_value().pack()
    }
}

impl<'r> Unpack<core::EpochNumberWithFraction> for packed::Uint64Reader<'r> {
    fn unpack(&self) -> core::EpochNumberWithFraction {
        core::EpochNumberWithFraction::from_full_value(self.unpack())
    }
}
impl_conversion_for_entity_unpack!(core::EpochNumberWithFraction, Uint64);

impl_conversion_for_option!(H256, Byte32Opt, Byte32OptReader);
impl_conversion_for_vector!(Capacity, Uint64Vec, Uint64VecReader);
impl_conversion_for_vector!(Bytes, BytesVec, BytesVecReader);
impl_conversion_for_packed_optional_pack!(TransactionPoint, TransactionPointOpt);
impl_conversion_for_packed_optional_pack!(Byte32, Byte32Opt);
impl_conversion_for_packed_optional_pack!(CellOutput, CellOutputOpt);
impl_conversion_for_packed_optional_pack!(Script, ScriptOpt);
impl_conversion_for_packed_iterator_pack!(ProposalShortId, ProposalShortIdVec);
impl_conversion_for_packed_iterator_pack!(Bytes, BytesVec);
impl_conversion_for_packed_iterator_pack!(Transaction, TransactionVec);
impl_conversion_for_packed_iterator_pack!(OutPoint, OutPointVec);
impl_conversion_for_packed_iterator_pack!(CellDep, CellDepVec);
impl_conversion_for_packed_iterator_pack!(CellOutput, CellOutputVec);
impl_conversion_for_packed_iterator_pack!(CellInput, CellInputVec);
impl_conversion_for_packed_iterator_pack!(UncleBlock, UncleBlockVec);
impl_conversion_for_packed_iterator_pack!(Header, HeaderVec);
impl_conversion_for_packed_iterator_pack!(Byte32, Byte32Vec);
