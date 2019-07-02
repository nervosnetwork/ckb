use crate::{Net, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_app_config::{BlockAssemblerConfig, CKBAppConfig};
use ckb_chain_spec::ChainSpec;
use ckb_core::block::Block;
use ckb_core::script::Script as CoreScript;
use ckb_core::Bytes;
use ckb_jsonrpc_types::JsonBytes;
use numext_fixed_hash::{h256, H256};

pub struct BootstrapCellbase;

impl Spec for BootstrapCellbase {
    fn run(&self, net: Net) {
        let node = &net.nodes[0];

        let blk_hashes: Vec<_> = (0..=DEFAULT_TX_PROPOSAL_WINDOW.1)
            .map(|_| node.generate_block())
            .collect();

        let bootstrap_lock = CoreScript {
            args: vec![Bytes::from(vec![1]), Bytes::from(vec![2])],
            code_hash: h256!("0xa1"),
        };

        let miner = CoreScript {
            args: vec![Bytes::from(vec![2]), Bytes::from(vec![1])],
            code_hash: h256!("0xa2"),
        };

        let is_bootstrap_cellbase = |blk_hash: &H256| {
            let blk: Block = node
                .rpc_client()
                .get_block(blk_hash.clone())
                .unwrap()
                .into();
            blk.transactions()[0].is_cellbase()
                && blk.transactions()[0].outputs()[0].lock == bootstrap_lock
        };

        assert!(blk_hashes.iter().all(is_bootstrap_cellbase));

        let hash = node.generate_block();

        let blk: Block = node.rpc_client().get_block(hash).unwrap().into();
        assert!(
            blk.transactions()[0].is_cellbase()
                && blk.transactions()[0].outputs()[0].lock == miner
                && blk.transactions()[0].outputs()[0].data == Bytes::from(vec![1; 30])
        )
    }

    fn modify_chain_spec(&self) -> Box<dyn Fn(&mut ChainSpec) -> ()> {
        Box::new(|spec_config| {
            spec_config.genesis.bootstrap_lock = CoreScript {
                args: vec![Bytes::from(vec![1]), Bytes::from(vec![2])],
                code_hash: h256!("0xa1"),
            }
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
            });
        })
    }
}
