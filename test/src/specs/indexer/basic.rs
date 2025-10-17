use crate::node::{connect_all, disconnect_all, waiting_for_sync};
use crate::util::mining::out_ibd_mode;
use crate::utils::find_available_port;
use crate::{Node, Spec};
use ckb_logger::{info, warn};
use ckb_types::packed;
use postgresql_embedded::{Settings, blocking::PostgreSQL};
use std::cell::RefCell;
use std::thread::sleep;
use std::time::Duration;

/// Test case to reproduce the rich-indexer uncle block reconstruction bug
///
/// This test specifically triggers the scenario where:
/// 1. Multiple competing blocks at the same height create uncle blocks
/// 2. Rich-indexer attempts to reconstruct uncle blocks as full blocks
/// 3. The reconstruction creates phantom blocks with corrupted data
/// 4. This causes an infinite rollback/append loop in the synchronizer
#[derive(Default)]
pub struct RichIndexerUncleBlockBug {
    postgresql: RefCell<Option<PostgreSQL>>,
}

impl Spec for RichIndexerUncleBlockBug {
    fn before_run(&self) -> Vec<Node> {
        info!("RichIndexerUncleBlockBug: Initializing test environment");

        // Create 3 nodes to maximize uncle block creation
        let node0 = Node::new(self.name(), "node0");
        let node1 = Node::new(self.name(), "node1");
        let node2 = Node::new(self.name(), "node1");
        let mut nodes = [node0, node1, node2];

        // Setup embedded PostgreSQL with detailed logging
        info!("Setting up PostgreSQL with detailed logging for rich-indexer");
        let postgres_port = 8889;
        let mut settings = Settings::default();
        settings.port = postgres_port;
        settings.host = "127.0.0.1".to_string();
        settings.temporary = true;
        settings.username = "postgres".to_string();
        settings.password = "postgres".to_string();
        settings.timeout = Some(Duration::from_secs(60));

        // Enable detailed PostgreSQL logging to capture the bug
        let configs = [
            ("log_directory", "/tmp/postgres_uncle_bug"),
            ("log_filename", "bug.log"),
            ("logging_collector", "on"),
            ("log_statement", "all"),
            ("log_min_duration_statement", "0"),
            ("shared_preload_libraries", "auto_explain"),
        ];

        for (key, value) in configs {
            settings.configuration.insert(key.into(), value.into());
        }

        let mut postgresql = PostgreSQL::new(settings.clone());
        postgresql.setup().expect("Failed to setup PostgreSQL");
        postgresql.start().expect("Failed to start PostgreSQL");
        postgresql
            .create_database("ckb_rich_indexer_uncle_test")
            .unwrap();

        info!(
            "PostgreSQL started on port {} with detailed logging",
            postgres_port
        );
        *self.postgresql.borrow_mut() = Some(postgresql);

        // Configure node0 with rich-indexer and aggressive settings to trigger the bug
        let node0 = &mut nodes[0];
        node0.modify_app_config(|config| {
            config
                .rpc
                .modules
                .push(ckb_app_config::RpcModule::RichIndexer);
            config.indexer.rich_indexer = ckb_app_config::RichIndexerConfig {
                db_type: ckb_app_config::DBDriver::Postgres,
                db_host: "127.0.0.1".to_string(),
                db_port: postgres_port,
                db_user: settings.username.clone(),
                db_password: settings.password.clone(),
                db_name: "ckb_rich_indexer_uncle_test".to_string(),
                ..Default::default()
            };

            // Aggressive settings to increase uncle block probability
            config.indexer.poll_interval = 1; // Poll every 100ms
            config.indexer.index_tx_pool = false;
            config.logger.log_to_stdout = true;
            config.logger.filter =
                Some("info,ckb_indexer_sync=trace,ckb_rich_indexer=trace".to_string());

            // // Mining settings to create more competing blocks
            // config.miner.workers = 1;
        });

        // Configure other nodes for competitive mining
        for (i, node) in nodes.iter_mut().enumerate().skip(1) {
            node.modify_app_config(|config| {
                config.logger.log_to_stdout = false;
                // config.miner.workers = 1;
            });
        }

        // Start all nodes
        nodes.iter_mut().for_each(|node| {
            info!("started node");
            node.start();
        });

        nodes.to_vec()
    }

