mod error;

mod block_verifier;
mod genesis_verifier;
mod header_verifier;
mod transaction_verifier;

use ckb_types::{
    core::{BlockBuilder, BlockNumber, EpochNumberWithFraction, HeaderBuilder},
    prelude::*,
};

trait BuilderBaseOnBlockNumber {
    fn new_with_number(number: BlockNumber) -> Self;
}

impl BuilderBaseOnBlockNumber for HeaderBuilder {
    fn new_with_number(number: BlockNumber) -> HeaderBuilder {
        Self::default()
            .number(number.pack())
            .epoch(EpochNumberWithFraction::new(number / 1000, number % 1000, 1000).pack())
    }
}

impl BuilderBaseOnBlockNumber for BlockBuilder {
    fn new_with_number(number: BlockNumber) -> BlockBuilder {
        Self::default()
            .number(number.pack())
            .epoch(EpochNumberWithFraction::new(number / 1000, number % 1000, 1000).pack())
    }
}
