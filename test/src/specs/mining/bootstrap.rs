use crate::{Net, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_app_config::{BlockAssemblerConfig, CKBAppConfig};
use ckb_jsonrpc_types::JsonBytes;
use ckb_types::{
    bytes::Bytes,
    core::{BlockView, ScriptHashType},
    h256, packed,
    prelude::*,
    H256,
};

pub struct BootstrapCellbase;

impl Spec for BootstrapCellbase {
    crate::name!("bootstrap_cellbase");

    // Since mining reward is delay sent in ckb, the 0 - PROPOSAL_WINDOW.furthest blocks'
    //    cellbase's outputs is empty, which called as bootstrap_cellbase

    fn run(&self, net: &mut Net) {
        let node = &net.nodes[0];

        let blk_hashes = node.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 1) as usize);

        let miner = packed::Script::new_builder()
            .args(Bytes::from(vec![2, 1]).pack())
            .code_hash(h256!("0xa2").pack())
            .hash_type(ScriptHashType::Data.into())
            .build();

        let is_bootstrap_cellbase = |blk_hash: &packed::Byte32| {
            let blk: BlockView = node
                .rpc_client()
                .get_block(blk_hash.clone())
                .unwrap()
                .into();
            blk.transactions()[0].is_cellbase()
                && blk.transactions()[0].outputs().as_reader().is_empty()
        };

        blk_hashes.iter().enumerate().for_each(|(index, blk_hash)| {
            assert!(
                is_bootstrap_cellbase(blk_hash),
                "The {} block's cellbase should be bootstrap_cellbase",
                index
            );
        });

        let hash = node.generate_block();
        let blk: BlockView = node.rpc_client().get_block(hash).unwrap().into();
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

    fn modify_ckb_config(&self) -> Box<dyn Fn(&mut CKBAppConfig)> {
        Box::new(|config| {
            config.block_assembler = Some(BlockAssemblerConfig {
                code_hash: h256!("0xa2"),
                args: JsonBytes::from_bytes(Bytes::from(vec![2, 1])),
                hash_type: ScriptHashType::Data.into(),
                message: Default::default(),
            });
        })
    }
}
