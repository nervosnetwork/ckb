use hash::Sha3;
use numext_fixed_hash::H256;

fn lowest_children_len(amount: usize) -> usize {
    let mut n: usize = 1;
    let mut r: usize = 0;

    while n <= amount {
        r = amount - n;
        n <<= 1;
    }

    r << 1
}

pub fn merkle_root(input: &[H256]) -> H256 {
    let inlen = input.len();
    // in case of empty slice, just return zero
    if inlen == 0 {
        return H256::zero();
    }

    let lwlen = lowest_children_len(inlen);
    let mut i: usize = 0;
    let mut nodes = Vec::with_capacity(inlen);

    while i < lwlen {
        nodes.push(merge(&input[i], &input[i + 1]));
        i += 2;
    }

    for h in input.iter().skip(i) {
        nodes.push(h.clone());
    }

    let nlen = nodes.len();
    let mut d = 1;
    while d < nlen {
        let mut j = 0;
        while j < nlen {
            nodes[j] = merge(&nodes[j], &nodes[j + d]);
            j += d + d;
        }
        d <<= 1;
    }

    nodes[0].clone()
}

fn merge(left: &H256, right: &H256) -> H256 {
    let mut hash = [0u8; 32];
    let mut sha3 = Sha3::new_sha3_256();
    sha3.update(left.as_bytes());
    sha3.update(right.as_bytes());
    sha3.finalize(&mut hash);
    hash.into()
}

#[cfg(test)]
mod tests {
    use super::merkle_root;
    use numext_fixed_hash::H256;
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
