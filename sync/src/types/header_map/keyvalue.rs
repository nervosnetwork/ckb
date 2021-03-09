use std::{clone, cmp, hash};

pub(crate) trait Key: cmp::Eq + hash::Hash + clone::Clone {
    fn as_slice(&self) -> &[u8];
}

pub(crate) trait Value: clone::Clone {
    fn from_slice(slice: &[u8]) -> Self;
    fn to_vec(&self) -> Vec<u8>;
}
