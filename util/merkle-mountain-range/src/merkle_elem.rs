use crate::Result;

pub trait MerkleElem: Sized {
    fn merge(left: &Self, right: &Self) -> Result<Self>;
}
