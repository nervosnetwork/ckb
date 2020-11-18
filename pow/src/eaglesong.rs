use super::PowEngine;
use ckb_types::{packed::Header, prelude::*, utilities::compact_to_target, U256};
use eaglesong::eaglesong;
use log::Level::Debug;
use log::{debug, log_enabled};

/// TODO(doc): @quake
pub struct EaglesongPowEngine;

impl PowEngine for EaglesongPowEngine {
    fn verify(&self, header: &Header) -> bool {
        let input =
            crate::pow_message(&header.as_reader().calc_pow_hash(), header.nonce().unpack());
        let mut output = [0u8; 32];
        eaglesong(&input, &mut output);

        let (block_target, overflow) = compact_to_target(header.raw().compact_target().unpack());

        if block_target.is_zero() || overflow {
            debug!(
                "compact_target is invalid: {:#x}",
                header.raw().compact_target()
            );
            return false;
        }

        if U256::from_big_endian(&output[..]).expect("bound checked") > block_target {
            if log_enabled!(Debug) {
                use ckb_types::bytes::Bytes;
                debug!(
                    "PowEngine::verify error: expected target {:#x}, got {:#x}",
                    block_target,
                    U256::from_big_endian(&output[..]).unwrap()
                );

                debug!(
                    "PowEngine::verify error: header raw 0x{:x}",
                    &header.raw().as_bytes()
                );
                debug!(
                    "PowEngine::verify error: pow hash {:#x}",
                    &header.as_reader().calc_pow_hash()
                );
                debug!(
                    "PowEngine::verify error: nonce {:#x}",
                    header.nonce().unpack()
                );
                debug!(
                    "PowEngine::verify error: pow input: 0x{:x}",
                    Bytes::from(input.to_vec())
                );
                debug!(
                    "PowEngine::verify error: pow output: 0x{:x}",
                    Bytes::from(output.to_vec())
                );
            }
            return false;
        }

        true
    }
}
