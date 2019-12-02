use super::PowEngine;
use ckb_types::{
    core::HeaderContext, packed::Header, prelude::*, utilities::compact_to_target, U256,
};
use eaglesong::eaglesong;

pub struct EaglesongPowEngine;

impl PowEngine for EaglesongPowEngine {
    fn verify(&self, header_ctx: &HeaderContext) -> bool {
        let header = header_ctx.header().data();
        let input =
            crate::pow_message(&header.as_reader().calc_pow_hash(), header.nonce().unpack());
        let mut output = [0u8; 32];
        eaglesong(&input, &mut output);

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
