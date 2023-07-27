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

impl_cmp_eq_and_hash!(Uint32);
impl_cmp_eq_and_hash!(Uint64);
impl_cmp_eq_and_hash!(Uint128);
impl_cmp_eq_and_hash!(Uint256);
impl_cmp_eq_and_hash!(Byte32);
impl_cmp_eq_and_hash!(Bytes);
impl_cmp_eq_and_hash!(BytesOpt);
impl_cmp_eq_and_hash!(ProposalShortId);
impl_cmp_eq_and_hash!(Script);
impl_cmp_eq_and_hash!(ScriptOpt);
impl_cmp_eq_and_hash!(CellDep);
impl_cmp_eq_and_hash!(OutPoint);
impl_cmp_eq_and_hash!(CellInput);
impl_cmp_eq_and_hash!(CellOutput);
impl_cmp_eq_and_hash!(Alert);
impl_cmp_eq_and_hash!(UncleBlock);
impl_cmp_eq_and_hash!(Block);
impl_cmp_eq_and_hash!(HeaderDigest);

macro_rules! impl_cmp_partial_ord {
    ($struct:ident) => {
        impl ::core::cmp::PartialOrd for packed::$struct {
            #[inline]
            fn partial_cmp(&self, other: &Self) -> Option<::core::cmp::Ordering> {
                Some(self.cmp(other))
            }
        }
    };
}

impl ::core::cmp::Ord for packed::Uint32 {
    #[inline]
    fn cmp(&self, other: &Self) -> ::core::cmp::Ordering {
        let self_val: u32 = self.unpack();
        let other_val: u32 = other.unpack();
        self_val.cmp(&other_val)
    }
}
impl_cmp_partial_ord!(Uint32);

impl ::core::cmp::Ord for packed::Uint64 {
    #[inline]
    fn cmp(&self, other: &Self) -> ::core::cmp::Ordering {
        let self_val: u64 = self.unpack();
        let other_val: u64 = other.unpack();
        self_val.cmp(&other_val)
    }
}
impl_cmp_partial_ord!(Uint64);

impl ::core::cmp::Ord for packed::Uint128 {
    #[inline]
    fn cmp(&self, other: &Self) -> ::core::cmp::Ordering {
        let self_val: u128 = self.unpack();
        let other_val: u128 = other.unpack();
        self_val.cmp(&other_val)
    }
}
impl_cmp_partial_ord!(Uint128);

#[cfg(feature = "std")]
mod std_feature_mod {
    use crate::{packed, prelude::*};
    use numext_fixed_uint::U256;

    impl ::core::cmp::Ord for packed::Uint256 {
        #[inline]
        fn cmp(&self, other: &Self) -> ::core::cmp::Ordering {
            let self_val: U256 = self.unpack();
            let other_val: U256 = other.unpack();
            self_val.cmp(&other_val)
        }
    }
    impl_cmp_partial_ord!(Uint256);
}

impl ::core::cmp::Ord for packed::Byte32 {
    #[inline]
    fn cmp(&self, other: &Self) -> ::core::cmp::Ordering {
        self.as_slice().cmp(other.as_slice())
    }
}
impl_cmp_partial_ord!(Byte32);

impl ::core::cmp::Ord for packed::Bytes {
    #[inline]
    fn cmp(&self, other: &Self) -> ::core::cmp::Ordering {
        self.as_slice().cmp(other.as_slice())
    }
}
impl_cmp_partial_ord!(Bytes);

impl ::core::cmp::Ord for packed::BytesOpt {
    #[inline]
    fn cmp(&self, other: &Self) -> ::core::cmp::Ordering {
        match (self.to_opt(), other.to_opt()) {
            (Some(bytes1), Some(bytes2)) => bytes1.cmp(&bytes2),
            (Some(_), None) => ::core::cmp::Ordering::Greater,
            (None, Some(_)) => ::core::cmp::Ordering::Less,
            (None, None) => ::core::cmp::Ordering::Equal,
        }
    }
}
impl_cmp_partial_ord!(BytesOpt);

impl ::core::cmp::Ord for packed::ProposalShortId {
    #[inline]
    fn cmp(&self, other: &Self) -> ::core::cmp::Ordering {
        self.as_slice().cmp(other.as_slice())
    }
}
impl_cmp_partial_ord!(ProposalShortId);

impl ::core::cmp::Ord for packed::Script {
    #[inline]
    fn cmp(&self, other: &Self) -> ::core::cmp::Ordering {
        let code_hash_order = self.code_hash().cmp(&other.code_hash());
        if code_hash_order != ::core::cmp::Ordering::Equal {
            return code_hash_order;
        }

        let hash_type_order = self.hash_type().cmp(&other.hash_type());
        if hash_type_order != ::core::cmp::Ordering::Equal {
            return hash_type_order;
        }

        self.args().cmp(&other.args())
    }
}
impl_cmp_partial_ord!(Script);

impl ::core::cmp::Ord for packed::ScriptOpt {
    #[inline]
    fn cmp(&self, other: &Self) -> ::core::cmp::Ordering {
        match (self.to_opt(), other.to_opt()) {
            (Some(script1), Some(script2)) => script1.cmp(&script2),
            (Some(_), None) => ::core::cmp::Ordering::Greater,
            (None, Some(_)) => ::core::cmp::Ordering::Less,
            (None, None) => ::core::cmp::Ordering::Equal,
        }
    }
}
impl_cmp_partial_ord!(ScriptOpt);

impl ::core::cmp::Ord for packed::CellDep {
    #[inline]
    fn cmp(&self, other: &Self) -> ::core::cmp::Ordering {
        let dep_type_order = self.dep_type().cmp(&other.dep_type());
        if dep_type_order != ::core::cmp::Ordering::Equal {
            return dep_type_order;
        }

        self.out_point().cmp(&other.out_point())
    }
}
impl_cmp_partial_ord!(CellDep);

impl ::core::cmp::Ord for packed::OutPoint {
    #[inline]
    fn cmp(&self, other: &Self) -> ::core::cmp::Ordering {
        let tx_hash_order = self.tx_hash().cmp(&other.tx_hash());
        if tx_hash_order != ::core::cmp::Ordering::Equal {
            return tx_hash_order;
        }

        self.index().cmp(&other.index())
    }
}
impl_cmp_partial_ord!(OutPoint);

impl ::core::cmp::Ord for packed::CellInput {
    #[inline]
    fn cmp(&self, other: &Self) -> ::core::cmp::Ordering {
        let previous_output_order = self.previous_output().cmp(&other.previous_output());
        if previous_output_order != ::core::cmp::Ordering::Equal {
            return previous_output_order;
        }

        // smaller since values are prioritized and appear earlier in the ordering
        other.since().cmp(&self.since())
    }
}
impl_cmp_partial_ord!(CellInput);

impl ::core::cmp::Ord for packed::CellOutput {
    #[inline]
    fn cmp(&self, other: &Self) -> ::core::cmp::Ordering {
        let lock_order = self.lock().cmp(&other.lock());
        if lock_order != ::core::cmp::Ordering::Equal {
            return lock_order;
        }

        let capacity_order = self.capacity().cmp(&other.capacity());
        if capacity_order != ::core::cmp::Ordering::Equal {
            return capacity_order;
        }

        self.type_().cmp(&other.type_())
    }
}
impl_cmp_partial_ord!(CellOutput);
