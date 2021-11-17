//! This mod implemented a ckb block reward calculator

use ckb_chain_spec::consensus::Consensus;
use ckb_dao::DaoCalculator;
use ckb_error::Error;
use ckb_logger::debug;
use ckb_store::ChainStore;
use ckb_types::{
    core::{BlockReward, Capacity, CapacityResult, HeaderView},
    packed::{Byte32, CellbaseWitness, ProposalShortId, Script},
    prelude::*,
};
use std::cmp;
use std::collections::HashSet;

#[cfg(test)]
mod tests;

/// Block Reward Calculator.
/// A Block reward calculator is used to calculate the total block reward for the target block.
///
/// For block(i) miner, CKB issues its total block reward by enforcing the
/// block(i + PROPOSAL_WINDOW.farthest + 1)'s cellbase:
///   - cellbase output capacity is block(i)'s total block reward
///   - cellbase output lock is block(i)'s miner provided lock in block(i) 's cellbase output-data
/// Conventionally, We say that block(i) is block(i + PROPOSAL_WINDOW.farthest + 1)'s target block.
///
/// Target block's total reward consists of four parts:
///  - primary block reward
///  - secondary block reward
///  - proposals reward
///  - transactions fees
pub struct RewardCalculator<'a, CS> {
    consensus: &'a Consensus,
    store: &'a CS,
}

