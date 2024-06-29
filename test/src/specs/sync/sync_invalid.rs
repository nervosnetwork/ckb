use crate::{Node, Spec};
use ckb_app_config::CKBAppConfig;
use ckb_logger::info;
use ckb_store::{ChainDB, ChainStore};
use ckb_types::core;
use ckb_types::packed;
use ckb_types::prelude::{AsBlockBuilder, Builder, Entity, IntoUncleBlockView};
use std::thread::sleep;
use std::time::Duration;

pub struct SyncInvalid;

impl Spec for SyncInvalid {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        nodes[0].mine(20);
        nodes[1].mine(1);

        nodes[0].connect(&nodes[1]);

        let info_nodes_tip = || {
            info!(
                "nodes tip_number: {:?}",
                nodes
                    .iter()
                    .map(|node| node.get_tip_block_number())
                    .collect::<Vec<_>>()
            );
        };

        let insert_invalid_block = || {
            let template = nodes[0].rpc_client().get_block_template(None, None, None);

            let block = packed::Block::from(template)
                .as_advanced_builder()
                .uncle(packed::UncleBlock::new_builder().build().into_view())
                .build();
            nodes[0]
                .rpc_client()
                .process_block_without_verify(block.data().into(), false);
        };

        info_nodes_tip();
        insert_invalid_block();
        insert_invalid_block();
        info_nodes_tip();
        assert_eq!(nodes[0].get_tip_block_number(), 22);

        while nodes[1]
            .rpc_client()
            .sync_state()
            .best_known_block_number
            .value()
            <= 20
        {
            sleep(Duration::from_secs(1));
        }

        let block_21_hash = core::BlockView::from(
            nodes[0]
                .rpc_client()
                .get_block_by_number(21)
                .expect("get block 21"),
        )
        .hash();
        let block_22_hash = core::BlockView::from(
            nodes[0]
                .rpc_client()
                .get_block_by_number(22)
                .expect("get block 22"),
        )
        .hash();

        assert!(!nodes[1].rpc_client().get_banned_addresses().is_empty());
        assert!(nodes[1]
            .rpc_client()
            .get_banned_addresses()
            .first()
            .unwrap()
            .ban_reason
            .contains(&format!("{}", block_21_hash)));
        info_nodes_tip();

        nodes[0].stop();
        nodes[1].stop();

        nodes[0].access_db(|store: &ChainDB| {
            {
                assert!(store.get_block(&block_21_hash).is_some());
                assert!(store.get_block(&block_22_hash).is_some());
                let ext = store.get_block_ext(&block_21_hash).expect("block 21 ext");
                assert_eq!(ext.verified, Some(true));
            }
            {
                assert!(store.get_block(&block_22_hash).is_some());
                assert!(store.get_block(&block_22_hash).is_some());
                let ext = store.get_block_ext(&block_22_hash).expect("block 22 ext");
                assert_eq!(ext.verified, Some(true));
            }
        });

        nodes[1].access_db(|store: &ChainDB| {
            assert!(store.get_block(&block_21_hash).is_none());
            assert!(store.get_block_ext(&block_21_hash).is_none());
            assert!(store.get_block(&block_22_hash).is_none());
            assert!(store.get_block_ext(&block_22_hash).is_none());
        });
    }

    fn modify_app_config(&self, config: &mut CKBAppConfig) {
        config.logger.filter = Some("ckb=debug".to_string());
    }
}
