use crate::utils::generate_utxo_set;
use crate::{Net, Node, Spec};
use ckb_types::core::{
    BlockEconomicState, BlockNumber, BlockReward, BlockView, Capacity, MinerReward, TransactionView,
};
use ckb_types::packed::{Byte32, OutPoint};
use ckb_types::prelude::*;
use std::collections::{HashMap, HashSet};

pub struct FeeOfTransaction;

impl Spec for FeeOfTransaction {
    crate::name!("fee_of_transaction");

    // Case: Only submit 1 transaction, and then wait for its proposed and committed
    //
    //   1. Submit transaction `tx` into transactions_pool after height `i`
    //   2. Expect that the miner proposes `tx` within `block[i + 1]`
    //   3. Expect that the miner commits `tx` within `block[i + 1 + PROPOSAL_WINDOW_CLOSEST]`
    //   4. Expect that the miner receives the proposed reward of `tx` from
    //      `block[i + 1 + FINALIZATION_DELAY_LENGTH]`
    //   5. Expect that the miner receives the committed reward of `tx` from
    //      `block[i + 1 + PROPOSAL_WINDOW_CLOSEST + FINALIZATION_DELAY_LENGTH]`
    fn run(&self, net: &mut Net) {
        let node = &net.nodes[0];
        let closest = node.consensus().tx_proposal_window().closest();
        let finalization_delay_length = node.consensus().finalization_delay_length();

        let txs = generate_utxo_set(node, 1).bang_random_fee(vec![node.always_success_cell_dep()]);
        node.submit_transaction(&txs[0]);

        let number_to_submit = node.get_tip_block_number();
        let number_to_propose = number_to_submit + 1;
        let number_to_commit = number_to_propose + closest;
        node.generate_blocks(2 * finalization_delay_length as usize);
        assert_proposals(&node.get_block_by_number(number_to_propose), &txs);
        assert_committed(&node.get_block_by_number(number_to_commit), &txs);

        assert_transactions_committed(node, &txs);
        assert_chain_rewards(node);
    }
}

pub struct FeeOfMaxBlockProposalsLimit;

impl Spec for FeeOfMaxBlockProposalsLimit {
    crate::name!("fee_of_max_block_proposals_limit");

    // Case: Submit `MAX_BLOCK_PROPOSALS_LIMIT` transactions, and then wait for its proposed and committed
    //
    //   1. Submit `MAX_BLOCK_PROPOSALS_LIMIT` transactions into transactions_pool after height `i`
    //   2. Expect that the miner receives the proposed reward of `tx` from
    //      `block[i + 1 + FINALIZATION_DELAY_LENGTH]`
    fn run(&self, net: &mut Net) {
        let node = &net.nodes[0];
        let max_block_proposals_limit = node.consensus().max_block_proposals_limit();
        let finalization_delay_length = node.consensus().finalization_delay_length();
        let txs = generate_utxo_set(node, max_block_proposals_limit as usize)
            .bang_random_fee(vec![node.always_success_cell_dep()]);
        txs.iter().for_each(|tx| {
            node.submit_transaction(tx);
        });

        let number_to_submit = node.get_tip_block_number();
        let number_to_propose = number_to_submit + 1;
        node.generate_blocks(2 * finalization_delay_length as usize);
        assert_proposals(&node.get_block_by_number(number_to_propose), &txs);

        assert_transactions_committed(node, &txs);
        assert_chain_rewards(node);
    }
}

pub struct FeeOfMultipleMaxBlockProposalsLimit;

impl Spec for FeeOfMultipleMaxBlockProposalsLimit {
    crate::name!("fee_of_multiple_max_block_proposals_limit");

    // Case: Submit `3 * MAX_BLOCK_PROPOSALS_LIMIT` transactions, and then wait for its proposed and committed
    //
    //   1. Submit `3 * MAX_BLOCK_PROPOSALS_LIMIT` transactions into transactions_pool after height `i`
    //   2. Expect that the miner propose those transactions in the next `3` blocks, every block
    //      contains `MAX_BLOCK_PROPOSALS_LIMIT` transactions
    fn run(&self, net: &mut Net) {
        let node = &net.nodes[0];
        let max_block_proposals_limit = node.consensus().max_block_proposals_limit();
        let finalization_delay_length = node.consensus().finalization_delay_length();

        let multiple = 3;
        let txs = generate_utxo_set(node, (multiple * max_block_proposals_limit) as usize)
            .bang_random_fee(vec![node.always_success_cell_dep()]);
        txs.iter().for_each(|tx| {
            node.submit_transaction(tx);
        });

        (0..multiple).for_each(|_| {
            let block = node.new_block(None, None, None);
            node.submit_block(&block);
            assert_eq!(
                max_block_proposals_limit as usize,
                block.union_proposal_ids_iter().count(),
            );
        });

        node.generate_blocks(2 * finalization_delay_length as usize);
        assert_transactions_committed(node, &txs);
        assert_chain_rewards(node);
    }
}

