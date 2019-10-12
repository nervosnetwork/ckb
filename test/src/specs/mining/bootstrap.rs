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

    fn run(&self, net: &mut Net) {
        let node = &net.nodes[0];

        let blk_hashes: Vec<_> = (0..=DEFAULT_TX_PROPOSAL_WINDOW.1)
            .map(|_| node.generate_block())
            .collect();

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

        assert!(blk_hashes.iter().all(is_bootstrap_cellbase));

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
                    == miner
        )
    }

    fn modify_ckb_config(&self) -> Box<dyn Fn(&mut CKBAppConfig) -> ()> {
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
