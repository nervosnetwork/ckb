pub use std::collections::{HashMap as TracedHashMap, HashSet as TracedHashSet};

pub struct TracedTag;

impl TracedTag {
    #[inline]
    pub fn push(_: &str) {}
    #[inline]
    pub fn replace_last(_: &str) {}
    #[inline]
    pub fn pop() {}
}