pub struct ProposeButNotCommit;

impl Spec for ProposeButNotCommit {
    crate::name!("propose_but_not_commit");

    crate::setup!(num_nodes: 2, connect_all: false);

    // Case: Propose a transaction but never commit it
    fn run(&self, net: &mut Net) {
        let target_node = &net.nodes[0];
        let feed_node = &net.nodes[1];

        // We use `feed_node` to construct a chain proposed `txs` in the tip block.
        //
        // The returned `feed_blocks`, which represents the main fork of
        // `feed_node`, only proposes `txs` in the last block and never commit
        let feed_blocks: Vec<_> = {
            let txs = generate_utxo_set(feed_node, 1)
                .bang_random_fee(vec![feed_node.always_success_cell_dep()]);
            feed_node.submit_transaction(&txs[0]);
            feed_node.generate_block();

            (1..feed_node.get_tip_block_number())
                .map(|number| feed_node.get_block_by_number(number))
                .collect()
        };

        // `target_node` propose `tx`
        feed_blocks.iter().for_each(|block| {
            target_node.submit_block(&block);
        });

        // `target_node` keeps growing, but it will never commit `tx` since its transactions_pool
        // have not `tx`.
        let finalization_delay_length = feed_node.consensus().finalization_delay_length();
        target_node.generate_blocks(2 * finalization_delay_length as usize);

        assert_chain_rewards(target_node);
    }
}

pub struct ProposeDuplicated;

impl Spec for ProposeDuplicated {
    crate::name!("propose_duplicated");

    // Case: Uncle contains a proposal, and the new block contains the same one.
    fn run(&self, net: &mut Net) {
        let node = &net.nodes[0];
        let txs = generate_utxo_set(node, 1).bang_random_fee(vec![node.always_success_cell_dep()]);
        let tx = &txs[0];

        let uncle1 = {
            let uncle = node
                .new_block_builder(None, None, None)
                .proposal(tx.proposal_short_id())
                .build()
                .as_uncle();
            node.generate_block();
            uncle
        };
        let uncle2 = {
            let uncle = node
                .new_block_builder(None, None, None)
                .proposal(tx.proposal_short_id())
                .nonce(99999.pack())
                .build()
                .as_uncle();
            node.generate_block();
            uncle
        };

        let block = node
            .new_block_builder(None, None, None)
            .uncle(uncle1)
            .uncle(uncle2)
            .build();
        node.submit_transaction(tx);
        node.submit_block(&block);

        let finalization_delay_length = node.consensus().finalization_delay_length();
        node.generate_blocks(2 * finalization_delay_length as usize);

        assert_transactions_committed(node, &txs);
        assert_chain_rewards(node);
    }
}

// Check the given transactions were proposed and committed
fn assert_transactions_committed(node: &Node, transactions: &[TransactionView]) {
    let tip_number = node.get_tip_block_number();
    let mut hashes: HashSet<_> = transactions.iter().map(|tx| tx.hash()).collect();
    (1..tip_number).for_each(|number| {
        let block = node.get_block_by_number(number);
        block.transactions().iter().skip(1).for_each(|tx| {
            hashes.remove(&tx.hash());
        });
    });
    assert!(hashes.is_empty());
}

// Check the proposed-rewards and committed-rewards is correct
fn assert_chain_rewards(node: &Node) {
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

fn assert_proposals(block: &BlockView, expected: &[TransactionView]) {
    let mut actual_proposals: Vec<_> = block.union_proposal_ids_iter().collect();
    let mut expected_proposals: Vec<_> = expected.iter().map(|tx| tx.proposal_short_id()).collect();
    actual_proposals.sort_by(|a, b| a.as_bytes().cmp(&b.as_bytes()));
    expected_proposals.sort_by(|a, b| a.as_bytes().cmp(&b.as_bytes()));
    assert_eq!(
        expected_proposals,
        actual_proposals,
        "assert_proposals failed at block[{}]",
        block.number()
    );
}

fn assert_committed(block: &BlockView, expected: &[TransactionView]) {
    let actual_committed_hashes: Vec<_> = block
        .transactions()
        .iter()
        .skip(1)
        .map(|tx| tx.hash())
        .collect();
    let expected_committed_hashes: Vec<_> = expected.iter().map(|tx| tx.hash()).collect();
    assert_eq!(
        &expected_committed_hashes,
        &actual_committed_hashes,
        "assert_committed failed at block[{}]",
        block.number(),
    );
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
