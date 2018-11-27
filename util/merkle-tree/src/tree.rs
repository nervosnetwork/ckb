use hasher::{DefaultHasher, Hasher};
use lower_leafs_count;
use proof::Proof;
use std::marker::PhantomData;

/// Merkle tree is a tree in which every leaf node is labelled with the hash of a data block and
/// every non-leaf node is labelled with the cryptographic hash of the labels of its child nodes.
///
/// [Article on Wikipedia](https://en.wikipedia.org/wiki/Merkle_tree)
///
/// This implementation use `Full and Complete Binary Tree` to store the data.
///
/// ```text
///         B4
///        / \
///       /   \
///      /     \
///     B2      B3
///    / \     / \
///   B1  T3  T4  T5
///  / \
/// T1  T2
/// ```
///
/// leafs: [T1,T2,T3,T4,T5]
/// branches: [B1 = h(T1, T2), B2 = h(B1, T3), B3 = h(T4, T5), B4 = h(B2, B3)]
pub struct Tree<T, H> {
    pub leafs: Vec<T>,
    pub branches: Vec<T>,
    _phantom: PhantomData<H>,
}

impl<T: Clone, H: Hasher<Item = T>> Tree<T, H> {
    /// Build merkle tree with items and hasher
    /// # Examples
    /// ```
    /// use merkle_tree::{Hasher, Tree};
    /// struct SumHasher;
    ///
    /// impl Hasher for SumHasher {
    ///     type Item = u32;
    ///
    ///     fn hash(&self, node1: &Self::Item, node2: &Self::Item) -> Self::Item {
    ///         node1 + node2
    ///     }
    /// }
    ///
    /// let leafs = vec![2, 3, 5, 7, 11];
    /// let tree = Tree::build(&leafs, &SumHasher);
    /// assert_eq!(leafs, tree.leafs);
    /// assert_eq!(vec![5, 10, 18, 28], tree.branches);
    /// assert_eq!(Some(28), tree.root());
    /// ```
    pub fn build(items: &[T], hasher: &H) -> Self {
        let mid = lower_leafs_count(items.len());
        let (low, high) = items.split_at(mid);
        // build upper branches with low leafs
        let mut branches = Self::build_upper_branches(low, hasher);
        // build upper branches with low leafs' branches and high leafs
        let mut nodes = [&branches, high].concat();
        while nodes.len() > 1 {
            nodes = Self::build_upper_branches(&nodes, hasher);
            branches.extend_from_slice(&nodes);
        }

        Self {
            leafs: items.to_vec(),
            branches,
            _phantom: PhantomData,
        }
    }

    fn build_upper_branches(nodes: &[T], hasher: &H) -> Vec<T> {
        nodes
            .chunks(2)
            .map(|pair| hasher.hash(&pair[0], &pair[1]))
            .collect::<Vec<_>>()
    }

    /// Generate proof with partial leafs indexes
    /// # Examples
    /// ```
    /// use merkle_tree::{Hasher, Tree};
    /// struct SumHasher;
    ///
    /// impl Hasher for SumHasher {
    ///     type Item = u32;
    ///
    ///     fn hash(&self, node1: &Self::Item, node2: &Self::Item) -> Self::Item {
    ///         node1 + node2
    ///     }
    /// }
    /// let leafs = vec![2, 3, 5, 7, 11];
    /// let tree = Tree::build(&leafs, &SumHasher);
    /// let proof = tree.gen_proof(&[1]);
    /// assert_eq!(vec![(3, 1)], proof.leafs);
    /// assert_eq!(vec![2, 5, 18], proof.lemmas);
    ///
    /// let proof = tree.gen_proof(&[0, 4]);
    /// assert_eq!(vec![(2, 0), (11, 4)], proof.leafs);
    /// assert_eq!(vec![3, 5, 7], proof.lemmas);
    ///
    pub fn gen_proof(&self, leaf_indexes: &[usize]) -> Proof<T> {
        let mid = lower_leafs_count(self.leafs.len());
        let split = match leaf_indexes.binary_search(&mid) {
            Ok(n) => n,
            Err(n) => n,
        };
        let (low_leafs, high_leafs) = self.leafs.split_at(mid);
        let (low_indexes, high_indexes) = leaf_indexes.split_at(split);
        // generate lemmas with low leafs
        let mut lemmas = Vec::new();
        let mut upper_indexes = Self::gen_lemmas(&mut lemmas, low_leafs, low_indexes);
        // generate lemmas with low leafs' lemmas and high leafs
        let mut offset = mid >> 1;
        upper_indexes.extend_from_slice(
            &high_indexes
                .iter()
                .map(|&index| index - offset)
                .collect::<Vec<_>>(),
        );
        let mut nodes = [&self.branches[..offset], high_leafs].concat();
        while nodes.len() > 1 {
            upper_indexes = Self::gen_lemmas(&mut lemmas, &nodes, &upper_indexes);
            let upper = offset + (nodes.len() >> 1);
            nodes = self.branches[offset..upper].to_vec();
            offset = upper;
        }

        Proof {
            leafs: leaf_indexes
                .iter()
                .filter_map(|&index| {
                    self.leafs
                        .get(index)
                        .and_then(|leaf| Some((leaf.clone(), index)))
                }).collect::<Vec<_>>(),
            lemmas,
            root: self.root(),
            leafs_count: self.leafs.len(),
        }
    }

