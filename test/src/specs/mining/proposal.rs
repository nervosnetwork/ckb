use crate::{Net, Spec};
use ckb_types::prelude::*;
use log::info;

pub struct AvoidDuplicatedProposalsWithUncles;

impl Spec for AvoidDuplicatedProposalsWithUncles {
    crate::name!("avoid_duplicated_proposals_with_uncles");

    // Case: This is not a validation rule, but just an improvement for miner
    //       filling proposals: Don't re-propose the transactions which
    //       has already been proposed within the uncles.
    fn run(&self, net: &mut Net) {
        let node = &net.nodes[0];
        let window = node.consensus().tx_proposal_window();
        node.generate_blocks(window.farthest() as usize + 2);

        info!(
            "(1) Submit `tx` into mempool, and `uncle` which proposed `tx` as an candidate uncle"
        );
        let tx = {
            node.generate_block();
            node.new_transaction_spend_tip_cellbase()
        };
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

        info!(
            "(2) Get block template, expect empty proposals cause we already proposed `tx` within `uncle`"
        );
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
            block.data().proposals().is_empty(),
            "expect empty proposals, actual: {:?}",
            block.data().proposals()
        );
    }
}
