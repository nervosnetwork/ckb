/// A trait for creating parent node.
pub trait Merge {
    type Item;
    /// Returns parent node of two nodes
    fn merge(left: &Self::Item, right: &Self::Item) -> Self::Item;
}
