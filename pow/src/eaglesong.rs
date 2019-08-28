use super::PowEngine;
use ckb_types::{packed::Header, prelude::*, utilities::difficulty_to_target};
use eaglesong::eaglesong;

pub struct EaglesongPowEngine;

impl PowEngine for EaglesongPowEngine {
    fn verify(&self, header: &Header) -> bool {
        let input =
            crate::pow_message(&header.as_reader().calc_pow_hash(), header.nonce().unpack());
        let mut output = [0u8; 32];
        eaglesong(&input, &mut output);
        output.pack() < difficulty_to_target(&header.raw().difficulty().unpack())
    }
}
