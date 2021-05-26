use crate::Node;
use ckb_types::core::{BlockEconomicState, BlockView, Capacity, Ratio, TransactionView};
use ckb_types::packed::{Byte32, OutPoint, ProposalShortId};
use ckb_types::prelude::*;
use std::collections::HashMap;

pub fn check_fee(node: &Node) {
    let tip_number = node.get_tip_block_number();
    let finalization_delay_length = node.consensus().finalization_delay_length();
    let proposer_reward_ratio = node.consensus().proposer_reward_ratio();
    let mut checker = RewardChecker::new(proposer_reward_ratio);

    for block_number in 0..=tip_number {
        let block = node.get_block_by_number(block_number);
        checker.apply_new_block(&block);

        if block_number > finalization_delay_length {
            let early_number = block_number - finalization_delay_length;
            let early_block = node.get_block_by_number(early_number);
            let (txs_fee, committed_fee, proposed_fee) = checker.calc_block_fee(&early_block);
            checker.remove_committed_proposals(&early_block);
            let economic : BlockEconomicState = node.rpc_client().get_block_economic_state(early_block.hash()).expect("block that is higher than finalization_delay_length should have economic state").into();
            assert_eq!(txs_fee, economic.txs_fee.as_u64());
            assert_eq!(proposed_fee, economic.miner_reward.proposal.as_u64());
            assert_eq!(committed_fee, economic.miner_reward.committed.as_u64());
        }
    }

    for block_number in tip_number.saturating_sub(finalization_delay_length - 1)..tip_number {
        let block_hash = node.rpc_client().get_block_hash(block_number).unwrap();
        let economic = node.rpc_client().get_block_economic_state(block_hash);
        assert_eq!(None, economic, "block not finalized has not economic state");
    }
}

/// RewardChecker calculates the block reward and compare it with the RPC
/// `get_block_economic_state` response.
///
/// A block reward consists:
///     - for proposing transactions
///     - for committing transactions
///     - epoch primary reward
///     - epoch secondary reward
///
/// As for the proposing transactions reward of a block, it has to wait until the block been finalized.
#[derive(Debug)]
pub(crate) struct RewardChecker {
    proposer_reward_ratio: Ratio,

    // #{ cell.out_point() => cell.capacity() }
    cells_capacity: HashMap<OutPoint, u64>,
    // #{ transaction.hash() => transaction.fee() }
    transactions_fee: HashMap<Byte32, u64>,
    // #{ transaction.proposal_id() => transaction.fee() }
    proposals_fee: HashMap<ProposalShortId, u64>,
}

impl RewardChecker {
    fn new(proposer_reward_ratio: Ratio) -> Self {
        Self {
            proposer_reward_ratio,
            cells_capacity: Default::default(),
            transactions_fee: Default::default(),
            proposals_fee: Default::default(),
        }
    }

    fn calc_block_fee(&self, block: &BlockView) -> (u64, u64, u64) {
        let txs = block.transactions();
        let txs_fee: u64 = txs
            .iter()
            .skip(1)
            .map(|tx| self.transactions_fee.get(&tx.hash()).unwrap())
            .sum();
        let committed_fee: u64 = txs
            .iter()
            .skip(1)
            .map(|tx| {
                let tx_fee = self.transactions_fee.get(&tx.hash()).unwrap();
                *tx_fee
                    - Capacity::shannons(*tx_fee)
                        .safe_mul_ratio(self.proposer_reward_ratio)
                        .unwrap()
                        .as_u64()
            })
            .sum();
        let proposed_fee: u64 = block
            .union_proposal_ids_iter()
            .map(|pid| {
                self.proposals_fee
                    .get(&pid)
                    .map(|tx_fee| {
                        Capacity::shannons(*tx_fee)
                            .safe_mul_ratio(self.proposer_reward_ratio)
                            .unwrap()
                            .as_u64()
                    })
                    .unwrap_or(0)
            })
            .sum();
        (txs_fee, committed_fee, proposed_fee)
    }

    // Apply all the block transactions in order
    fn apply_new_block(&mut self, block: &BlockView) {
        block
            .transactions()
            .iter()
            .for_each(|tx| self.apply_new_transaction(tx));
    }

    // Apply the transaction
    fn apply_new_transaction(&mut self, tx: &TransactionView) {
        for (index, output) in tx.outputs().into_iter().enumerate() {
            let out_point = OutPoint::new(tx.hash(), index as u32);
            let capacity: u64 = output.capacity().unpack();
            self.cells_capacity.insert(out_point, capacity);
        }

        if !tx.is_cellbase() {
            let outputs_capacity = tx.outputs_capacity().unwrap().as_u64();
            let inputs_capacity: u64 = tx
                .input_pts_iter()
                .map(|previous_out_point| self.cells_capacity.get(&previous_out_point).unwrap())
                .sum();
            let fee = inputs_capacity - outputs_capacity;
            self.transactions_fee.insert(tx.hash(), fee);
            self.proposals_fee.insert(tx.proposal_short_id(), fee);
        }
    }

    fn remove_committed_proposals(&mut self, block: &BlockView) {
        block.union_proposal_ids_iter().for_each(|pid| {
            self.proposals_fee.remove(&pid);
        });
    }
}
