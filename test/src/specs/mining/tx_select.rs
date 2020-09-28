use crate::generic::GetCommitTxIds;
use crate::{Node, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_types::core::Capacity;

pub struct TemplateTxSelect;

impl Spec for TemplateTxSelect {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        // prepare blocks
        node.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 6) as usize);
        let number = node.get_tip_block_number();
        let blank_block_size = node
            .get_tip_block()
            .data()
            .serialized_size_without_uncle_proposals();

        // send 3 txs which tx fee rate is same
        let mut txs_hash = Vec::new();
        [501, 501, 501, 501, 300]
            .iter()
            .enumerate()
            .for_each(|(i, &n)| {
                let block = node.get_block_by_number(number - i as u64);
                let cellbase = &block.transactions()[0];
                let tx = node.new_transaction_with_fee_and_size(
                    &cellbase,
                    Capacity::shannons(n as u64),
                    n as usize,
                );
                let hash = node.rpc_client().send_transaction(tx.data().into());
                txs_hash.push(hash);
            });

        // skip proposal window
        node.generate_blocks(DEFAULT_TX_PROPOSAL_WINDOW.0 as usize);

        let new_block = node.new_block(Some(blank_block_size as u64 + 900), None, None);
        // should choose two txs: 501, 300
        assert_eq!(
            new_block.get_commit_tx_ids().len(),
            2,
            "New block should contain txs: 501, 300"
        );
    }
}
