use bigint::{H256, U256};

/// f(x) = 2^256 / x
pub fn boundary_to_difficulty(boundary: &H256) -> U256 {
    let d = U256::from(*boundary);
    if d <= U256::one() {
        U256::max_value()
    } else {
        ((U256::one() << 255) / d) << 1
    }
}

pub fn difficulty_to_boundary(difficulty: &U256) -> H256 {
    if *difficulty <= U256::one() {
        U256::max_value().into()
    } else {
        (((U256::one() << 255) / *difficulty) << 1).into()
    }
}

#[cfg(test)]
mod tests {
    use super::boundary_to_difficulty;
    use bigint::{H256, U256};

    #[test]
    fn test_boundary_to_difficulty() {
        let h1 = H256::from(4096);
        let h2: H256 = boundary_to_difficulty(&h1).into();
        assert_eq!(boundary_to_difficulty(&h2), U256::from(4096));
    }
}
