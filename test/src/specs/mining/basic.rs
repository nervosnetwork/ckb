use crate::assertion::tx_assertion::*;
use crate::DEFAULT_TX_PROPOSAL_WINDOW;
use crate::{Net, Spec};
use ckb_jsonrpc_types::BlockTemplate;
use ckb_types::{core::BlockView, prelude::*};
use std::convert::Into;

pub struct MiningBasic;

impl Spec for MiningBasic {
    crate::name!("mining_basic");

    // Basic life cycle of transactions:
    //     1. Submit transaction 'tx' into transactions_pool after height i
    //     2. Expect tx will be included in block[i+1] proposal zone;
    //     3. Expect tx will be included in block[i + 1 + proposal_window.closest]
    //        commit zone.

    fn run(&self, net: &mut Net) {
        let node = &net.nodes[0];
        node.generate_blocks_until_contains_valid_cellbase();

        let transaction = [node.new_transaction_spend_tip_cellbase()];
        node.submit_transaction(&transaction[0]);

        let block1_hash = node.generate_block();
        let block1: BlockView = node.rpc_client().get_block(block1_hash).unwrap().into();

        assert_proposed_txs(&block1, &transaction);

        // skip (proposal_window.closest - 1) block
        (0..DEFAULT_TX_PROPOSAL_WINDOW.0 - 1).for_each(|_| {
            node.generate_block();
        });

        let block3_hash = node.generate_block();
        let block3: BlockView = node.rpc_client().get_block(block3_hash).unwrap().into();

        assert_committed_txs(&block3, &transaction);
    }
}

pub struct BlockTemplates;

impl Spec for BlockTemplates {
    crate::name!("block_template");

    // Block template:
    //    1. Tip block hash should be parent_hash in block template;
    //    2. Block template should be updated if tip block updated.

    fn run(&self, net: &mut Net) {
        let node = &net.nodes[0];
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
