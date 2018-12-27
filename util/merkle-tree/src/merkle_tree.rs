use super::merge;
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};
use std::collections::VecDeque;

pub fn merkle_root(leaves: &[H256]) -> H256 {
    let leaves_len = leaves.len();
    // in case of empty slice, just return zero
    if leaves_len == 0 {
        return H256::zero();
    }

    let mut queue = VecDeque::with_capacity((leaves_len + 1) >> 1);

    if leaves_len & 1 == 1 {
        queue.push_back(leaves[0].clone());
    }

    let mut i = leaves_len;

    while i > 1 {
        i -= 2;
        queue.push_back(merge(&leaves[i], &leaves[i + 1]));
    }

    while queue.len() > 1 {
        let right = queue.pop_front().unwrap();
        let left = queue.pop_front().unwrap();
        queue.push_back(merge(&left, &right));
    }

    queue.pop_front().unwrap()
}

pub fn build_merkle_tree(leaves: Vec<H256>) -> Vec<H256> {
    if leaves.is_empty() {
        return vec![];
    }

    let mut nodes = vec![H256::zero(); leaves.len() - 1];
    nodes.extend(leaves);
    let mut i = nodes.len() - 1;

    while i != 0 {
        nodes[(i - 1) >> 1] = merge(&nodes[i - 1], &nodes[i]);
        i -= 2;
    }

    nodes
}

pub fn update_root(tree: &mut Vec<H256>, mut index: usize) {
    if index >= tree.len() {
        return;
    }

    while index != 0 {
        let sibling = ((index + 1) ^ 1) - 1;
        let tmp = (index - 1) >> 1;
        tree[tmp] = if sibling > index {
            merge(&tree[index], &tree[sibling])
        } else {
            merge(&tree[sibling], &tree[index])
        };
        index = tmp;
    }
}

#[derive(Eq, PartialEq, Debug, Serialize, Deserialize, Clone)]
pub struct Proof {
    // From 0
    pub index: u32,
    pub nodes: Vec<H256>,
}

impl Proof {
    pub fn root(&self, mut hash: H256) -> Option<H256> {
        let mut index = self.index;
        let len = 32 - (index + 1).leading_zeros() - 1;

        if self.nodes.len() != len as usize {
            return None;
        }

        hash = self.nodes.iter().fold(hash, |x1, x2| {
            let ret = if index & 1 == 0 {
                merge(x2, &x1)
            } else {
                merge(&x1, x2)
            };
            index = (index - 1) >> 1;
            ret
        });

        Some(hash)
    }

    pub fn verify(&self, root: &H256, hash: H256) -> bool {
        if let Some(x) = self.root(hash) {
            &x == root
        } else {
            false
        }
    }
}

pub fn build_proof(tree: &[H256], index: usize) -> Option<Proof> {
    if index >= tree.len() {
        return None;
    }
    let mut nodes = Vec::new();
    let mut curr = index;

    while curr != 0 {
        let sibling = ((curr + 1) ^ 1) - 1;
        nodes.push(tree[sibling].clone());
        curr = (curr - 1) >> 1;
    }
    let index = index as u32;
    Some(Proof { index, nodes })
}

pub fn calc_sibling(num: usize) -> usize {
    if num == 0 {
        0
    } else {
        ((num + 1) ^ 1) - 1
    }
}

pub fn calc_parent(num: usize) -> usize {
    if num == 0 {
        0
    } else {
        (num - 1) >> 1
    }
}

pub fn is_left(num: usize) -> bool {
    num & 1 == 1
}

#[derive(Eq, PartialEq, Debug, Serialize, Deserialize, Clone)]
pub struct CombinedProof {
    // size of items in the tree
    pub size: u32,
    // nodes on the path which can not be calculated, in descending order by index
    pub nodes: Vec<H256>,
}

impl CombinedProof {
    pub fn root(&self, mut tuples: Vec<(u32, H256)>) -> Option<H256> {
        let mut queue = VecDeque::new();
        tuples.sort_by(|a, b| a.0.cmp(&b.0));
        tuples.reverse();

        for (index, hash) in tuples {
            queue.push_back(((index + self.size - 1) as usize, hash));
        }

        let mut nodes_iter = self.nodes.iter();

        while !queue.is_empty() {
            let mut in_queue = false;

            let (index1, hash1) = queue.pop_front().unwrap();
            let sibling = calc_sibling(index1);

            if let Some((index2, _)) = queue.front() {
                if index2 == &sibling {
                    in_queue = true;
                }
            }

            let hash2 = if in_queue {
                queue.pop_front().unwrap().1
            } else if let Some(h) = nodes_iter.next() {
                h.clone()
            } else {
                return None;
            };

            let hash = if is_left(index1) {
                merge(&hash1, &hash2)
            } else {
                merge(&hash2, &hash1)
            };

            let parent = calc_parent(index1);

            if parent == 0 {
                return Some(hash);
            } else {
                queue.push_back((parent, hash));
            }
        }

        None
    }

    pub fn verify(&self, root: &H256, tuples: Vec<(u32, H256)>) -> bool {
        if let Some(h) = self.root(tuples) {
            &h == root
        } else {
            false
        }
    }
}

