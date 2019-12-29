use crate::{Net, Node, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_jsonrpc_types::BlockTemplate;
use ckb_types::{core::BlockView, packed::ProposalShortId, prelude::*};
use log::info;
use std::convert::Into;
use std::thread::sleep;
use std::time::Duration;

pub struct MiningBasic;

impl Spec for MiningBasic {
    crate::name!("mining_basic");

    fn run(&self, net: &mut Net) {
        let node = &net.nodes[0];

        self.test_basic(node);
        self.test_block_template_cache(node);
    }
}

impl MiningBasic {
    pub const BLOCK_TEMPLATE_TIMEOUT: u64 = 3;

    pub fn test_basic(&self, node: &Node) {
        node.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);
        info!("Use generated block's cellbase as tx input");
        let transaction_hash = node.generate_transaction();
        let block1_hash = node.generate_block();
        let _ = node.generate_block(); // skip
        let block3_hash = node.generate_block();

        let block1: BlockView = node.rpc_client().get_block(block1_hash).unwrap().into();
        let block3: BlockView = node.rpc_client().get_block(block3_hash).unwrap().into();

        info!("Generated tx should be included in next block's proposal txs");
        assert!(block1
            .union_proposal_ids_iter()
            .any(|id| ProposalShortId::from_tx_hash(&transaction_hash).eq(&id)));

        info!("Generated tx should be included in next + n block's commit txs, current n = 2");
        assert!(block3
            .transactions()
            .into_iter()
            .any(|tx| transaction_hash.eq(&tx.hash())));
    }

    pub fn test_block_template_cache(&self, node: &Node) {
        let block1 = node.new_block(None, None, None);
        sleep(Duration::new(Self::BLOCK_TEMPLATE_TIMEOUT + 1, 0)); // Wait block timeout cache timeout
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
        assert_ne!(block1.header().timestamp(), block2.header().timestamp());

        // According to the first-received policy,
        // the first block is always the best block
        let rpc_client = node.rpc_client();
        assert_eq!(block1.hash(), node.submit_block(&block1));
        assert_eq!(block1.hash(), rpc_client.get_tip_header().hash.pack());

        let template1 = rpc_client.get_block_template(None, None, None);
        sleep(Duration::new(0, 200));
        let template2 = rpc_client.get_block_template(None, None, None);
        assert_eq!(block1.hash(), template1.parent_hash.pack());
        assert!(
            is_block_template_equal(&template1, &template2),
            "templates keep same since block template cache",
        );

        assert_eq!(block2.hash(), node.submit_block(&block2));
        assert_eq!(block1.hash(), rpc_client.get_tip_header().hash.pack());
        let template3 = rpc_client.get_block_template(None, None, None);
        assert_eq!(block1.hash(), template3.parent_hash.pack());
        assert!(
            template3.current_time.value() > template1.current_time.value(),
            "New tip block, new template",
        );
    }
}

fn is_block_template_equal(template1: &BlockTemplate, template2: &BlockTemplate) -> bool {
    let mut temp = template1.clone();
    temp.current_time = template2.current_time;
    &temp == template2
}
