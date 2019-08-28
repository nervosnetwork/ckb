use numext_fixed_uint::{u256, U256};

use crate::{packed::Byte32, prelude::*};

const ONE: U256 = U256::one();
// ONE << 255
const T_NUMBER: U256 =
    u256!("57896044618658097711785492504343953926634992332820282019728792003956564819968");

/// f(x) = 2^256 / x
pub fn target_to_difficulty(boundary: &Byte32) -> U256 {
    let d = U256::from_big_endian(boundary.as_slice()).expect("convert from Byte32 to U256");
    if d.le(&ONE) {
        U256::max_value()
    } else {
        (&T_NUMBER / d) << 1u8
    }
}

pub fn difficulty_to_target(difficulty: &U256) -> Byte32 {
    if difficulty.le(&ONE) {
        Byte32::max_value()
    } else {
        let boundary = (&T_NUMBER / difficulty) << 1u8;
        let mut inner = vec![0u8; 32];
        boundary
            .into_big_endian(&mut inner)
            .expect("convert from U256 to Byte32");
        Byte32::new_unchecked(inner.into())
    }
}

#[cfg(test)]
mod tests {
    use super::{target_to_difficulty, ONE, T_NUMBER};
    use crate::{packed::Byte32, prelude::*};
    use ckb_fixed_hash::{h256, H256};
    use numext_fixed_uint::{u256, U256};

    #[test]
    fn test_boundary_to_difficulty() {
        let h1 = h256!("0x1000");
        let h2: U256 = target_to_difficulty(&h1.pack());
        let h2 = {
            let mut inner = vec![0u8; 32];
            h2.into_big_endian(&mut inner)
                .expect("convert from U256 to Byte32");
            Byte32::new_unchecked(inner.into())
        };

        assert_eq!(target_to_difficulty(&h2), u256!("4096"));
    }

    #[test]
    fn test_t_number() {
        assert_eq!(T_NUMBER, ONE << 255);
    }
}
