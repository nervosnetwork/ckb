use crate::{packed, prelude::*};

macro_rules! impl_std_cmp_eq_and_hash {
    ($struct:ident) => {
        impl PartialEq for packed::$struct {
            #[inline]
            fn eq(&self, other: &Self) -> bool {
                self.as_slice() == other.as_slice()
            }
        }
        impl Eq for packed::$struct {}

        impl ::std::hash::Hash for packed::$struct {
            #[inline]
            fn hash<H: ::std::hash::Hasher>(&self, state: &mut H) {
                state.write(self.as_slice())
            }
        }
    };
}

impl_std_cmp_eq_and_hash!(Byte32);
impl_std_cmp_eq_and_hash!(Uint256);
impl_std_cmp_eq_and_hash!(ProposalShortId);
impl_std_cmp_eq_and_hash!(Script);
impl_std_cmp_eq_and_hash!(CellDep);
impl_std_cmp_eq_and_hash!(OutPoint);
impl_std_cmp_eq_and_hash!(CellInput);
impl_std_cmp_eq_and_hash!(CellOutput);
impl_std_cmp_eq_and_hash!(Alert);
impl_std_cmp_eq_and_hash!(UncleBlock);
impl_std_cmp_eq_and_hash!(Block);
impl_std_cmp_eq_and_hash!(HeaderDigest);

impl ::std::cmp::Ord for packed::Byte32 {
    #[inline]
    fn cmp(&self, other: &Self) -> ::std::cmp::Ordering {
        self.as_slice().cmp(other.as_slice())
    }
}

impl ::std::cmp::PartialOrd for packed::Byte32 {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<::std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
