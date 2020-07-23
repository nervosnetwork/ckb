use crate::Node;
use ckb_types::core::{BlockEconomicState, BlockNumber, BlockReward, Capacity, MinerReward};
use ckb_types::packed::{Byte32, OutPoint};
use ckb_types::prelude::*;
use std::collections::HashMap;

// Check the proposed-rewards and committed-rewards is correct
pub fn assert_chain_rewards(node: &Node) {
    let mut fee_collector = FeeCollector::build(node);
    let finalization_delay_length = node.consensus().finalization_delay_length();
    let tip_number = node.get_tip_block_number();
    assert!(tip_number > finalization_delay_length);

    for block_number in finalization_delay_length + 1..=tip_number {
        let block_hash = node.rpc_client().get_block_hash(block_number).unwrap();
        let early_number = block_number - finalization_delay_length;
        let early_block = node.get_block_by_number(early_number);
        let target_hash = early_block.hash();
        let txs_fee: u64 = early_block
            .transactions()
            .iter()
            .map(|tx| fee_collector.peek(tx.hash()))
            .sum();
        let proposed_fee: u64 = early_block
            .union_proposal_ids_iter()
            .map(|pid| {
                let fee = fee_collector.remove(pid);
                Capacity::shannons(fee)
                    .safe_mul_ratio(node.consensus().proposer_reward_ratio())
                    .unwrap()
                    .as_u64()
            })
            .sum();
        let committed_fee: u64 = early_block
            .transactions()
            .iter()
            .skip(1)
            .map(|tx| {
                let fee = fee_collector.remove(tx.hash());
                fee - Capacity::shannons(fee)
                    .safe_mul_ratio(node.consensus().proposer_reward_ratio())
                    .unwrap()
                    .as_u64()
            })
            .sum();
        assert_proposed_reward(node, block_number, &block_hash, proposed_fee);
        assert_committed_reward(node, block_number, &block_hash, committed_fee);
        assert_block_reward(
            node,
            early_number,
            &target_hash,
            Some((block_hash, txs_fee)),
        );
    }
    {
        let target_number = 0;
        let target_hash = node.get_header_by_number(target_number).hash();
        assert_block_reward(node, target_number, &target_hash, None);
    }
    for target_number in (tip_number - finalization_delay_length + 1)..=tip_number {
        let target_hash = node.get_header_by_number(target_number).hash();
        assert_block_reward(node, target_number, &target_hash, None);
    }
}

fn assert_block_reward(
    node: &Node,
    block_number: BlockNumber,
    target_hash: &Byte32,
    ext: Option<(Byte32, u64)>,
) {
    let actual = node
        .rpc_client()
        .get_block_economic_state(target_hash.clone());
    if let Some((block_hash, txs_fee)) = ext {
        assert!(
            actual.is_some(),
            "assert_block_reward failed at block[{}]: should not be none",
            block_number
        );
        let actual: BlockEconomicState = actual.unwrap().into();
        let expected: BlockReward = node
            .rpc_client()
            .get_cellbase_output_capacity_details(block_hash)
            .unwrap()
            .into();
        let expected: MinerReward = expected.into();
        assert_eq!(
            expected, actual.miner_reward,
            "assert_block_reward failed at block[{}]: miner_reward should be same",
            block_number
        );
        assert!(
            actual.issuance.primary == actual.miner_reward.primary,
            "assert_block_reward failed at block[{}]: all primary to miner",
            block_number
        );
        assert!(
            actual.issuance.secondary > actual.miner_reward.secondary,
            "assert_block_reward failed at block[{}]: not all secondary to miner",
            block_number
        );
        assert_eq!(
            txs_fee,
            actual.txs_fee.as_u64(),
            "assert_block_reward failed at block[{}]: txs_fee should be same",
            block_number
        );
    } else {
        assert_eq!(
            actual, None,
            "assert_block_reward failed at block[{}]: should be none",
            block_number
        );
    }
}

fn assert_proposed_reward(
    node: &Node,
    block_number: BlockNumber,
    block_hash: &Byte32,
    expected: u64,
) {
    let actual = node
        .rpc_client()
        .get_cellbase_output_capacity_details(block_hash.clone())
        .unwrap()
        .proposal_reward
        .value();
    assert_eq!(
        expected, actual,
        "assert_proposed_reward failed at block[{}]",
        block_number
    );
}

fn assert_committed_reward(
    node: &Node,
    block_number: BlockNumber,
    block_hash: &Byte32,
    expected: u64,
) {
    let actual = node
        .rpc_client()
        .get_cellbase_output_capacity_details(block_hash.clone())
        .unwrap()
        .tx_fee
        .value();
    assert_eq!(
        expected, actual,
        "assert_committed_reward failed at block[{}]",
        block_number
    );
}

#[derive(Default)]
struct FeeCollector {
    inner: HashMap<String, u64>,
}

impl FeeCollector {
    fn build(node: &Node) -> Self {
        let mut this = Self::default();
        let mut cells = HashMap::new();
        for number in 0..node.get_tip_block_number() {
            let block = node.get_block_by_number(number);
            for (tx_index, tx) in block.transactions().iter().enumerate() {
                for (index, output) in tx.outputs().into_iter().enumerate() {
                    let capacity: u64 = output.capacity().unpack();
                    cells.insert(OutPoint::new(tx.hash(), index as u32), capacity);
                }

                if tx_index == 0 {
                    continue;
                }
                let outputs_capacity = tx.outputs_capacity().unwrap().as_u64();
                let inputs_capacity: u64 = tx
                    .input_pts_iter()
                    .map(|previous_out_point| *cells.get(&previous_out_point).unwrap())
                    .sum();
                let fee = inputs_capacity - outputs_capacity;
                this.inner.insert(tx.hash().to_string(), fee);
                this.inner.insert(tx.proposal_short_id().to_string(), fee);
            }
        }
        this
    }

    fn remove<S: ToString>(&mut self, key: S) -> u64 {
        self.inner.remove(&key.to_string()).unwrap_or(0u64)
    }

    fn peek<S: ToString>(&mut self, key: S) -> u64 {
        self.inner
            .get(&key.to_string())
            .map(ToOwned::to_owned)
            .unwrap_or(0u64)
    }
}
