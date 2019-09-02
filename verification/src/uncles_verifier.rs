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

// A block B1 is considered to be the uncle of another block B2 if all of the following conditions are met:
// (1) they are in the same epoch, sharing the same difficulty;
// (2) height(B2) > height(B1);
// (3) B1's parent is either B2's ancestor or embedded in B2 or its ancestors as an uncle;
// and (4) B2 is the first block in its chain to refer to B1.
impl<'a, P> UnclesVerifier<'a, P>
where
    P: UncleProvider,
{
    pub fn new(provider: P, block: &'a BlockView) -> Self {
        UnclesVerifier { provider, block }
    }

    // -  uncles_hash
    // -  uncles_num
    // -  depth
    // -  uncle not in main chain
    // -  uncle duplicate
    pub fn verify(&self) -> Result<(), Error> {
        // verify uncles_count
        let uncles_count = self.block.data().uncles().len() as u32;
        if uncles_count != self.block.uncles_count() {
            Err(UnclesError::UnmatchedCount {
                expected: self.block.uncles_count(),
                actual: uncles_count,
            })?;
        }

        // verify uncles_hash
        let actual_uncles_hash = self.block.calc_uncles_hash();
        if actual_uncles_hash != self.block.uncles_hash() {
            Err(UnclesError::UnmatchedUnclesHash {
                expected: self.block.uncles_hash(),
                actual: actual_uncles_hash,
            })?;
        }

        // if self.block.uncles is empty, return
        if uncles_count == 0 {
            return Ok(());
        }

        // if block is genesis, which is expected with zero uncles, return error
        if self.block.is_genesis() {
            Err(UnclesError::TooManyUncles {
                max: 0,
                actual: uncles_count,
            })?;
        }

        // verify uncles length =< max_uncles_num
        let max_uncles_num = self.provider.consensus().max_uncles_num() as u32;
        if uncles_count > max_uncles_num {
            Err(UnclesError::TooManyUncles {
                max: max_uncles_num,
                actual: uncles_count,
            })?;
        }

        let mut included: HashMap<Byte32, BlockNumber> = HashMap::default();
        for uncle in self.block.uncles().into_iter() {
            if &uncle.difficulty() != self.provider.epoch().difficulty() {
                Err(UnclesError::UnmatchedDifficulty)?;
            }

            if self.provider.epoch().number() != uncle.epoch() {
                Err(UnclesError::UnmatchedEpochNumber)?;
            }

            if uncle.number() >= self.block.number() {
                Err(UnclesError::UnmatchedBlockNumber)?;
            }

            let embedded_descendant = included
                .get(&uncle.data().header().raw().parent_hash())
                .map(|number| (number + 1) == uncle.number())
                .unwrap_or(false);

            if !(embedded_descendant || self.provider.descendant(&uncle.header())) {
                Err(UnclesError::DescendantLimit)?;
            }

            if included.contains_key(&uncle.hash()) {
                Err(UnclesError::DuplicatedUncles(uncle.hash()))?;
            }

            if self.provider.double_inclusion(&uncle.hash()) {
                Err(UnclesError::DoubleInclusion(uncle.hash()))?;
            }

            if uncle.data().proposals().len()
                > self.provider.consensus().max_block_proposals_limit() as usize
            {
                Err(UnclesError::TooManyProposals)?;
            }

            if uncle.proposals_hash() != uncle.data().as_reader().calc_proposals_hash() {
                Err(UnclesError::UnmatchedProposalRoot)?;
            }

            let mut seen = HashSet::with_capacity(uncle.data().proposals().len());
            if !uncle
                .data()
                .proposals()
                .into_iter()
                .all(|id| seen.insert(id))
            {
                Err(UnclesError::DuplicatedProposalTransactions)?;
            }

            if !self
                .provider
                .consensus()
                .pow_engine()
                .verify(&uncle.data().header())
            {
                Err(PowError::InvalidNonce)?;
            }

            included.insert(uncle.hash(), uncle.number());
        }

        Ok(())
    }
}
