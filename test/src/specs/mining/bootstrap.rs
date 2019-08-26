use crate::{Net, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_app_config::{BlockAssemblerConfig, CKBAppConfig};
use ckb_chain_spec::ChainSpec;
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

    fn run(&self, net: Net) {
        let node = &net.nodes[0];

        let blk_hashes: Vec<_> = (0..=DEFAULT_TX_PROPOSAL_WINDOW.1)
            .map(|_| node.generate_block())
            .collect();

        let bootstrap_lock = packed::Script::new_builder()
            .args(vec![Bytes::from(vec![1]), Bytes::from(vec![2])].pack())
            .code_hash(h256!("0xa1").pack())
            .hash_type(ScriptHashType::Data.pack())
            .build();

        let miner = packed::Script::new_builder()
            .args(vec![Bytes::from(vec![2]), Bytes::from(vec![1])].pack())
            .code_hash(h256!("0xa2").pack())
            .hash_type(ScriptHashType::Data.pack())
            .build();

        let is_bootstrap_cellbase = |blk_hash: &H256| {
            let blk: BlockView = node.get_block(blk_hash.clone()).unwrap().into();
            blk.transactions()[0].is_cellbase()
                && blk.transactions()[0]
                    .outputs()
                    .as_reader()
                    .get(0)
                    .unwrap()
                    .to_entity()
                    .lock()
                    == bootstrap_lock
        };

        assert!(blk_hashes.iter().all(is_bootstrap_cellbase));

        let hash = node.generate_block();

        let blk: BlockView = node.get_block(hash).unwrap().into();
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
                && Unpack::<Bytes>::unpack(
                    &blk.transactions()[0]
                        .outputs_data()
                        .as_reader()
                        .get(0)
                        .unwrap()
                        .to_entity()
                ) == Bytes::from(vec![1; 30])
        )
    }

    fn modify_chain_spec(&self) -> Box<dyn Fn(&mut ChainSpec) -> ()> {
        Box::new(|spec_config| {
            spec_config.genesis.bootstrap_lock = packed::Script::new_builder()
                .args(vec![Bytes::from(vec![1]), Bytes::from(vec![2])].pack())
                .code_hash(h256!("0xa1").pack())
                .hash_type(ScriptHashType::Data.pack())
                .build()
                .into();
        })
    }

    fn modify_ckb_config(&self) -> Box<dyn Fn(&mut CKBAppConfig) -> ()> {
        Box::new(|config| {
            config.block_assembler = Some(BlockAssemblerConfig {
                code_hash: h256!("0xa2"),
                args: vec![
                    JsonBytes::from_bytes(Bytes::from(vec![2])),
                    JsonBytes::from_bytes(Bytes::from(vec![1])),
                ],
                data: JsonBytes::from_bytes(Bytes::from(vec![1; 30])),
                hash_type: ScriptHashType::Data.into(),
            });
        })
    }
}
