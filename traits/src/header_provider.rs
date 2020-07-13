use ckb_types::{core::HeaderView, packed::Byte32};

pub trait HeaderProvider {
    fn get_header(&self, hash: &Byte32) -> Option<HeaderView>;
}
