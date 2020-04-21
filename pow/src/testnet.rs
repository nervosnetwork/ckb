use super::PowEngine;
use ckb_hash::blake2b_256;
use ckb_types::{packed::Header, prelude::*, utilities::compact_to_target, U256};
use eaglesong::eaglesong;

pub struct TestnetPowEngine;

impl PowEngine for TestnetPowEngine {
    fn verify(&self, header: &Header) -> bool {
        let input =
            crate::pow_message(&header.as_reader().calc_pow_hash(), header.nonce().unpack());
        let output = {
            let mut output_tmp = [0u8; 32];
            eaglesong(&input, &mut output_tmp);
            blake2b_256(&output_tmp)
        };

        let (block_target, overflow) = compact_to_target(header.raw().compact_target().unpack());

        if block_target.is_zero() || overflow {
            return false;
        }

        if U256::from_big_endian(&output[..]).expect("bound checked") > block_target {
            return false;
        }

        true
    }
}
