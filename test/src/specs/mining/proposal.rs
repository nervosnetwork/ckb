use crate::generic::GetProposalTxIds;
use crate::{Node, Spec};
use ckb_types::prelude::*;

pub struct AvoidDuplicatedProposalsWithUncles;

impl Spec for AvoidDuplicatedProposalsWithUncles {
    // Case: This is not a validation rule, but just an improvement for miner
    //       filling proposals: Don't re-propose the transactions which
    //       has already been proposed within the uncles.
    //    1. Submit `tx` into mempool, and `uncle` which proposed `tx` as an candidate uncle
    //    2. Get block template, expect empty proposals cause we already proposed `tx` within `uncle`

    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        node.generate_blocks_until_contains_valid_cellbase();

        let tx = node.new_transaction_spend_tip_cellbase();

        let uncle = {
            let block = node.new_block(None, None, None);
            let uncle = block
                .as_advanced_builder()
                .timestamp((block.timestamp() + 1).pack())
                .set_proposals(vec![tx.proposal_short_id()])
                .build();
            node.submit_block(&block);
            uncle
        };
        node.submit_block(&uncle);
        node.submit_transaction(&tx);

        let block = node.new_block(None, None, None);
        assert_eq!(
            vec![uncle.hash()],
            block
                .uncles()
                .into_iter()
                .map(|u| u.hash())
                .collect::<Vec<_>>()
        );
        assert!(
            block.get_proposal_tx_ids().is_empty(),
            "expect empty proposals, actual: {:?}",
            block.get_proposal_tx_ids()
        );
    }
}
