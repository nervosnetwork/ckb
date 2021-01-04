use crate::generic::{GetCommitTxIds, GetProposalTxIds};
use crate::util::cell::gen_spendable;
use crate::util::mining::mine;
use crate::util::transaction::always_success_transaction;
use crate::DEFAULT_TX_PROPOSAL_WINDOW;
use crate::{Node, Spec};
use ckb_jsonrpc_types::BlockTemplate;
use ckb_types::prelude::*;

pub struct MiningBasic;

impl Spec for MiningBasic {
    // Basic life cycle of transactions:
    //     1. Submit transaction 'tx' into transactions_pool after height i
    //     2. Expect tx will be included in block[i+1] proposal zone;
    //     3. Expect tx will be included in block[i + 1 + proposal_window.closest]
    //        commit zone.

    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let cells = gen_spendable(node, 1);
        let transaction = always_success_transaction(node, &cells[0]);
        node.submit_transaction(&transaction);

        mine(&node, 1);
        let block1 = node.get_tip_block();

        assert_eq!(
            block1.get_proposal_tx_ids(),
            transaction.get_proposal_tx_ids(),
        );

        // skip (proposal_window.closest - 1) block
        mine(&node, DEFAULT_TX_PROPOSAL_WINDOW.0);
        let block3 = node.get_tip_block();

        assert_eq!(block3.get_commit_tx_ids(), transaction.get_commit_tx_ids());
    }
}

pub struct BlockTemplates;

impl Spec for BlockTemplates {
    // Block template:
    //    1. Tip block hash should be parent_hash in block template;
    //    2. Block template should be updated if tip block updated.

    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let rpc_client = node.rpc_client();

        let is_block_template_equal = |template1: &BlockTemplate, template2: &BlockTemplate| {
            let mut temp = template1.clone();
            temp.current_time = template2.current_time;
            &temp == template2
        };

        let block1 = node.new_block(None, None, None);
        let block2 = node
            .new_block_builder(None, None, None)
            .header(
                block1
                    .header()
                    .as_advanced_builder()
                    .timestamp((block1.header().timestamp() + 1).pack())
                    .build(),
            )
            .build();
        assert!(block1.header().timestamp() < block2.header().timestamp());
        assert_eq!(block1.parent_hash(), block2.parent_hash());

        node.submit_block(&block1);
        node.submit_block(&block2);
        assert_eq!(
            block1.hash(),
            rpc_client.get_tip_header().hash.pack(),
            "Block1 should be the tip block according first-received policy"
        );
        let template1 = rpc_client.get_block_template(None, None, None);
        assert_eq!(
            block1.hash(),
            template1.parent_hash.pack(),
            "Block1 should be block template's parent block since it's tip block"
        );

        let block3 = node.new_block(None, None, None);
        node.submit_block(&block3);
        let template2 = rpc_client.get_block_template(None, None, None);
        assert!(
            !is_block_template_equal(&template1, &template2),
            "Template should be updated after tip block updated"
        );
    }
}
