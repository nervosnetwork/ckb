use crate::Result;

pub trait Merge {
    type Item;
    fn merge(left: &Self::Item, right: &Self::Item) -> Result<Self::Item>;
}
