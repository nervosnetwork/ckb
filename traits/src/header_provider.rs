use ckb_types::{core::HeaderView, packed::Byte32};

/// TODO(doc): @quake
pub trait HeaderProvider {
    /// TODO(doc): @quake
    fn get_header(&self, hash: &Byte32) -> Option<HeaderView>;
}