impl<'a, CS: ChainStore<'a>> RewardCalculator<'a, CS> {
    /// Creates a new `RewardCalculator`.
    pub fn new(consensus: &'a Consensus, store: &'a CS) -> Self {
        RewardCalculator { consensus, store }
    }

    /// Calculates the current block number based on `parent,` locates the current block's target block, returns the target block miner's lock, and total block reward.
    pub fn block_reward_to_finalize(
        &self,
        parent: &HeaderView,
    ) -> Result<(Script, BlockReward), Error> {
        let block_number = parent.number() + 1;
        let target_number = self
            .consensus
            .finalize_target(block_number)
            .expect("block number checked before involving finalize_target");
        let target = self
            .store
            .get_block_hash(target_number)
            .and_then(|hash| self.store.get_block_header(&hash))
            .expect("block hash checked before involving get_ancestor");
        self.block_reward_internal(&target, parent)
    }

    /// Returns the `target` block miner's lock and total block reward.
    pub fn block_reward_for_target(
        &self,
        target: &HeaderView,
    ) -> Result<(Script, BlockReward), Error> {
        let finalization_parent_number =
            target.number() + self.consensus.finalization_delay_length() - 1;
        let parent = self
            .store
            .get_block_hash(finalization_parent_number)
            .and_then(|hash| self.store.get_block_header(&hash))
            .expect("block hash checked before involving get_ancestor");
        self.block_reward_internal(target, &parent)
    }

    fn block_reward_internal(
        &self,
        target: &HeaderView,
        parent: &HeaderView,
    ) -> Result<(Script, BlockReward), Error> {
        let target_lock = CellbaseWitness::from_slice(
            &self
                .store
                .get_cellbase(&target.hash())
                .expect("target cellbase exist")
                .witnesses()
                .get(0)
                .expect("target witness exist")
                .raw_data(),
        )
        .expect("cellbase loaded from store should has non-empty witness")
        .lock();

        let txs_fees = self.txs_fees(target)?;
        let proposal_reward = self.proposal_reward(parent, target)?;
        let (primary, secondary) = self.base_block_reward(target)?;

        let total = txs_fees
            .safe_add(proposal_reward)?
            .safe_add(primary)?
            .safe_add(secondary)?;

        debug!(
            "[RewardCalculator] target {} {}\n
             txs_fees {:?}, proposal_reward {:?}, primary {:?}, secondary: {:?}, total_reward {:?}",
            target.number(),
            target.hash(),
            txs_fees,
            proposal_reward,
            primary,
            secondary,
            total,
        );

        let block_reward = BlockReward {
            total,
            primary,
            secondary,
            tx_fee: txs_fees,
            proposal_reward,
        };

        Ok((target_lock, block_reward))
    }

    // Miner get (tx_fee - 40% of tx fee) for tx commitment.
    // Be careful of the rounding, tx_fee - 40% of tx fee is different from 60% of tx fee.
    fn txs_fees(&self, target: &HeaderView) -> CapacityResult<Capacity> {
        let consensus = self.consensus;
        let target_ext = self
            .store
            .get_block_ext(&target.hash())
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

    fn proposal_reward(
        &self,
        parent: &HeaderView,
        target: &HeaderView,
    ) -> CapacityResult<Capacity> {
        let mut target_proposals = self.get_proposal_ids_by_hash(&target.hash());

        let proposal_window = self.consensus.tx_proposal_window();
        let proposer_ratio = self.consensus.proposer_reward_ratio();
        let block_number = parent.number() + 1;
        let store = self.store;

        let mut reward = Capacity::zero();

        // Transaction can be committed at height H(c): H(c) > H(w_close)
        let competing_commit_start = cmp::max(
            block_number.saturating_sub(proposal_window.length()),
            1 + proposal_window.closest(),
        );

        let mut proposed: HashSet<ProposalShortId> = HashSet::new();
        let mut index = parent.to_owned();

        // NOTE: We have to ensure that `committed_idx_proc` and `txs_fees_proc` return in the
        // same order, the order of transactions in block.
        let committed_idx_proc = |hash: &Byte32| -> Vec<ProposalShortId> {
            store
                .get_block_txs_hashes(hash)
                .into_iter()
                .skip(1)
                .map(|tx_hash| ProposalShortId::from_tx_hash(&tx_hash))
                .collect()
        };

        let txs_fees_proc = |hash: &Byte32| -> Vec<Capacity> {
            store
                .get_block_ext(hash)
                .expect("block ext stored")
                .txs_fees
        };

        let committed_idx = committed_idx_proc(&index.hash());

        let has_committed = target_proposals
            .intersection(&committed_idx.iter().cloned().collect::<HashSet<_>>())
            .next()
            .is_some();
        if has_committed {
            for (id, tx_fee) in committed_idx
                .into_iter()
                .zip(txs_fees_proc(&index.hash()).iter())
            {
                // target block is the earliest block with effective proposals for the parent block
                if target_proposals.remove(&id) {
                    reward = reward.safe_add(tx_fee.safe_mul_ratio(proposer_ratio)?)?;
                }
            }
        }

        while index.number() > competing_commit_start && !target_proposals.is_empty() {
            index = store
                .get_block_header(&index.data().raw().parent_hash())
                .expect("header stored");

            // Transaction can be proposed at height H(p): H(p) > H(0)
            let competing_proposal_start =
                cmp::max(index.number().saturating_sub(proposal_window.farthest()), 1);

            let previous_ids = store
                .get_block_hash(competing_proposal_start)
                .map(|hash| self.get_proposal_ids_by_hash(&hash))
                .expect("finalize target exist");

            proposed.extend(previous_ids);

            let committed_idx = committed_idx_proc(&index.hash());

            let has_committed = target_proposals
                .intersection(&committed_idx.iter().cloned().collect::<HashSet<_>>())
                .next()
                .is_some();
            if has_committed {
                for (id, tx_fee) in committed_idx
                    .into_iter()
                    .zip(txs_fees_proc(&index.hash()).iter())
                {
                    if target_proposals.remove(&id) && !proposed.contains(&id) {
                        reward = reward.safe_add(tx_fee.safe_mul_ratio(proposer_ratio)?)?;
                    }
                }
            }
        }
        Ok(reward)
    }

    fn base_block_reward(&self, target: &HeaderView) -> Result<(Capacity, Capacity), Error> {
        let data_loader = self.store.as_data_provider();
        let calculator = DaoCalculator::new(&self.consensus, &data_loader);
        let primary_block_reward = calculator.primary_block_reward(target)?;
        let secondary_block_reward = calculator.secondary_block_reward(target)?;

        Ok((primary_block_reward, secondary_block_reward))
    }

    fn get_proposal_ids_by_hash(&self, hash: &Byte32) -> HashSet<ProposalShortId> {
        let mut ids_set = HashSet::new();
        if let Some(ids) = self.store.get_block_proposal_txs_ids(&hash) {
            ids_set.extend(ids)
        }
        if let Some(us) = self.store.get_block_uncles(hash) {
            for u in us.data().into_iter() {
                ids_set.extend(u.proposals().into_iter());
            }
        }
        ids_set
    }
}
