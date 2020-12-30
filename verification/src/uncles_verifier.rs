use crate::{PowError, UnclesError};
use ckb_chain_spec::consensus::Consensus;
use ckb_error::Error;
use ckb_types::{
    core::{BlockNumber, BlockView, EpochExt, HeaderView},
    packed::Byte32,
};
use std::collections::{HashMap, HashSet};

pub trait UncleProvider {
    fn double_inclusion(&self, hash: &Byte32) -> bool;

    fn consensus(&self) -> &Consensus;

    fn epoch(&self) -> &EpochExt;

    fn descendant(&self, uncle: &HeaderView) -> bool;
}

#[derive(Clone)]
pub struct UnclesVerifier<'a, P> {
    provider: P,
    block: &'a BlockView,
}

// A block B1 is considered to be the uncle of another block B2 if all of the following conditions
// are met:
//
// 1. they are in the same epoch, sharing the same difficulty;
// 2. height(B2) > height(B1);
// 3. B1's parent is either B2's ancestor or embedded in B2 or its ancestors as an uncle;
// 4. B2 is the first block in its chain to refer to B1.
impl<'a, P> UnclesVerifier<'a, P>
where
    P: UncleProvider,
{
    pub fn new(provider: P, block: &'a BlockView) -> Self {
        UnclesVerifier { provider, block }
    }

    //
    // - uncles_hash
    // - uncles_num
    // - depth
    // - uncle not in main chain
    // - uncle duplicate
    pub fn verify(&self) -> Result<(), Error> {
        let uncles_count = self.block.data().uncles().len() as u32;

        // verify uncles_hash
        let actual_uncles_hash = self.block.calc_uncles_hash();
        if actual_uncles_hash != self.block.uncles_hash() {
            return Err(UnclesError::InvalidHash {
                expected: self.block.uncles_hash(),
                actual: actual_uncles_hash,
            }
            .into());
        }

        // if self.block.uncles is empty, return
        if uncles_count == 0 {
            return Ok(());
        }

        // if block is genesis, which is expected with zero uncles, return error
        if self.block.is_genesis() {
            return Err(UnclesError::OverCount {
                max: 0,
                actual: uncles_count,
            }
            .into());
        }

        // verify uncles length =< max_uncles_num
        let max_uncles_num = self.provider.consensus().max_uncles_num() as u32;
        if uncles_count > max_uncles_num {
            return Err(UnclesError::OverCount {
                max: max_uncles_num,
                actual: uncles_count,
            }
            .into());
        }

        let mut included: HashMap<Byte32, BlockNumber> = HashMap::default();
        for uncle in self.block.uncles().into_iter() {
            if uncle.compact_target() != self.provider.epoch().compact_target() {
                return Err(UnclesError::InvalidTarget.into());
            }

            if self.provider.epoch().number() != uncle.epoch().number() {
                return Err((UnclesError::InvalidDifficultyEpoch).into());
            }

            if uncle.number() >= self.block.number() {
                return Err((UnclesError::InvalidNumber).into());
            }

            let embedded_descendant = included
                .get(&uncle.data().header().raw().parent_hash())
                .map(|number| (number + 1) == uncle.number())
                .unwrap_or(false);

            if !(embedded_descendant || self.provider.descendant(&uncle.header())) {
                return Err((UnclesError::DescendantLimit).into());
            }

            if included.contains_key(&uncle.hash()) {
                return Err((UnclesError::Duplicate(uncle.hash())).into());
            }

            if self.provider.double_inclusion(&uncle.hash()) {
                return Err((UnclesError::DoubleInclusion(uncle.hash())).into());
            }

            if uncle.data().proposals().len()
                > self.provider.consensus().max_block_proposals_limit() as usize
            {
                return Err((UnclesError::ExceededMaximumProposalsLimit).into());
            }

            if uncle.proposals_hash() != uncle.data().as_reader().calc_proposals_hash() {
                return Err((UnclesError::ProposalsHash).into());
            }

            let mut seen = HashSet::with_capacity(uncle.data().proposals().len());
            if !uncle
                .data()
                .proposals()
                .into_iter()
                .all(|id| seen.insert(id))
            {
                return Err((UnclesError::ProposalDuplicate).into());
            }

            if !self
                .provider
                .consensus()
                .pow_engine()
                .verify(&uncle.data().header())
            {
                return Err((PowError::InvalidNonce).into());
            }

            included.insert(uncle.hash(), uncle.number());
        }

        Ok(())
    }
}
