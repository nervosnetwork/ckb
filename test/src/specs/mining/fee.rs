use crate::assertion::reward_assertion::*;
use crate::generic::{GetCommitTxIds, GetProposalTxIds};
use crate::util::check::is_transaction_committed;
use crate::utils::generate_utxo_set;
use crate::{Node, Spec};
use crate::{DEFAULT_TX_PROPOSAL_WINDOW, FINALIZATION_DELAY_LENGTH};
use ckb_types::prelude::*;

pub struct FeeOfTransaction;

impl Spec for FeeOfTransaction {
    // Case: Only submit 1 transaction, and then wait for its proposed and committed
    //
    //   1. Submit transaction `tx` into transactions_pool after height `i`
    //   2. Expect that the miner proposes `tx` within `block[i + 1]`
    //   3. Expect that the miner commits `tx` within `block[i + 1 + PROPOSAL_WINDOW_CLOSEST]`
    //   4. Expect that the miner receives the proposed reward of `tx` from
    //      `block[i + 1 + FINALIZATION_DELAY_LENGTH]`
    //   5. Expect that the miner receives the committed reward of `tx` from
    //      `block[i + 1 + PROPOSAL_WINDOW_CLOSEST + FINALIZATION_DELAY_LENGTH]`

    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let closest = DEFAULT_TX_PROPOSAL_WINDOW.0;

        let txs = generate_utxo_set(node, 1).bang_random_fee(vec![node.always_success_cell_dep()]);
        node.submit_transaction(&txs[0]);

        let number_to_propose = node.get_tip_block_number() + 1;
        let number_to_commit = number_to_propose + closest;
        node.generate_blocks(2 * FINALIZATION_DELAY_LENGTH as usize);

        assert_eq!(
            node.get_block_by_number(number_to_propose)
                .get_proposal_tx_ids(),
            txs.get_proposal_tx_ids(),
        );
        assert_eq!(
            node.get_block_by_number(number_to_commit)
                .get_commit_tx_ids(),
            txs.get_commit_tx_ids()
        );

        assert_chain_rewards(node);
    }
}

pub struct FeeOfMaxBlockProposalsLimit;

impl Spec for FeeOfMaxBlockProposalsLimit {
    // Case: Submit `MAX_BLOCK_PROPOSALS_LIMIT` transactions, and then wait for its proposed and committed
    //
    //   1. Submit `MAX_BLOCK_PROPOSALS_LIMIT` transactions into transactions_pool after height `i`
    //   2. Expect that the miner receives the proposed reward of `tx` from
    //      `block[i + 1 + FINALIZATION_DELAY_LENGTH]`

    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let max_block_proposals_limit = node.consensus().max_block_proposals_limit();
        let txs = generate_utxo_set(node, max_block_proposals_limit as usize)
            .bang_random_fee(vec![node.always_success_cell_dep()]);
        txs.iter().for_each(|tx| {
            node.submit_transaction(tx);
        });

        let number_to_propose = node.get_tip_block_number() + 1;
        node.generate_blocks(2 * FINALIZATION_DELAY_LENGTH as usize);

        assert_eq!(
            node.get_block_by_number(number_to_propose)
                .get_proposal_tx_ids()
                .len(),
            txs.get_proposal_tx_ids().len()
        );
        assert!(txs.iter().all(|tx| is_transaction_committed(node, tx)));

        assert_chain_rewards(node);
    }
}

pub struct FeeOfMultipleMaxBlockProposalsLimit;

impl Spec for FeeOfMultipleMaxBlockProposalsLimit {
    // Case: Submit `3 * MAX_BLOCK_PROPOSALS_LIMIT` transactions, and then wait for its proposed and committed
    //
    //   1. Submit `3 * MAX_BLOCK_PROPOSALS_LIMIT` transactions into transactions_pool after height `i`
    //   2. Expect that the miner propose those transactions in the next `3` blocks, every block
    //      contains `MAX_BLOCK_PROPOSALS_LIMIT` transactions

    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let max_block_proposals_limit = node.consensus().max_block_proposals_limit();

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
                "block should contain {} blocks in proposal zone",
                max_block_proposals_limit,
            );
        });
        node.generate_blocks(2 * FINALIZATION_DELAY_LENGTH as usize);

        assert!(txs.iter().all(|tx| is_transaction_committed(node, tx)));
        assert_chain_rewards(node);
    }
}

pub struct ProposeButNotCommit;

impl Spec for ProposeButNotCommit {
    crate::setup!(num_nodes: 2);

    // Case: Propose a transaction but never commit it
    //     1. feed_node propose a tx in the latest block but not commit;
    //     2. target_node fork from feed_node which tip block doesn't proposal tx;
    //     3. target_node keep growing, but it will never commit 'tx'
    //        since its trasactions_pool does not have 'tx'

    fn run(&self, nodes: &mut Vec<Node>) {
        let target_node = &nodes[0];
        let feed_node = &nodes[1];

        let txs = generate_utxo_set(feed_node, 1)
            .bang_random_fee(vec![feed_node.always_success_cell_dep()]);
        feed_node.submit_transaction(&txs[0]);
        feed_node.generate_block();

        let feed_blocks: Vec<_> = (1..feed_node.get_tip_block_number())
            .map(|number| feed_node.get_block_by_number(number))
            .collect();

        feed_blocks.iter().for_each(|block| {
            target_node.submit_block(&block);
        });
        target_node.generate_blocks(2 * FINALIZATION_DELAY_LENGTH as usize);

        assert!(!is_transaction_committed(target_node, &txs[0]));
    }
}

pub struct ProposeDuplicated;

impl Spec for ProposeDuplicated {
    // Case: Uncle contains a proposal, and the new block contains the same one.

    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
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

        node.generate_blocks(2 * FINALIZATION_DELAY_LENGTH as usize);

        assert!(txs.iter().all(|tx| is_transaction_committed(node, tx)));
        assert_chain_rewards(node);
    }
}
