use crate::{packed, prelude::*};

impl_conversion_for_packed_iterator_pack!(IndexTransaction, IndexTransactionVec);
impl_conversion_for_packed_iterator_pack!(RelayTransaction, RelayTransactionVec);
impl_conversion_for_packed_iterator_pack!(Uint256, Uint256Vec);
impl_conversion_for_packed_iterator_pack!(HeaderDigest, HeaderDigestVec);
