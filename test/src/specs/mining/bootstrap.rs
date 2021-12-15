use crate::util::mining::mine;
use crate::{Node, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_app_config::BlockAssemblerConfig;
use ckb_jsonrpc_types::JsonBytes;
use ckb_types::{
    bytes::Bytes,
    core::{BlockView, ScriptHashType},
    h256, packed,
    prelude::*,
};

pub struct BootstrapCellbase;

impl Spec for BootstrapCellbase {
    // Since mining reward is delay sent in ckb, the 0 - PROPOSAL_WINDOW.furthest blocks'
    //    cellbase's outputs is empty, which called as bootstrap_cellbase

    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];

        mine(node, DEFAULT_TX_PROPOSAL_WINDOW.1 + 1);

        let miner = packed::Script::new_builder()
            .args(Bytes::from(vec![2, 1]).pack())
            .code_hash(h256!("0xa2").pack())
            .hash_type(ScriptHashType::Data.into())
            .build();

        let is_bootstrap_cellbase = |number| {
            let blk: BlockView = node.get_block_by_number(number);
            blk.transactions()[0].is_cellbase()
                && blk.transactions()[0].outputs().as_reader().is_empty()
        };

        (1..=node.get_tip_block_number()).for_each(|number| {
            assert!(
                is_bootstrap_cellbase(number),
                "The {} block's cellbase should be bootstrap_cellbase",
                number
            );
        });

        mine(node, 1);
        let blk = node.get_tip_block();
        assert!(
            blk.transactions()[0].is_cellbase()
                && blk.transactions()[0]
                    .outputs()
                    .as_reader()
                    .get(0)
                    .unwrap()
                    .to_entity()
                    .lock()
                    == miner,
            "PROPOSAL_WINDOW.furthest + 1 block's cellbase should not be bootstrap_cellbase"
        )
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.block_assembler = Some(BlockAssemblerConfig {
            code_hash: h256!("0xa2"),
            args: JsonBytes::from_bytes(Bytes::from(vec![2, 1])),
            hash_type: ScriptHashType::Data.into(),
            message: Default::default(),
            use_binary_version_as_message_prefix: false,
            binary_version: "TEST".to_string(),
        });
    }
}
