use crate::{Node, Spec};
use ckb_types::core::BlockNumber;
use log::info;

pub struct ProposalExpireRuleForCommittingAndExpiredAtOneTime;

impl Spec for ProposalExpireRuleForCommittingAndExpiredAtOneTime {
    // Case: Check the proposal expire rule works fine for the case that a transaction is both
    //       committed and expired. A transaction be committed at the end of its commit-window is
    //       committed and expired.
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let window = node.consensus().tx_proposal_window();
        node.generate_blocks(window.farthest() as usize + 2);

        let tx = node.new_transaction_spend_tip_cellbase();
        node.submit_transaction(&tx);

        let submit_number = node.get_tip_block_number();
        let proposed_number = submit_number + 1;
        let commit_window = commit_window(node, proposed_number);

        info!(
            "(1) Propose `tx` at height {}. The responding commit-window is [{}, {}]",
            proposed_number, commit_window.0, commit_window.1,
        );
        let proposed_block = node
            .new_block_builder(None, None, None)
            .set_proposals(vec![tx.proposal_short_id()])
            .build();
        node.submit_block(&proposed_block);

        info!(
            "(2) Generate blank blocks for heights [{}, {}], propose and commit nothing",
            node.get_tip_block_number() + 1,
            commit_window.1 - 1,
        );
        while node.get_tip_block_number() + 1 != commit_window.1 {
            let example = node.new_block(None, None, None);
            let blank_block = example
                .as_advanced_builder()
                .set_proposals(vec![])
                .set_transactions(vec![example.transaction(0).unwrap()])
                .build();
            node.submit_block(&blank_block);
        }
        assert_eq!(node.get_tip_block_number(), commit_window.1 - 1);

        info!(
            "(3) Commit `tx` at height {}, the end of commit-window)",
            commit_window.1,
        );
        let committed_block = node
            .new_block_builder(None, None, None)
            .set_proposals(vec![])
            .build();
        assert_eq!(committed_block.transaction(1), Some(tx));
        node.submit_block(&committed_block);

        // Based on "proposal expire rule", after committing `tx`, txpool should remove `tx` from
        // pending-pool and proposal-pool, and does not propose it again in later blocks
        node.assert_tx_pool_size(0, 0);
        let later_block = node.new_block(None, None, None);
        assert_eq!(0, later_block.union_proposal_ids_iter().count(),);
    }
}

fn commit_window(node: &Node, proposed_number: BlockNumber) -> (BlockNumber, BlockNumber) {
    let proposal_window = node.consensus().tx_proposal_window();
    (
        proposed_number + proposal_window.closest(),
        proposed_number + proposal_window.farthest(),
    )
}
