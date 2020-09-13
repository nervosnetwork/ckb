use crate::generic::GetCommitTxIds;
use crate::util::cell::{as_input, as_output, gen_spendable};
use crate::util::mining::mine;
use crate::{Node, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_types::core::TransactionBuilder;

pub struct TemplateSizeLimit;

impl Spec for TemplateSizeLimit {
    // Case: txs number could be contained in new block limit by template size
    //    1. generate 6 txs;
    //    2. passing different bytes_limit when generate new block,
    //       check how many txs will be included.

    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];

        let cells = gen_spendable(node, 6);
        let txs: Vec<_> = cells
            .into_iter()
            .map(|cell| {
                TransactionBuilder::default()
                    .input(as_input(&cell))
                    .output(as_output(&cell))
                    .output_data(Default::default())
                    .cell_dep(node.always_success_cell_dep())
                    .build()
            })
            .collect();
        let tx_size = txs[0].data().serialized_size_in_block();

        // get blank block size
        let blank_block = node.new_block(None, Some(0), None);
        let blank_block_size = blank_block.data().serialized_size_without_uncle_proposals();

        // send transaction adn skip proposal window
        txs.into_iter().for_each(|tx| {
            node.submit_transaction(&tx);
        });

        // skip proposal window
        mine(node, DEFAULT_TX_PROPOSAL_WINDOW.0);

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