    /// Reproduces the specific uncle block reconstruction bug
    ///
    /// Attack Plan:
    /// 1. Create initial sync between all nodes
    /// 2. Disconnect nodes to create isolated mining environments
    /// 3. Generate competing blocks at the same height with different transaction sets
    /// 4. Reconnect nodes to trigger uncle block processing
    /// 5. Monitor rich-indexer for the infinite loop bug
    fn run(&self, nodes: &mut Vec<Node>) {
        info!("=== Phase 1: Initial Setup and Sync ===");
        let node0 = &nodes[0];
        let node1 = &nodes[1];
        let node2 = &nodes[2];

        // Connect all nodes and establish initial chain
        info!("connect all nodes");
        connect_all(nodes);
        info!("mine until out bootstrap period");
        node0.mine_until_out_bootstrap_period();
        info!("out ibd mode");
        out_ibd_mode(nodes);
        info!("writing for sync");
        waiting_for_sync(nodes);

        let initial_height = node0.get_tip_block_number();
        info!("All nodes synced to height {}", initial_height);

        let print_indexer_tip = |node: &Node| -> String {
            let indexer_tip = node.rpc_client().get_indexer_tip().unwrap();
            let indexer_tip_number: u64 = indexer_tip.block_number.into();
            format!("{}-{}", indexer_tip_number, indexer_tip.block_hash)
        };

        //////////////////////////////////////////////////////////////////////////////////

        info!("\n\n\n\n------------ begin");
        let block_13 = node1.new_block_builder(None, None, None).build();
        let uncle_13 = block_13
            .as_advanced_builder()
            .timestamp(block_13.timestamp() + 1)
            .build();

        info!(
            "=========== constructed :\nblock:{}-{}\nuncle:{}-{}\n",
            block_13.number(),
            block_13.hash(),
            uncle_13.number(),
            uncle_13.hash(),
        );
        node0.process_block_without_verify(&uncle_13, false);
        node0.process_block_without_verify(&block_13, false);

        node1.process_block_without_verify(&block_13, false);
        node2.process_block_without_verify(&block_13, false);
        node1.process_block_without_verify(&uncle_13, false);
        node2.process_block_without_verify(&uncle_13, false);

        info!("\n\n");
        {
            let block_14 = node1.new_block_builder(None, None, None).build();
            let uncle_14 = block_14
                .as_advanced_builder()
                .timestamp(block_14.timestamp() + 1)
                .build();

            node1.process_block_without_verify(&uncle_14, false);
            node0.connect(&node1);
            waiting_for_sync(&[node0, node1]);

            node2.process_block_without_verify(&block_14, false);

            info!(
                "node0 tip: {}-{}, parent:{}",
                node0.get_tip_block_number(),
                node0.get_tip_block().hash(),
                node0.get_tip_block().parent_hash()
            );
            info!(
                "node1 tip: {}-{}, parent:{}",
                node1.get_tip_block_number(),
                node1.get_tip_block().hash(),
                node1.get_tip_block().parent_hash()
            );
            info!(
                "node2 tip: {}-{}, parent:{}",
                node2.get_tip_block_number(),
                node2.get_tip_block().hash(),
                node2.get_tip_block().parent_hash()
            );

            connect_all(nodes);
            node2.mine(1);
            waiting_for_sync(nodes);
            node2.mine(2);
            waiting_for_sync(nodes);
        }

        let now = std::time::Instant::now();
        while now.elapsed().lt(&Duration::from_secs(60)) {
            let tip = node0.get_tip_block();
            let indexer_tip = node0.rpc_client().get_indexer_tip().unwrap();
            let indexer_tip_number: u64 = indexer_tip.block_number.into();
            info!(
                "node0 tip: {}-{}, indexer-tip:{}-{}",
                tip.number(),
                tip.hash(),
                indexer_tip_number,
                indexer_tip.block_hash
            );
            if tip.hash() == indexer_tip.block_hash.into() {
                break;
            }
            sleep(Duration::from_secs(3));
        }

        // {
        //     let (block, uncle) = node1.construct_uncle();
        //     info!(
        //         "=========== constructed :\nblock:{}-{}\nuncle:{}-{}\n ",
        //         block.number(),
        //         block.hash(),
        //         uncle.number(),
        //         uncle.hash(),
        //     );

        //     node1.process_block_without_verify(&uncle, false);
        //     connect_all(nodes);
        //     waiting_for_sync(nodes);
        //     sleep(Duration::from_secs(15));
        //     info!("node0 indexer tip: {}", print_indexer_tip(node0));

        //     node1.process_block_without_verify(&block, false);

        //     node2.process_block_without_verify(&block, false);
        //     node2.process_block_without_verify(&uncle, false);
        //     node2.mine(1);
        //     connect_all(nodes);
        // }

        // waiting_for_sync(nodes);
        // sleep(Duration::from_secs(15));
        // info!("node0 indexer tip: {}", print_indexer_tip(node0));
        // {
        //     info!("checking node0's tip and indexer tip");
        //     let tip = node0.get_tip_block();
        //     info!(
        //         "node0 tip: {}-{}, indexer-tip:{}",
        //         tip.number(),
        //         tip.hash(),
        //         print_indexer_tip(node0)
        //     );
        // }
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        // Disable automatic peer discovery to control test environment
        config.network.connect_outbound_interval_secs = 100_000;
        config.network.discovery_local_address = false;

        // Aggressive mining settings to increase competition
        // config.tx_pool.min_fee_rate = 0.into();
        // config.tx_pool.max_tx_pool_size = 1000;
    }
}
