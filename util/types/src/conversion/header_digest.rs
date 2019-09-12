use crate::{core::HeaderView, packed::HeaderDigest, prelude::*};

impl From<HeaderView> for HeaderDigest {
    fn from(header_view: HeaderView) -> Self {
        HeaderDigest::new_builder()
            .hash(header_view.hash())
            .total_difficulty(header_view.difficulty().pack())
            .build()
    }
}
