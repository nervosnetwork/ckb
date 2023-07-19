use crate::core;
use ckb_gen_types::{packed, prelude::*};

impl Pack<packed::Uint64> for core::EpochNumberWithFraction {
    fn pack(&self) -> packed::Uint64 {
        self.full_value().pack()
    }
}

impl<'r> Unpack<core::EpochNumberWithFraction> for packed::Uint64Reader<'r> {
    fn unpack(&self) -> core::EpochNumberWithFraction {
        core::EpochNumberWithFraction::from_full_value_unchecked(self.unpack())
    }
}
impl_conversion_for_entity_unpack!(core::EpochNumberWithFraction, Uint64);
