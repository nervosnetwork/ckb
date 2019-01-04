use crate::hash::Merge;
use std::collections::VecDeque;

/// Merkle tree is a tree in which every leaf node is labelled with the hash of a data block and
/// every non-leaf node is labelled with the cryptographic hash of the labels of its child nodes.
///
/// [Article on Wikipedia](https://en.wikipedia.org/wiki/Merkle_tree)
///
/// This implementation use `Full and Complete Binary Tree` to store the data.
///
/// ```text
///         with 6 leaves                       with 7 leaves
///
///               B0                                 B0
///              /  \                               /  \
///            /      \                           /      \
///          /          \                       /          \
///        /              \                   /              \
///       B1              B2                 B1              B2
///      /  \            /  \               /  \            /  \
///     /    \          /    \             /    \          /    \
///    /      \        /      \           /      \        /      \
///   B3      B4      TO      T1         B3      B4      B5      T0
///  /  \    /  \                       /  \    /  \    /  \
/// T2  T3  T4  T5                     T1  T2  T3  T4  T5  T6
/// ```
///
/// the two trees above can be represented as:
/// [B0, B1, B2, B3, B4, T0, T1, T2, T3, T4, T5]
/// [B0, B1, B2, B3, B4, B5, T0, T1, T2, T3, T4, T5, T6]
pub struct Tree<M>
where
    M: Merge,
{
    pub(crate) nodes: Vec<M::Item>,
}

impl<M> Tree<M>
where
    M: Merge,
    <M as Merge>::Item: Clone + Default,
{
    /// Create a merkle tree with leaves
    /// # Examples
    /// ```
    /// use merkle_tree::{Merge, Tree};
    /// struct DummyHash;
    ///
    /// impl Merge for DummyHash {
    ///     type Item = i32;
    ///
    ///     fn merge(left: &Self::Item, right: &Self::Item) -> Self::Item {
    ///         right.wrapping_sub(*left)
    ///     }
    /// }
    ///
    /// let leaves = vec![2, 3, 5, 7, 11, 13];
    /// let tree = Tree::<DummyHash>::new(&leaves);
    /// assert_eq!(vec![1, 0, 1, 2, 2, 2, 3, 5, 7, 11, 13], tree.nodes());
    /// assert_eq!(Some(1), tree.root());
    /// ```
    pub fn new(leaves: &[M::Item]) -> Self {
        let len = leaves.len();
        if len > 0 {
            let mut vec = vec![M::Item::default(); len - 1];
            vec.extend(leaves.to_vec());

            (0..len - 1)
                .rev()
                .for_each(|i| vec[i] = M::merge(&vec[(i << 1) + 1], &vec[(i << 1) + 2]));

            Self { nodes: vec }
        } else {
            Self { nodes: vec![] }
        }
    }

    /// Returns all nodes of the tree
    pub fn nodes(&self) -> &[M::Item] {
        &self.nodes
    }

    /// Returns the root of the tree, or None if it is empty.
    pub fn root(&self) -> Option<M::Item> {
        self.nodes.first().cloned()
    }

    /// Build merkle root directly without tree initialization
    pub fn build_root(leaves: &[M::Item]) -> Option<M::Item> {
        if leaves.is_empty() {
            return None;
        }

        let mut queue = VecDeque::with_capacity((leaves.len() + 1) >> 1);

        let mut iter = leaves.rchunks_exact(2);
        while let Some([leaf1, leaf2]) = iter.next() {
            queue.push_back(M::merge(leaf1, leaf2))
        }
        if let [leaf] = iter.remainder() {
            queue.push_front(leaf.clone())
        }

        while queue.len() > 1 {
            let right = queue.pop_front().unwrap();
            let left = queue.pop_front().unwrap();
            queue.push_back(M::merge(&left, &right));
        }

        queue.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::distributions::Standard;
    use rand::{thread_rng, Rng};

    struct DummyHash;

    impl Merge for DummyHash {
        type Item = i32;

        fn merge(left: &Self::Item, right: &Self::Item) -> Self::Item {
            right.wrapping_sub(*left)
        }
    }

    #[test]
    fn build_empty() {
        let leaves = vec![];
        let tree = Tree::<DummyHash>::new(&leaves);
        assert!(tree.nodes().is_empty());
        assert!(tree.root().is_none());
    }

    #[test]
    fn build_one() {
        let leaves = vec![1];
        let tree = Tree::<DummyHash>::new(&leaves);
        assert_eq!(vec![1], tree.nodes());
    }

    #[test]
    fn build_two() {
        let leaves = vec![1, 2];
        let tree = Tree::<DummyHash>::new(&leaves);
        assert_eq!(vec![1, 1, 2], tree.nodes());
    }

    #[test]
    fn build_five() {
        let leaves = vec![2, 3, 5, 7, 11];
        let tree = Tree::<DummyHash>::new(&leaves);
        assert_eq!(vec![4, -2, 2, 4, 2, 3, 5, 7, 11], tree.nodes());
    }

    #[test]
    fn build_root_directly() {
        let leaves = vec![2, 3, 5, 7, 11];
        assert_eq!(Some(4), Tree::<DummyHash>::build_root(&leaves));
    }

    #[test]
    fn random() {
        let total: usize = thread_rng().gen_range(500, 1000);
        let leaves: Vec<i32> = thread_rng().sample_iter(&Standard).take(total).collect();
        let tree = Tree::<DummyHash>::new(&leaves);
        assert_eq!(Tree::<DummyHash>::build_root(&leaves), tree.root());
    }
}
