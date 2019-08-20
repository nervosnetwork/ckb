use crate::{packed, prelude::*};

macro_rules! impl_std_cmp_eq_and_hash {
    ($struct:ident) => {
        impl PartialEq for packed::$struct {
            fn eq(&self, other: &Self) -> bool {
                self.as_slice() == other.as_slice()
            }
        }
        impl Eq for packed::$struct {}

        impl ::std::hash::Hash for packed::$struct {
            fn hash<H: ::std::hash::Hasher>(&self, state: &mut H) {
                state.write(self.as_slice())
            }
        }
    };
}

impl_std_cmp_eq_and_hash!(Byte32);
impl_std_cmp_eq_and_hash!(ProposalShortId);
impl_std_cmp_eq_and_hash!(Script);
impl_std_cmp_eq_and_hash!(CellDep);
impl_std_cmp_eq_and_hash!(OutPoint);
impl_std_cmp_eq_and_hash!(CellInput);
impl_std_cmp_eq_and_hash!(CellOutput);
impl_std_cmp_eq_and_hash!(Alert);
impl_std_cmp_eq_and_hash!(UncleBlock);
impl_std_cmp_eq_and_hash!(Block);
