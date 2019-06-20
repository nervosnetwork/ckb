use crate::error::{Error, UnclesError};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::Block;
use ckb_core::extras::EpochExt;
use fnv::FnvHashSet;
use numext_fixed_hash::H256;
use std::collections::HashSet;

pub trait UncleProvider {
    fn double_inclusion(&self, hash: &H256) -> bool;

    fn consensus(&self) -> &Consensus;

    fn epoch(&self) -> &EpochExt;
}

#[derive(Clone)]
pub struct UnclesVerifier<'a, P> {
    provider: P,
    block: &'a Block,
}

/// A block B1 is considered to be the uncle of
/// another block B2 if all of the following conditions are met:
/// (1) they are in the same epoch, sharing the same difficulty;
/// (2) height(B2) > height(B1);
/// (3) B2 is the first block in its chain to refer to B1
impl<'a, P> UnclesVerifier<'a, P>
where
    P: UncleProvider,
{
    pub fn new(provider: P, block: &'a Block) -> Self {
        UnclesVerifier { provider, block }
    }

    // -  uncles_hash
    // -  uncles_num
    // -  depth
    // -  uncle not in main chain
    // -  uncle duplicate
    pub fn verify(&self) -> Result<(), Error> {
        // verify uncles_count
        let uncles_count = self.block.uncles().len() as u32;
        if uncles_count != self.block.header().uncles_count() {
            return Err(Error::Uncles(UnclesError::MissMatchCount {
                expected: self.block.header().uncles_count(),
                actual: uncles_count,
            }));
        }

        // verify uncles_hash
        let actual_uncles_hash = self.block.cal_uncles_hash();
        if &actual_uncles_hash != self.block.header().uncles_hash() {
            return Err(Error::Uncles(UnclesError::InvalidHash {
                expected: self.block.header().uncles_hash().to_owned(),
                actual: actual_uncles_hash,
            }));
        }

        // if self.block.uncles is empty, return
        if uncles_count == 0 {
            return Ok(());
        }

        // if block is genesis, which is expected with zero uncles, return error
        if self.block.is_genesis() {
            return Err(Error::Uncles(UnclesError::OverCount {
                max: 0,
                actual: uncles_count,
            }));
        }

        // verify uncles length =< max_uncles_num
        let max_uncles_num = self.provider.consensus().max_uncles_num() as u32;
        if uncles_count > max_uncles_num {
            return Err(Error::Uncles(UnclesError::OverCount {
                max: max_uncles_num,
                actual: uncles_count,
            }));
        }

        let mut included = FnvHashSet::default();
        for uncle in self.block.uncles() {
            if uncle.header().difficulty() != self.provider.epoch().difficulty() {
                return Err(Error::Uncles(UnclesError::InvalidDifficulty));
            }

            if self.provider.epoch().number() != uncle.header().epoch() {
                return Err(Error::Uncles(UnclesError::InvalidDifficultyEpoch));
            }

            if uncle.header().number() >= self.block.header().number() {
                return Err(Error::Uncles(UnclesError::InvalidNumber));
            }

            let uncle_hash = uncle.header.hash().to_owned();
            if included.contains(&uncle_hash) {
                return Err(Error::Uncles(UnclesError::Duplicate(uncle_hash.clone())));
            }

            if self.provider.double_inclusion(&uncle_hash) {
                return Err(Error::Uncles(UnclesError::DoubleInclusion(
                    uncle_hash.clone(),
                )));
            }

            if uncle.proposals().len()
                > self.provider.consensus().max_block_proposals_limit() as usize
            {
                return Err(Error::Uncles(UnclesError::ExceededMaximumProposalsLimit));
            }

            if uncle.header.proposals_hash() != &uncle.cal_proposals_hash() {
                return Err(Error::Uncles(UnclesError::ProposalsHash));
            }

            let mut seen = HashSet::with_capacity(uncle.proposals().len());
            if !uncle.proposals().iter().all(|id| seen.insert(id)) {
                return Err(Error::Uncles(UnclesError::ProposalDuplicate));
            }

            if !self
                .provider
                .consensus()
                .pow_engine()
                .verify_header(&uncle.header)
            {
                return Err(Error::Uncles(UnclesError::InvalidProof));
            }

            included.insert(uncle_hash);
        }

        Ok(())
    }
}
