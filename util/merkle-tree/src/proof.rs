use hasher::Hasher;
use lower_leafs_count;

pub struct Proof<T> {
    /// a partial leafs collection keeps the elements sorted based on index
    pub leafs: Vec<(T, usize)>,
    /// lemmas (without root)
    pub lemmas: Vec<T>,
    pub root: Option<T>,
    /// total leafs count
    pub leafs_count: usize,
}

impl<T: Clone + PartialEq> Proof<T> {
    /// Returns true if the proof is valid
    /// # Examples
    /// ```
    /// use merkle_tree::{Hasher, Proof};
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
    /// let proof = Proof {
    ///     leafs: vec![(3, 1)],
    ///     lemmas: vec![2, 5, 18],
    ///     root: Some(28),
    ///     leafs_count: 5,
    /// };
    /// assert!(proof.validate(&SumHasher));
    ///
    /// let proof = Proof {
    ///     leafs: vec![(2, 0), (11, 4)],
    ///     lemmas: vec![3, 5, 7],
    ///     root: Some(28),
    ///     leafs_count: 5,
    /// };
    /// assert!(proof.validate(&SumHasher));
    /// ```
    pub fn validate<H: Hasher<Item = T>>(&self, hasher: &H) -> bool {
        let mid = lower_leafs_count(self.leafs_count);
        let split = match self.leafs.binary_search_by_key(&mid, |&(_, index)| index) {
            Ok(n) => n,
            Err(n) => n,
        };
        let (low, high) = self.leafs.split_at(split);

        let mut lemmas_counter = 0;
        let mut nodes = self.calculate(low, &mut lemmas_counter, hasher);
        let offset = mid >> 1;
        nodes.extend_from_slice(
            &high
                .iter()
                .map(|&(ref item, index)| (item.clone(), index - offset))
                .collect::<Vec<_>>(),
        );
        while lemmas_counter < self.lemmas.len() || nodes.len() > 1 {
            nodes = self.calculate(&nodes, &mut lemmas_counter, hasher);
        }

        // all lemmas used and root equals
        lemmas_counter == self.lemmas.len() && nodes.last().map(|node| node.0.clone()) == self.root
    }

    fn calculate<H: Hasher<Item = T>>(
        &self,
        nodes: &[(T, usize)],
        lemmas_counter: &mut usize,
        hasher: &H,
    ) -> Vec<(T, usize)> {
        let mut result = Vec::new();
        nodes.iter().for_each(|&(ref node, index)| {
            let i = if index & 1 == 0 { index + 1 } else { index - 1 };
            if result
                .binary_search_by_key(&(i >> 1), |&(_, index)| index)
                .is_err()
            {
                if let Some(other_node) = nodes
                    .binary_search_by_key(&i, |&(_, index)| index)
                    .ok()
                    .map(|i| &nodes[i].0)
                    .or_else(|| {
                        let lemma = self.lemmas.get(*lemmas_counter);
                        *lemmas_counter += 1;
                        lemma
                    }) {
                    result.push((hasher.hash(node, other_node), i >> 1));
                }
            }
        });

        result
    }
}

#[cfg(test)]
mod tests {
    extern crate rand;

    use self::rand::distributions::Standard;
    use self::rand::{thread_rng, Rng};
    use super::*;
    use tree::Tree;
    struct SumHasher;

    impl Hasher for SumHasher {
        type Item = u32;

        fn hash(&self, node1: &Self::Item, node2: &Self::Item) -> Self::Item {
            node1.wrapping_add(*node2)
        }
    }

    #[test]
    fn empty() {
        let proof = Proof {
            leafs: vec![],
            lemmas: vec![],
            root: None,
            leafs_count: 0,
        };
        assert!(proof.validate(&SumHasher));
    }

    #[test]
    fn one() {
        let proof = Proof {
            leafs: vec![(2, 0)],
            lemmas: vec![],
            root: Some(2),
            leafs_count: 1,
        };
        assert!(proof.validate(&SumHasher));
    }

    #[test]
    fn two() {
        let proof = Proof {
            leafs: vec![(2, 0), (3, 1)],
            lemmas: vec![],
            root: Some(5),
            leafs_count: 2,
        };
        assert!(proof.validate(&SumHasher));
    }

    #[test]
    fn invalid() {
        // extra lemma
        let proof = Proof {
            leafs: vec![(3, 1)],
            lemmas: vec![2, 5, 18, 18],
            root: Some(28),
            leafs_count: 5,
        };
        assert!(!proof.validate(&SumHasher));

        // invalid lemma
        let proof = Proof {
            leafs: vec![(3, 1)],
            lemmas: vec![2, 6, 18],
            root: Some(28),
            leafs_count: 5,
        };
        assert!(!proof.validate(&SumHasher));
    }

    #[test]
    fn random() {
        let total: usize = thread_rng().gen_range(500, 1000);
        let leafs: Vec<u32> = thread_rng().sample_iter(&Standard).take(total).collect();
        let tree = Tree::build(&leafs, &SumHasher);
        let mut partial = (0..thread_rng().gen_range(50, total))
            .map(|_| thread_rng().gen_range(0, total))
            .collect::<Vec<_>>();
        partial.sort_unstable();
        partial.dedup();
        let proof = tree.gen_proof(&partial);
        assert!(proof.validate(&SumHasher));
    }
}