    // fills lemmas with node indexes and returns upper nodes index
    fn gen_lemmas(lemmas: &mut Vec<T>, nodes: &[T], indexes: &[usize]) -> Vec<usize> {
        let mut result = Vec::new();
        indexes.iter().for_each(|&index| {
            let i = if index & 1 == 0 { index + 1 } else { index - 1 };
            if !result.contains(&(i >> 1)) {
                if indexes.binary_search(&i).is_err() {
                    if let Some(node) = nodes.get(i) {
                        lemmas.push(node.clone());
                    }
                }
                result.push(i >> 1);
            }
        });
        result
    }

    pub fn root(&self) -> Option<T> {
        self.branches.last().or_else(|| self.leafs.last()).cloned()
    }
}

impl Tree<[u8; 32], DefaultHasher> {
    pub fn default(items: &[[u8; 32]]) -> Self {
        Tree::build(items, &DefaultHasher)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hash::Sha3;

    struct SumHasher;

    impl Hasher for SumHasher {
        type Item = u32;

        fn hash(&self, node1: &Self::Item, node2: &Self::Item) -> Self::Item {
            node1 + node2
        }
    }

    #[test]
    fn build_empty() {
        let leafs = vec![];
        let tree = Tree::build(&leafs, &SumHasher);
        assert!(tree.leafs.is_empty());
        assert!(tree.branches.is_empty());
        assert!(tree.root().is_none());
    }

    #[test]
    fn build_one() {
        let leafs = vec![1];
        let tree = Tree::build(&leafs, &SumHasher);
        assert_eq!(leafs, tree.leafs);
        assert!(tree.branches.is_empty());
        assert_eq!(Some(1), tree.root());
    }

    #[test]
    fn build_two() {
        let leafs: Vec<u32> = vec![1, 2];
        let tree = Tree::build(&leafs, &SumHasher);
        assert_eq!(leafs, tree.leafs);
        assert_eq!(vec![3], tree.branches);
        assert_eq!(Some(3), tree.root());
    }

    #[test]
    fn gen_empty() {
        let leafs = vec![];
        let tree = Tree::build(&leafs, &SumHasher);
        let proof = tree.gen_proof(&[0]);
        assert!(proof.leafs.is_empty());
        assert!(proof.lemmas.is_empty());
    }

    #[test]
    fn gen_one() {
        let leafs = vec![1];
        let tree = Tree::build(&leafs, &SumHasher);
        let proof = tree.gen_proof(&[0]);
        assert_eq!(vec![(1, 0)], proof.leafs);
        assert!(proof.lemmas.is_empty());
    }

    #[test]
    fn gen_two() {
        let leafs: Vec<u32> = vec![1, 2];
        let tree = Tree::build(&leafs, &SumHasher);
        let proof = tree.gen_proof(&[1]);
        assert_eq!(vec![(2, 1)], proof.leafs);
        assert_eq!(vec![1], proof.lemmas);
    }

    #[test]
    fn gen_five() {
        let leafs = vec![2, 3, 5, 7, 11];
        let tree = Tree::build(&leafs, &SumHasher);
        let proof = tree.gen_proof(&[0, 1]);
        assert_eq!(vec![(2, 0), (3, 1)], proof.leafs);
        assert_eq!(vec![5, 18], proof.lemmas);
    }

    #[test]
    fn default() {
        let leafs = vec![[0; 32], [1; 32], [2; 32], [3; 32], [4; 32]];

        let tree = Tree::default(&leafs);

        let mut b1 = [0u8; 32];
        let mut sha3 = Sha3::new_sha3_256();
        sha3.update(&leafs[0]);
        sha3.update(&leafs[1]);
        sha3.finalize(&mut b1);

        let mut b2 = [0u8; 32];
        let mut sha3 = Sha3::new_sha3_256();
        sha3.update(&b1);
        sha3.update(&leafs[2]);
        sha3.finalize(&mut b2);

        let mut b3 = [0u8; 32];
        let mut sha3 = Sha3::new_sha3_256();
        sha3.update(&leafs[3]);
        sha3.update(&leafs[4]);
        sha3.finalize(&mut b3);

        let mut b4 = [0u8; 32];
        let mut sha3 = Sha3::new_sha3_256();
        sha3.update(&b2);
        sha3.update(&b3);
        sha3.finalize(&mut b4);

        assert_eq!(vec![b1, b2, b3, b4], tree.branches);
        assert_eq!(Some(b4), tree.root());
    }
}
