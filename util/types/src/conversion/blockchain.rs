use crate::{core, packed, prelude::*};

impl Pack<packed::Uint64> for core::EpochNumberWithFraction {
    fn pack(&self) -> packed::Uint64 {
        self.full_value().pack()
    }
}

impl From<core::EpochNumberWithFraction> for packed::Uint64 {
    fn from(value: core::EpochNumberWithFraction) -> Self {
        (&value).into()
    }
}

impl From<&core::EpochNumberWithFraction> for packed::Uint64 {
    fn from(value: &core::EpochNumberWithFraction) -> Self {
        value.full_value().into()
    }
}

impl<'r> Unpack<core::EpochNumberWithFraction> for packed::Uint64Reader<'r> {
    fn unpack(&self) -> core::EpochNumberWithFraction {
        core::EpochNumberWithFraction::from_full_value_unchecked(self.unpack())
    }
}
impl_conversion_for_entity_unpack!(core::EpochNumberWithFraction, Uint64);

impl<'r> From<packed::Uint64Reader<'r>> for core::EpochNumberWithFraction {
    fn from(value: packed::Uint64Reader<'r>) -> core::EpochNumberWithFraction {
        core::EpochNumberWithFraction::from_full_value_unchecked(value.into())
    }
}
impl_conversion_for_entity_from!(core::EpochNumberWithFraction, Uint64);
