//! This mod implemented a ckb block reward calculator

use ckb_chain_spec::consensus::Consensus;
use ckb_core::header::Header;
use ckb_core::reward::BlockReward;
use ckb_core::script::Script;
use ckb_core::transaction::ProposalShortId;
use ckb_core::Capacity;
use ckb_dao::DaoCalculator;
use ckb_error::Error;
use ckb_logger::debug;
use ckb_store::ChainStore;
use numext_fixed_hash::H256;
use std::cmp;
use std::collections::HashSet;

pub struct RewardCalculator<'a, CS> {
    pub consensus: &'a Consensus,
    pub store: &'a CS,
}

impl<'a, CS: ChainStore<'a>> RewardCalculator<'a, CS> {
    pub fn new(consensus: &'a Consensus, store: &'a CS) -> Self {
        RewardCalculator { consensus, store }
    }

    /// `RewardCalculator` is used to calculate block finalize target's reward according to the parent header.
    /// block reward consists of four parts: base block reward, tx fee, proposal reward, and secondary block reward.
    pub fn block_reward(&self, parent: &Header) -> Result<(Script, BlockReward), Error> {
        let consensus = self.consensus;
        let store = self.store;

        let block_number = parent.number() + 1;
        let target_number = consensus
            .finalize_target(block_number)
            .expect("block number checked before involving finalize_target");

        let target = self
            .store
            .get_block_hash(target_number)
            .and_then(|hash| self.store.get_block_header(&hash))
            .expect("block hash checked before involving get_ancestor");

        let target_lock = Script::from_witness(
            &store
                .get_cellbase(target.hash())
                .expect("target cellbase exist")
                .witnesses()[0],
        )
        .expect("cellbase loaded from store should has non-empty witness");

        let txs_fees = self.txs_fees(&target)?;
        let proposal_reward = self.proposal_reward(parent, &target)?;
        let (primary, secondary) = self.base_block_reward(&target)?;

        let total = txs_fees
            .safe_add(proposal_reward)?
            .safe_add(primary)?
            .safe_add(secondary)?;

        debug!(
            "[RewardCalculator] target {} {:x}\n
             txs_fees {:?}, proposal_reward {:?}, primary {:?}, secondary: {:?}, totol_reward {:?}",
            target_number,
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

    /// Miner get (tx_fee - 40% of tx fee) for tx commitment.
    /// Be careful of the rounding, tx_fee - 40% of tx fee is different from 60% of tx fee.
    pub fn txs_fees(&self, target: &Header) -> Result<Capacity, Error> {
        let consensus = self.consensus;
        let target_ext = self
            .store
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

    pub fn proposal_reward(&self, parent: &Header, target: &Header) -> Result<Capacity, Error> {
        let mut target_proposals = self.get_proposal_ids_by_hash(target.hash());

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

        let mut proposed: HashSet<ProposalShortId> = HashSet::default();
        let mut index = parent.to_owned();

        let committed_idx_proc = |hash: &H256| -> HashSet<ProposalShortId> {
            store
                .get_block_txs_hashes(hash)
                .iter()
                .skip(1)
                .map(ProposalShortId::from_tx_hash)
                .collect()
        };

        let txs_fees_proc = |hash: &H256| -> Vec<Capacity> {
            store
                .get_block_ext(hash)
                .expect("block ext stored")
                .txs_fees
        };

        let committed_idx = committed_idx_proc(index.hash());

        let has_committed = committed_idx
            .intersection(&target_proposals)
            .next()
            .is_some();

        if has_committed {
            for (id, tx_fee) in committed_idx
                .into_iter()
                .zip(txs_fees_proc(index.hash()).iter())
            {
                // target block is the earliest block with effective proposals for the parent block
                if target_proposals.remove(&id) {
                    reward = reward.safe_add(tx_fee.safe_mul_ratio(proposer_ratio)?)?;
                }
            }
        }

        while index.number() > competing_commit_start {
            index = store
                .get_block_header(index.parent_hash())
                .expect("header stored");

            // Transaction can be proposed at height H(p): H(p) > H(0)
            let competing_proposal_start =
                cmp::max(index.number().saturating_sub(proposal_window.farthest()), 1);

            let previous_ids = store
                .get_block_hash(competing_proposal_start)
                .map(|hash| self.get_proposal_ids_by_hash(&hash))
                .expect("finalize target exist");

            proposed.extend(previous_ids);

            let committed_idx = committed_idx_proc(index.hash());

            let has_committed = committed_idx
                .intersection(&target_proposals)
                .next()
                .is_some();

            if has_committed {
                for (id, tx_fee) in committed_idx
                    .into_iter()
                    .zip(txs_fees_proc(index.hash()).iter())
                {
                    if target_proposals.remove(&id) && !proposed.contains(&id) {
                        reward = reward.safe_add(tx_fee.safe_mul_ratio(proposer_ratio)?)?;
                    }
                }
            }
        }
        Ok(reward)
    }

    fn base_block_reward(&self, target: &Header) -> Result<(Capacity, Capacity), Error> {
        let calculator = DaoCalculator::new(&self.consensus, self.store);
        let primary_block_reward = calculator.primary_block_reward(target)?;
        let secondary_block_reward = calculator.secondary_block_reward(target)?;

        Ok((primary_block_reward, secondary_block_reward))
    }

    fn get_proposal_ids_by_hash(&self, hash: &H256) -> HashSet<ProposalShortId> {
        let mut ids_set = HashSet::default();
        if let Some(ids) = self.store.get_block_proposal_txs_ids(&hash) {
            ids_set.extend(ids)
        }
        if let Some(us) = self.store.get_block_uncles(&hash) {
            for u in us {
                ids_set.extend(u.proposals);
            }
        }
        ids_set
    }
}

#[cfg(test)]
mod tests {
    use super::RewardCalculator;
    use ckb_chain_spec::consensus::{Consensus, ProposalWindow};
    use ckb_core::block::BlockBuilder;
    use ckb_core::extras::BlockExt;
    use ckb_core::header::HeaderBuilder;
    use ckb_core::transaction::ProposalShortId;
    use ckb_core::transaction::TransactionBuilder;
    use ckb_db::RocksDB;
    use ckb_occupied_capacity::AsCapacity;
    use ckb_store::{ChainDB, ChainStore, COLUMNS};
    use std::collections::HashSet;
    use std::iter::FromIterator;

    #[test]
    fn get_proposal_ids_by_hash() {
        let db = RocksDB::open_tmp(COLUMNS);
        let store = ChainDB::new(db);

        let proposal1 = ProposalShortId::new([1; 10]);
        let proposal2 = ProposalShortId::new([2; 10]);
        let proposal3 = ProposalShortId::new([3; 10]);

        let expected = HashSet::from_iter(vec![proposal1, proposal2, proposal3]);

        let uncle1 = BlockBuilder::default()
            .proposal(proposal1)
            .proposal(proposal2)
            .build();
        let uncle2 = BlockBuilder::default()
            .proposal(proposal2)
            .proposal(proposal3)
            .build();

        let block = BlockBuilder::default()
            .proposal(proposal1)
            .uncles(vec![uncle1, uncle2])
            .build();

        let hash = block.header().hash();
        let txn = store.begin_transaction();
        txn.insert_block(&block).unwrap();
        txn.commit().unwrap();
        assert_eq!(block, store.get_block(&hash).unwrap());

        let consensus = Consensus::default();
        let reward_calculator = RewardCalculator::new(&consensus, &store);
        let ids = reward_calculator.get_proposal_ids_by_hash(block.header().hash());

        assert_eq!(ids, expected);
    }

    #[test]
    fn test_txs_fees() {
        let db = RocksDB::open_tmp(COLUMNS);
        let store = ChainDB::new(db);

        // Default PROPOSER_REWARD_RATIO is Ratio(4, 10)
        let consensus = Consensus::default();

        let block = BlockBuilder::default().build();
        let ext_tx_fees = vec![
            100u32.as_capacity(),
            20u32.as_capacity(),
            33u32.as_capacity(),
            34u32.as_capacity(),
        ];
        let ext = BlockExt {
            received_at: block.header().timestamp(),
            total_difficulty: block.header().difficulty().to_owned(),
            total_uncles_count: block.uncles().len() as u64,
            verified: Some(true),
            txs_fees: ext_tx_fees,
        };

        let txn = store.begin_transaction();
        txn.insert_block(&block).unwrap();
        txn.insert_block_ext(&block.header().hash(), &ext).unwrap();
        txn.commit().unwrap();

        let reward_calculator = RewardCalculator::new(&consensus, &store);
        let txs_fees = reward_calculator.txs_fees(block.header()).unwrap();

        let expected: u32 = [100u32, 20u32, 33u32, 34u32]
            .iter()
            .map(|x| x - x * 4 / 10)
            .sum();

        assert_eq!(txs_fees, expected.as_capacity());
    }

    // Earliest proposer get 40% of tx fee as reward when tx committed
    //  block H(19) target H(13) ProposalWindow(2, 5)
    //                 target                    current
    //                  /                        /
    //     10  11  12  13  14  15  16  17  18  19
    //      \   \   \   \______/___/___/___/
    //       \   \   \________/___/___/
    //        \   \__________/___/
    //         \____________/
    //
    // pn denotes poposal
    // block-10: p1
    // block-11: p2, uncles-proposals: p3
    // block-13 [target]: p1, p3, p4, p5, uncles-proposals: p6
    // block-14: p4, txs(p1, p2, p3)
    // block-15: txs(p4)
    // block-18: txs(p5, p6)
    // block-19 [current]
    // target's earliest proposals: p4, p5, p6
    #[test]
    fn test_proposal_reward() {
        let db = RocksDB::open_tmp(COLUMNS);
        let store = ChainDB::new(db);

        let consensus = Consensus::default().set_tx_proposal_window(ProposalWindow(2, 5));

        let tx1 = TransactionBuilder::default().version(100).build();
        let tx2 = TransactionBuilder::default().version(200).build();
        let tx3 = TransactionBuilder::default().version(300).build();
        let tx4 = TransactionBuilder::default().version(400).build();
        let tx5 = TransactionBuilder::default().version(500).build();
        let tx6 = TransactionBuilder::default().version(600).build();

        let p1 = tx1.proposal_short_id();
        let p2 = tx2.proposal_short_id();
        let p3 = tx3.proposal_short_id();
        let p4 = tx4.proposal_short_id();
        let p5 = tx5.proposal_short_id();
        let p6 = tx6.proposal_short_id();

        let block_10 = BlockBuilder::default()
            .header(HeaderBuilder::default().number(10).build())
            .proposal(p1)
            .build();

        let uncle = BlockBuilder::default().proposal(p3).build();
        let block_11 = BlockBuilder::default()
            .header(
                HeaderBuilder::default()
                    .number(11)
                    .parent_hash(block_10.header().hash().to_owned())
                    .build(),
            )
            .proposal(p2)
            .uncle(uncle)
            .build();

        let block_12 = BlockBuilder::default()
            .header(
                HeaderBuilder::default()
                    .number(12)
                    .parent_hash(block_11.header().hash().to_owned())
                    .build(),
            )
            .build();

        let uncle = BlockBuilder::default().proposal(p6).build();
        let block_13 = BlockBuilder::default()
            .header(
                HeaderBuilder::default()
                    .number(13)
                    .parent_hash(block_12.header().hash().to_owned())
                    .build(),
            )
            .proposals(vec![p1, p3, p4, p5])
            .uncle(uncle)
            .build();

        let block_14 = BlockBuilder::default()
            .header(
                HeaderBuilder::default()
                    .number(14)
                    .parent_hash(block_13.header().hash().to_owned())
                    .build(),
            )
            .proposal(p4)
            .transaction(TransactionBuilder::default().build())
            .transactions(vec![tx1, tx2, tx3])
            .build();

        let block_15 = BlockBuilder::default()
            .header(
                HeaderBuilder::default()
                    .number(15)
                    .parent_hash(block_14.header().hash().to_owned())
                    .build(),
            )
            .transaction(TransactionBuilder::default().build())
            .transaction(tx4)
            .build();
        let block_16 = BlockBuilder::default()
            .header(
                HeaderBuilder::default()
                    .number(16)
                    .parent_hash(block_15.header().hash().to_owned())
                    .build(),
            )
            .build();
        let block_17 = BlockBuilder::default()
            .header(
                HeaderBuilder::default()
                    .number(17)
                    .parent_hash(block_16.header().hash().to_owned())
                    .build(),
            )
            .build();
        let block_18 = BlockBuilder::default()
            .header(
                HeaderBuilder::default()
                    .number(18)
                    .parent_hash(block_17.header().hash().to_owned())
                    .build(),
            )
            .transaction(TransactionBuilder::default().build())
            .transactions(vec![tx5, tx6])
            .build();

        let ext_tx_fees_14 = vec![
            100u32.as_capacity(),
            20u32.as_capacity(),
            33u32.as_capacity(),
        ];

        let ext_14 = BlockExt {
            received_at: block_14.header().timestamp(),
            total_difficulty: block_14.header().difficulty().to_owned(),
            total_uncles_count: block_14.uncles().len() as u64,
            verified: Some(true),
            txs_fees: ext_tx_fees_14,
        };

        // txs(p4)
        let ext_tx_fees_15 = vec![300u32.as_capacity()];

        let ext_15 = BlockExt {
            received_at: block_15.header().timestamp(),
            total_difficulty: block_15.header().difficulty().to_owned(),
            total_uncles_count: block_15.uncles().len() as u64,
            verified: Some(true),
            txs_fees: ext_tx_fees_15,
        };

        // txs(p5, p6)
        let ext_tx_fees_18 = vec![41u32.as_capacity(), 999u32.as_capacity()];

        let ext_18 = BlockExt {
            received_at: block_18.header().timestamp(),
            total_difficulty: block_18.header().difficulty().to_owned(),
            total_uncles_count: block_18.uncles().len() as u64,
            verified: Some(true),
            txs_fees: ext_tx_fees_18,
        };

        let txn = store.begin_transaction();
        for block in vec![
            block_10,
            block_11,
            block_12.clone(),
            block_13.clone(),
            block_14.clone(),
            block_15.clone(),
            block_16,
            block_17,
            block_18.clone(),
        ] {
            txn.insert_block(&block).unwrap();
            txn.attach_block(&block).unwrap();
        }

        txn.insert_block_ext(&block_14.header().hash(), &ext_14)
            .unwrap();
        txn.insert_block_ext(&block_15.header().hash(), &ext_15)
            .unwrap();
        txn.insert_block_ext(&block_18.header().hash(), &ext_18)
            .unwrap();
        txn.commit().unwrap();

        assert_eq!(
            block_12.header().hash().to_owned(),
            store.get_block_hash(12).unwrap()
        );

        let reward_calculator = RewardCalculator::new(&consensus, &store);
        let proposal_reward = reward_calculator
            .proposal_reward(block_18.header(), block_13.header())
            .unwrap();

        // target's earliest proposals: p4, p5, p6
        let expected: u32 = [300u32, 41u32, 999u32].iter().map(|x| x * 4 / 10).sum();

        assert_eq!(proposal_reward, expected.as_capacity());
    }

}
