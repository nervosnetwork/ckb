use numext_fixed_hash::H256;
use numext_fixed_uint::U256;

const ONE: U256 = U256::one();

/// f(x) = 2^256 / x
pub fn boundary_to_difficulty(boundary: &H256) -> U256 {
    let d: U256 = boundary.into();
    if d.le(&ONE) {
        U256::max_value()
    } else {
        ((ONE << 255) / d) << 1
    }
}

pub fn difficulty_to_boundary(difficulty: &U256) -> H256 {
    if difficulty.le(&ONE) {
        U256::max_value().into()
    } else {
        let t = ONE << 255;
        let boundary = (&t / difficulty) << 1u8;
        boundary.into()
    }
}

#[cfg(test)]
mod tests {
    use super::boundary_to_difficulty;
    use numext_fixed_hash::H256;
    use numext_fixed_uint::U256;

    #[test]
    fn test_boundary_to_difficulty() {
        let h1 = H256::from_trimmed_hex_str("1000").unwrap();
        let h2: U256 = boundary_to_difficulty(&h1);

        assert_eq!(boundary_to_difficulty(&h2.into()), U256::from(4096u64));
    }
}
