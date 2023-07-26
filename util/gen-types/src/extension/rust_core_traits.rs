use crate::{packed, prelude::*};

macro_rules! impl_cmp_eq_and_hash {
    ($struct:ident) => {
        impl ::core::cmp::PartialEq for packed::$struct {
            #[inline]
            fn eq(&self, other: &Self) -> bool {
                self.as_slice() == other.as_slice()
            }
        }
        impl ::core::cmp::Eq for packed::$struct {}

        impl ::core::hash::Hash for packed::$struct {
            #[inline]
            fn hash<H: ::core::hash::Hasher>(&self, state: &mut H) {
                state.write(self.as_slice())
            }
        }
    };
}

impl_cmp_eq_and_hash!(Byte32);
impl_cmp_eq_and_hash!(Uint256);
impl_cmp_eq_and_hash!(ProposalShortId);
impl_cmp_eq_and_hash!(Script);
impl_cmp_eq_and_hash!(CellDep);
impl_cmp_eq_and_hash!(OutPoint);
impl_cmp_eq_and_hash!(CellInput);
impl_cmp_eq_and_hash!(CellOutput);
impl_cmp_eq_and_hash!(Alert);
impl_cmp_eq_and_hash!(UncleBlock);
impl_cmp_eq_and_hash!(Block);
impl_cmp_eq_and_hash!(HeaderDigest);

impl ::core::cmp::Ord for packed::Byte32 {
    #[inline]
    fn cmp(&self, other: &Self) -> ::core::cmp::Ordering {
        self.as_slice().cmp(other.as_slice())
    }
}

impl ::core::cmp::PartialOrd for packed::Byte32 {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<::core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
