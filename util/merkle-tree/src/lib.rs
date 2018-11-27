extern crate bigint;
extern crate hash;

mod hasher;
mod proof;
mod tree;

pub use hasher::{DefaultHasher, Hasher};
pub use proof::Proof;
pub use tree::Tree;

fn lower_leafs_count(n: usize) -> usize {
    (n & (n.next_power_of_two() >> 1).saturating_sub(1)) << 1
}

use bigint::H256;

pub fn merkle_root(input: &[H256]) -> H256 {
    Tree::default(&input.iter().map(|h| h.0).collect::<Vec<_>>())
        .root()
        .unwrap_or([0; 32])
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bigint::H256;
    use std::str::FromStr;

    #[test]
    fn merkle_root_test() {
        assert_eq!(
            merkle_root(&[
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
            ]),
            H256::from_str("34c5d6f2ec196e6836549e49b0ed73a1b19524acc6f1d0e5c951cfd9652da93c")
                .unwrap()
        );
    }

}
