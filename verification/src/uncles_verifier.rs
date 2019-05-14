use crate::error::{Error, UnclesError};
use ckb_core::block::Block;
use ckb_core::extras::EpochExt;
use ckb_traits::ChainProvider;
use fnv::FnvHashSet;
use std::collections::HashSet;

#[derive(Clone)]
pub struct UnclesVerifier<'a, P> {
    provider: P,
    epoch: &'a EpochExt,
    block: &'a Block,
}

impl<'a, P> UnclesVerifier<'a, P>
where
    P: ChainProvider + Clone,
{
    pub fn new(provider: P, epoch: &'a EpochExt, block: &'a Block) -> Self {
        UnclesVerifier {
            provider,
            epoch,
            block,
        }
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

        // verify uncles age
        let max_uncles_age = self.provider.consensus().max_uncles_age() as u64;
        for uncle in self.block.uncles() {
            let depth = self.block.header().number().saturating_sub(uncle.number());

            if depth > max_uncles_age || depth < 1 {
                return Err(Error::Uncles(UnclesError::InvalidDepth {
                    min: self.block.header().number().saturating_sub(max_uncles_age),
                    max: self.block.header().number().saturating_sub(1),
                    actual: uncle.number(),
                }));
            }
        }

        // cB
        // cB.p^0       1 depth, valid uncle
        // cB.p^1   ---/  2
        // cB.p^2   -----/  3
        // cB.p^3   -------/  4
        // cB.p^4   ---------/  5
        // cB.p^5   -----------/  6
        // cB.p^6   -------------/
        // cB.p^7
        // verify uncles is not included in main chain
        // TODO: cache context
        let mut excluded = FnvHashSet::default();
        let mut included = FnvHashSet::default();
        excluded.insert(self.block.header().hash().to_owned());
        let mut block_hash = self.block.header().parent_hash().to_owned();
        excluded.insert(block_hash.clone());
        for _ in 0..max_uncles_age {
            if let Some(header) = self.provider.block_header(&block_hash) {
                let parent_hash = header.parent_hash().to_owned();
                excluded.insert(parent_hash.clone());
                if let Some(uncles) = self.provider.uncles(&block_hash) {
                    uncles.iter().for_each(|uncle| {
                        excluded.insert(uncle.header.hash().to_owned());
                    });
                };
                block_hash = parent_hash;
            } else {
                break;
            }
        }

        for uncle in self.block.uncles() {
            if uncle.header().difficulty() != self.epoch.difficulty() {
                return Err(Error::Uncles(UnclesError::InvalidDifficulty));
            }

            if self.epoch.number() != uncle.header().epoch() {
                return Err(Error::Uncles(UnclesError::InvalidDifficultyEpoch));
            }

            let uncle_header = uncle.header.clone();

            let uncle_hash = uncle_header.hash().to_owned();
            if included.contains(&uncle_hash) {
                return Err(Error::Uncles(UnclesError::Duplicate(uncle_hash.clone())));
            }

            if excluded.contains(&uncle_hash) {
                return Err(Error::Uncles(UnclesError::InvalidInclude(
                    uncle_hash.clone(),
                )));
            }

            if uncle.proposals().len()
                > self.provider.consensus().max_block_proposals_limit() as usize
            {
                return Err(Error::Uncles(UnclesError::ExceededMaximumProposalsLimit));
            }

            if uncle_header.proposals_hash() != &uncle.cal_proposals_hash() {
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
                .verify_header(&uncle_header)
            {
                return Err(Error::Uncles(UnclesError::InvalidProof));
            }

            included.insert(uncle_hash);
        }

        Ok(())
    }
}
