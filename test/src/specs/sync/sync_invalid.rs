use crate::{Node, Spec};
use ckb_app_config::CKBAppConfig;
use ckb_logger::info;
use ckb_store::{ChainDB, ChainStore};
use ckb_types::core;
use ckb_types::core::BlockNumber;
use ckb_types::packed;
use ckb_types::prelude::{Builder, Entity, IntoUncleBlockView};
use std::thread::sleep;
use std::time::Duration;

pub struct SyncInvalid;

impl Spec for SyncInvalid {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        nodes[0].mine(20);

        // wait for node[0] to find unverified blocks finished
        nodes[0].wait_find_unverified_blocks_finished();

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

        let insert_invalid_block = |number: BlockNumber| {
            let block = nodes[0]
                .new_block_builder_with_blocking(|template| template.number < number.into())
                .uncle(packed::UncleBlock::new_builder().build().into_view())
                .build();
            nodes[0]
                .rpc_client()
                .process_block_without_verify(block.data().into(), false);
            info!("inserted invalid block {}", number);
        };

        info_nodes_tip();
        insert_invalid_block(21);
        insert_invalid_block(22);
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

        {
            let now = std::time::Instant::now();
            while nodes[1].rpc_client().get_banned_addresses().is_empty()
                || !nodes[1]
                    .rpc_client()
                    .get_banned_addresses()
                    .first()
                    .unwrap()
                    .ban_reason
                    .contains(&format!("{}", block_21_hash))
            {
                if now.elapsed() > Duration::from_secs(60) {
                    panic!("node[1] should ban node[0] in 60s");
                }
                info!("waiting for node[1] to ban node[0]");
                sleep(Duration::from_secs(1));
            }
        }

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