pub fn build_combined_proof(tree: &[H256], mut indexes: Vec<usize>) -> Option<CombinedProof> {
    let mut nodes = Vec::new();
    let mut queue = VecDeque::new();

    indexes.sort();
    indexes.reverse();
    let size = (tree.len() >> 1) + 1;

    for index in indexes {
        queue.push_back(index + size - 1);
    }

    while !queue.is_empty() {
        let index = queue.pop_front().unwrap();
        let sibling = calc_sibling(index);
        if Some(&sibling) == queue.front() {
            queue.pop_front();
        } else if let Some(h) = tree.get(sibling) {
            nodes.push(h.clone());
        } else {
            return None;
        }

        let parent = calc_parent(index);
        if parent != 0 {
            queue.push_back(parent);
        }
    }

    Some(CombinedProof {
        size: size as u32,
        nodes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use numext_fixed_hash::H256;
    use std::str::FromStr;

    #[test]
    fn merkle_root_test() {
        let leaves = vec![
            H256::from_str("8e827ab731f2416f6057b9c7f241b1841e345ffeabb4274e35995a45f4d42a1a")
                .unwrap(),
            H256::from_str("768dfb4ca3311fa3bf4d696dde334e30edf3542e8ea114a4f9d18fb34365f1d1")
                .unwrap(),
            H256::from_str("e68dfb4ca3311fa3bf4d696dde334e30edf3542e8ea114a4f9d18fb34365f1d1")
                .unwrap(),
            H256::from_str("f68dfb4ca3311fa3bf4d696dde334e30edf3542e8ea114a4f9d18fb34365f1d1")
                .unwrap(),
            H256::from_str("968dfb4ca3311fa3bf4d696dde334e30edf3542e8ea114a4f9d18fb34365f1d1")
                .unwrap(),
        ];
        let root = merkle_root(&leaves);
        let tree = build_merkle_tree(leaves);
        assert_eq!(root, tree[0]);
    }

    #[test]
    fn update_root_test() {
        let leaves = vec![
            H256::from_str("8e827ab731f2416f6057b9c7f241b1841e345ffeabb4274e35995a45f4d42a1a")
                .unwrap(),
            H256::from_str("768dfb4ca3311fa3bf4d696dde334e30edf3542e8ea114a4f9d18fb34365f1d1")
                .unwrap(),
            H256::from_str("e68dfb4ca3311fa3bf4d696dde334e30edf3542e8ea114a4f9d18fb34365f1d1")
                .unwrap(),
            H256::from_str("f68dfb4ca3311fa3bf4d696dde334e30edf3542e8ea114a4f9d18fb34365f1d1")
                .unwrap(),
            H256::from_str("968dfb4ca3311fa3bf4d696dde334e30edf3542e8ea114a4f9d18fb34365f1d1")
                .unwrap(),
        ];
        let mut leaves2 = leaves.clone();
        let mut tree = build_merkle_tree(leaves);
        leaves2[4] =
            H256::from_str("345dfb4ca3311fa3bf4d696dde334e30edf3542e8ea114a4f9d18fb34365f1d1")
                .unwrap();
        tree[8] =
            H256::from_str("345dfb4ca3311fa3bf4d696dde334e30edf3542e8ea114a4f9d18fb34365f1d1")
                .unwrap();
        update_root(&mut tree, 8);
        assert_eq!(merkle_root(&leaves2), tree[0]);
    }

    #[test]
    fn test_proof() {
        let leaves = vec![
            H256::from_str("8e827ab731f2416f6057b9c7f241b1841e345ffeabb4274e35995a45f4d42a1a")
                .unwrap(),
            H256::from_str("768dfb4ca3311fa3bf4d696dde334e30edf3542e8ea114a4f9d18fb34365f1d1")
                .unwrap(),
            H256::from_str("e68dfb4ca3311fa3bf4d696dde334e30edf3542e8ea114a4f9d18fb34365f1d1")
                .unwrap(),
            H256::from_str("f68dfb4ca3311fa3bf4d696dde334e30edf3542e8ea114a4f9d18fb34365f1d1")
                .unwrap(),
            H256::from_str("968dfb4ca3311fa3bf4d696dde334e30edf3542e8ea114a4f9d18fb34365f1d1")
                .unwrap(),
        ];
        let tree = build_merkle_tree(leaves);
        let proof = build_proof(&tree, 7).unwrap();

        assert!(proof.verify(&tree[0], tree[7].clone()));
        assert!(!proof.verify(&tree[1], tree[7].clone()));
        assert!(!proof.verify(&tree[0], tree[6].clone()));
    }

    #[test]
    fn test_combined_proof() {
        let leaves = vec![
            H256::from_str("8e827ab731f2416f6057b9c7f241b1841e345ffeabb4274e35995a45f4d42a1a")
                .unwrap(),
            H256::from_str("768dfb4ca3311fa3bf4d696dde334e30edf3542e8ea114a4f9d18fb34365f1d1")
                .unwrap(),
            H256::from_str("e68dfb4ca3311fa3bf4d696dde334e30edf3542e8ea114a4f9d18fb34365f1d1")
                .unwrap(),
            H256::from_str("f68dfb4ca3311fa3bf4d696dde334e30edf3542e8ea114a4f9d18fb34365f1d1")
                .unwrap(),
            H256::from_str("968dfb4ca3311fa3bf4d696dde334e30edf3542e8ea114a4f9d18fb34365f1d1")
                .unwrap(),
        ];
        let tree = build_merkle_tree(leaves);
        let proof = build_combined_proof(&tree, vec![4, 2, 0]).unwrap();

        assert_eq!(proof.nodes.len(), 2);
        assert!(proof.verify(
            &tree[0],
            vec![
                (4, tree[8].clone()),
                (0, tree[4].clone()),
                (2, tree[6].clone())
            ]
        ));
        assert!(!proof.verify(
            &tree[0],
            vec![
                (4, tree[7].clone()),
                (0, tree[4].clone()),
                (2, tree[6].clone())
            ]
        ));
    }

}
