use numext_fixed_hash::H256;
use numext_fixed_uint::{u256, U256};

const ONE: U256 = U256::one();
// ONE << 255
const T_NUMBER: U256 =
    u256!("57896044618658097711785492504343953926634992332820282019728792003956564819968");

/// f(x) = 2^256 / x
pub fn target_to_difficulty(boundary: &H256) -> U256 {
    let d: U256 = boundary.into();
    if d.le(&ONE) {
        U256::max_value()
    } else {
        (&T_NUMBER / d) << 1u8
    }
}

pub fn difficulty_to_target(difficulty: &U256) -> H256 {
    if difficulty.le(&ONE) {
        H256::max_value()
    } else {
        let boundary = (&T_NUMBER / difficulty) << 1u8;
        boundary.into()
    }
}

#[cfg(test)]
mod tests {
    use super::{target_to_difficulty, ONE, T_NUMBER};
    use numext_fixed_hash::{h256, H256};
    use numext_fixed_uint::{u256, U256};

    #[test]
    fn test_boundary_to_difficulty() {
        let h1 = h256!("0x1000");
        let h2: U256 = target_to_difficulty(&h1);

        assert_eq!(target_to_difficulty(&h2.into()), u256!("4096"));
    }

    #[test]
    fn test_t_number() {
        assert_eq!(T_NUMBER, ONE << 255);
    }
}
