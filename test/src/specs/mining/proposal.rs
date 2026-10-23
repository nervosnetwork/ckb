use crate::generic::GetProposalTxIds;
use crate::util::cell::gen_spendable;
use crate::util::transaction::always_success_transaction;
use crate::{Node, Spec};

pub struct AvoidDuplicatedProposalsWithUncles;

impl Spec for AvoidDuplicatedProposalsWithUncles {
    // A candidate uncle is optional, while a pending transaction must retain a
    // path into the proposal window. If both contain the same proposal ID,
    // proposal selection wins and the conflicting uncle is omitted. Keeping
    // the uncle instead would strand a reorg-recovered transaction until the
    // uncle expires from the candidate set.

    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let cells = gen_spendable(node, 1);
        let tx = always_success_transaction(node, &cells[0]);
        let uncle = {
            let block = node.new_block(None, None, None);
            let uncle = block
                .as_advanced_builder()
                .timestamp(block.timestamp() + 1)
                .set_proposals(vec![tx.proposal_short_id()])
                .build();
            node.submit_block(&block);
            uncle
        };
        node.submit_block(&uncle);
        node.submit_transaction(&tx);

        let block = node.new_block_with_blocking(|template| template.proposals.is_empty());
        assert!(
            block.uncles().into_iter().next().is_none(),
            "a conflicting optional uncle must not suppress a pending proposal"
        );
        assert_eq!(vec![tx.proposal_short_id()], block.get_proposal_tx_ids());
    }
}
