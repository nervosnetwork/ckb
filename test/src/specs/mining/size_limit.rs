use crate::generic::GetCommitTxIds;
use crate::{Node, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_types::prelude::Unpack;
use log::info;

pub struct TemplateSizeLimit;

impl Spec for TemplateSizeLimit {
    // Case: txs number could be contained in new block limit by template size
    //    1. generate 6 txs;
    //    2. passing different bytes_limit when generate new block,
    //       check how many txs will be included.

    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        node.generate_blocks_until_contains_valid_cellbase();

        // get blank block size
        let blank_block = node.new_block(None, None, None);
        node.submit_block(&blank_block);
        let blank_block_size = blank_block.data().serialized_size_without_uncle_proposals();

        // Generate 6 txs
        let mut txs_hash = Vec::new();
        let block = node.get_tip_block();
        let cellbase = &block.transactions()[0];
        let capacity = cellbase.outputs().get(0).unwrap().capacity().unpack();
        let tx = node.new_transaction_with_since_capacity(cellbase.hash(), 0, capacity);
        let tx_size = tx.data().serialized_size_in_block();
        info!(
            "blank_block_size: {}, tx_size: {}",
            blank_block_size, tx_size
        );

        let mut hash = node.rpc_client().send_transaction(tx.data().into());
        txs_hash.push(hash.clone());

        (0..5).for_each(|_| {
            let tx = node.new_transaction_with_since_capacity(hash.clone(), 0, capacity);
            hash = node.rpc_client().send_transaction(tx.data().into());
            txs_hash.push(hash.clone());
        });

        // skip proposal window
        node.generate_blocks(DEFAULT_TX_PROPOSAL_WINDOW.0 as usize);

        for bytes_limit in (1000..=2000).step_by(100) {
            let new_block = node.new_block(Some(bytes_limit), None, None);
            let tx_num = ((bytes_limit as usize) - blank_block_size) / tx_size;
            assert_eq!(
                new_block.get_commit_tx_ids().len(),
                tx_num,
                "block should contain {} txs",
                tx_num
            );
        }
    }
}
