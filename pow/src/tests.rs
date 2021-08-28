use crate::pow_message;
use ckb_hash::blake2b_256;
use ckb_types::prelude::*;

#[test]
fn test_pow_message() {
    let zero_hash = blake2b_256(&[]).pack();
    let nonce = u128::max_value();
    let message = pow_message(&zero_hash, nonce);
    assert_eq!(
        message.to_vec(),
        [
            68, 244, 198, 151, 68, 213, 248, 197, 93, 100, 32, 98, 148, 157, 202, 228, 155, 196,
            231, 239, 67, 211, 136, 197, 161, 47, 66, 181, 99, 61, 22, 62, 255, 255, 255, 255, 255,
            255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255
        ]
        .to_vec()
    );
}
