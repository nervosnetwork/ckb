//! This mod implemented a ckb block reward calculator

use ckb_core::header::{BlockNumber, Header};
use ckb_core::script::Script;
use ckb_core::transaction::ProposalShortId;
use ckb_core::Capacity;
use ckb_logger::debug;
use ckb_store::ChainStore;
use ckb_traits::ChainProvider;
use failure::{Error as FailureError, Fail};
use fnv::FnvHashSet;
use numext_fixed_hash::H256;
use std::cmp;

#[derive(Debug, PartialEq, Clone, Eq, Fail)]
pub enum Error {
    #[fail(display = "Can't resolve finalize target: {}", _0)]
    Target(BlockNumber),
    #[fail(display = "Can't parse Script from target witness: {:x}", _0)]
    Script(H256),
}

pub struct RewardCalculator<'a, P> {
    pub provider: &'a P,
}

impl<'a, P: ChainProvider> RewardCalculator<'a, P> {
    pub fn new(provider: &'a P) -> Self {
        RewardCalculator { provider }
    }

    /// `RewardCalculator` is used to calculate block finalize target's reward according to the parent header.
    /// block reward consists of three parts: base block reward, tx fee, proposal reward.
    pub fn block_reward(&self, parent: &Header) -> Result<(Script, Capacity), FailureError> {
        let consensus = self.provider.consensus();
        let store = self.provider.store();

        let block_number = parent.number() + 1;
        let target_number = consensus
            .finalize_target(block_number)
            .ok_or_else(|| Error::Target(block_number))?;

        let target = self
            .provider
            .get_ancestor(parent.hash(), target_number)
            .ok_or_else(|| Error::Target(block_number))?;

        let target_lock = Script::from_witness(
            &store
                .get_cellbase(target.hash())
                .expect("target cellbase exist")
                .witnesses()[0],
        )
        .ok_or_else(|| Error::Script(target.hash().to_owned()))?;

        let txs_fees = self.txs_fees(&target)?;
        let proposal_reward = self.proposal_reward(parent, &target)?;
        let base_block_reward = self.base_block_reward(&target)?;

        let reward = txs_fees
            .safe_add(proposal_reward)?
            .safe_add(base_block_reward)?;

        debug!(
            "[RewardCalculator] target {} {:x}\n
             txs_fees {:?}, proposal_reward {:?}, base_block_reward {:?}, totol_reward {:?}",
            target_number,
            target.hash(),
            txs_fees,
            proposal_reward,
            base_block_reward,
            reward,
        );
        Ok((target_lock, reward))
    }

    /// Miner get 60% of tx fee for tx commitment.
    pub fn txs_fees(&self, target: &Header) -> Result<Capacity, FailureError> {
        let consensus = self.provider.consensus();
        let target_ext = self
            .provider
            .store()
            .get_block_ext(target.hash())
            .expect("block body stored");

        target_ext
            .txs_fees
            .iter()
            .try_fold(Capacity::zero(), |acc, tx_fee| {
                tx_fee
                    .safe_mul_ratio(consensus.proposer_reward_ratio())
                    .and_then(|proposer| {
                        tx_fee
                            .safe_sub(proposer)
                            .and_then(|miner| acc.safe_add(miner))
                    })
            })
            .map_err(Into::into)
    }

    /// Earliest proposer get 40% of tx fee as reward when tx committed
    ///  block H(19) target H(13) ProposalWindow(2, 5)
    ///                 target                    current
    ///                  /                        /
    ///     10  11  12  13  14  15  16  17  18  19
    ///      \   \   \   \______/___/___/___/
    ///       \   \   \________/___/___/
    ///        \   \__________/___/
    ///         \____________/
    ///

    pub fn proposal_reward(
        &self,
        parent: &Header,
        target: &Header,
    ) -> Result<Capacity, FailureError> {
        let mut target_proposals = self.get_proposal_ids_by_hash(target.hash());

        let proposal_window = self.provider.consensus().tx_proposal_window();
        let proposer_ratio = self.provider.consensus().proposer_reward_ratio();
        let block_number = parent.number() + 1;
        let store = self.provider.store();

        let mut reward = Capacity::zero();

        // Transaction can be committed at height H(c): H(c) > H(w_close)
        let competing_commit_start = cmp::max(
            block_number.saturating_sub(proposal_window.length()),
            1 + proposal_window.closest(),
        );

        let mut proposed = FnvHashSet::default();
        let mut index = parent.to_owned();
        for (id, tx_fee) in store
            .get_block_txs_hashes(index.hash())
            .expect("block body stored")
            .iter()
            .skip(1)
            .map(ProposalShortId::from_tx_hash)
            .zip(
                store
                    .get_block_ext(index.hash())
                    .expect("block body stored")
                    .txs_fees
                    .iter(),
            )
        {
            // target block is the earliest block with effective proposals for the parent block
            if target_proposals.remove(&id) {
                reward = reward.safe_add(tx_fee.safe_mul_ratio(proposer_ratio)?)?;
            }
        }

        while index.number() > competing_commit_start {
            index = store
                .get_block_header(index.parent_hash())
                .expect("header stored");

            // Transaction can be proposed at height H(p): H(p) > H(0)
            let competing_proposal_start =
                cmp::max(index.number().saturating_sub(proposal_window.farthest()), 1);

            let previous_ids = self
                .provider
                .get_ancestor(parent.hash(), competing_proposal_start)
                .map(|header| self.get_proposal_ids_by_hash(header.hash()))
                .expect("finalize target exist");

            proposed.extend(previous_ids);

            for (id, tx_fee) in store
                .get_block_txs_hashes(index.hash())
                .expect("block body stored")
                .iter()
                .skip(1)
                .map(ProposalShortId::from_tx_hash)
                .zip(
                    store
                        .get_block_ext(index.hash())
                        .expect("block body stored")
                        .txs_fees
                        .iter(),
                )
            {
                if target_proposals.remove(&id) && !proposed.contains(&id) {
                    reward = reward.safe_add(tx_fee.safe_mul_ratio(proposer_ratio)?)?;
                }
            }
        }
        Ok(reward)
    }

    fn base_block_reward(&self, target: &Header) -> Result<Capacity, FailureError> {
        let target_parent_hash = target.parent_hash();
        let target_parent_epoch = self
            .provider
            .get_block_epoch(target_parent_hash)
            .expect("target parent exist");
        let target_parent = self
            .provider
            .store()
            .get_block_header(target_parent_hash)
            .expect("target parent exist");
        let epoch = self
            .provider
            .next_epoch_ext(&target_parent_epoch, &target_parent)
            .unwrap_or(target_parent_epoch);

        epoch.block_reward(target.number()).map_err(Into::into)
    }

    fn get_proposal_ids_by_hash(&self, hash: &H256) -> FnvHashSet<ProposalShortId> {
        let mut ids_set = FnvHashSet::default();
        if let Some(ids) = self.provider.store().get_block_proposal_txs_ids(&hash) {
            ids_set.extend(ids)
        }
        if let Some(us) = self.provider.store().get_block_uncles(&hash) {
            for u in us {
                ids_set.extend(u.proposals);
            }
        }
        ids_set
    }
}
