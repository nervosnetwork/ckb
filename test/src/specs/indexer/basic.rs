use crate::node::{connect_all, disconnect_all, waiting_for_sync};
use crate::util::mining::out_ibd_mode;
use crate::utils::find_available_port;
use crate::{Node, Spec};
use ckb_logger::{info, warn};
use ckb_types::packed;
use postgresql_embedded::{Settings, blocking::PostgreSQL};
use std::thread::sleep;
use std::time::Duration;

/// Test case to reproduce the rich-indexer chain reorganization bug
///
/// This test creates a chain fork scenario with 2 nodes:
/// 1. Node0 and Node1 both mine independently to create competing chains
/// 2. Trigger chain reorganization by connecting nodes
/// 3. Check if rich-indexer's tip updates correctly to follow the main chain
pub struct RichIndexerChainReorgBug;

impl Spec for RichIndexerChainReorgBug {
    fn before_run(&self) -> Vec<Node> {
        let node0 = Node::new(self.name(), "node0");
        let node1 = Node::new(self.name(), "node1");
        let mut nodes = [node0, node1];

        // Setup embedded PostgreSQL
        info!("Setting up embedded PostgreSQL for rich-indexer");
        let postgres_port = find_available_port();
        let mut settings = Settings::default();
        settings.port = postgres_port;
        settings.username = "postgres".to_string();
        settings.password = "password".to_string();
        // Make Postgres emit statements and durations to stderr
        settings
            .configuration
            .insert("log_destination".into(), "stderr".into());
        // Don't capture into files; send to stderr
        settings
            .configuration
            .insert("logging_collector".into(), "off".into());
        // Log every statement (alternatives: ddl | mod | none)
        settings
            .configuration
            .insert("log_statement".into(), "all".into());
        // Also log duration of every completed statement (0 ms threshold)
        settings
            .configuration
            .insert("log_min_duration_statement".into(), "0".into());

        info!("setitngs; {:?}", settings);
        let mut postgresql = PostgreSQL::new(settings);
        postgresql.setup().expect("Failed to setup PostgreSQL");
        postgresql.start().expect("Failed to start PostgreSQL");

        // Enable rich-indexer only on node0
        {
            let node0 = &mut nodes[0];

            node0.modify_app_config(|config| {
                // Configure rich-indexer to use PostgreSQL
                config.rpc.modules.push(ckb_app_config::RpcModule::Indexer);
                info!("rpc.modules:{:?}", config.rpc.modules);
                config.indexer.rich_indexer = ckb_app_config::RichIndexerConfig {
                    db_type: ckb_app_config::DBDriver::Postgres,
                    db_host: "127.0.0.1".to_string(),
                    db_port: postgres_port,
                    db_user: "postgres".to_string(),
                    db_password: "password".to_string(),
                    db_name: "ckb_rich_indexer_test".to_string(),
                    ..Default::default()
                };

                // Configure faster polling to increase chance of race conditions
                config.indexer.poll_interval = 1;
                config.indexer.index_tx_pool = false;
            });
        }

        nodes.iter_mut().for_each(|node| {
            node.start();
        });

        // Store postgresql instance for cleanup (in a real implementation,
        // we'd store this properly for cleanup in a Drop impl)
        info!("PostgreSQL started on port {}", postgres_port);

        nodes.to_vec()
    }

    /// Reproduces the rich-indexer chain reorganization bug
    ///
    /// Timeline:
    /// 1. Both nodes mine independently to create fork
    /// 2. Node0 mines shorter chain, Node1 mines longer chain  
    /// 3. Connect nodes to trigger chain reorganization
    /// 4. Check if rich-indexer tip updates correctly
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];

        info!("=== Phase 1: Setup independent mining ===");
        out_ibd_mode(nodes);

        // Create shared history
        node1.connect(node0);
        node0.mine(1);
        node1.mine(1);

        waiting_for_sync(&[node0, node1]);
        info!(
            "Both nodes synced to height {}",
            node0.get_tip_block_number()
        );

        info!("=== Phase 2: Create competing chains ===");

        let node_dbg = |height: Option<u64>| {
            nodes.iter().enumerate().for_each(|(id, node)| {
                if let Some(h) = height {
                    let block = node.get_block_by_number(h);
                    info!(
                        "Node{} block at height {}: hash={}, parent={}",
                        id,
                        h,
                        block.hash(),
                        block.parent_hash()
                    );
                } else {
                    if id == 0 {
                        let indexer_tip = node
                            .rpc_client()
                            .get_indexer_tip()
                            .expect("must get indexer tip");
                        let indexer_tip_number: u64 = indexer_tip.block_number.into();
                        let indexer_tip_hash: packed::Byte32 = indexer_tip.block_hash.into();
                        info!(
                            "Node{} indexer: {}-{}",
                            id, indexer_tip_number, indexer_tip_hash,
                        );
                    }
                    let tip = node.get_tip_block();
                    info!("Node{} tip: height {}-{}", id, tip.number(), tip.hash(),);
                }
            });
        };

        let now = std::time::Instant::now();
        while now.elapsed().le(&Duration::from_secs(600)) {
            info!("create forking..............................................");
            disconnect_all(&nodes);
            node0.mine(1);
            node1.mine(1);
            let base_height = node0.get_tip_block_number();
            node_dbg(Some(base_height));
            node1.mine(1);
            connect_all(&nodes);
            waiting_for_sync(nodes);
            node_dbg(None);
        }

        info!("Fork created:");
        node_dbg(None);

        info!("=== Phase 3: Check rich-indexer before reorganization ===");
        let indexer_tip_before = node0.rpc_client().get_indexer_tip().unwrap();
        info!(
            "Rich-indexer tip before reorg: {}-{}",
            indexer_tip_before.block_number, indexer_tip_before.block_hash
        );

        info!("=== Phase 4: Trigger chain reorganization ===");
        waiting_for_sync(&[node0, node1]);

        info!("After sync");
        nodes.iter().enumerate().for_each(|(id, node)| {
            let tip = node.get_tip_block();
            info!(
                "Node {} tip: height {} -> {:?}",
                id,
                tip.number(),
                tip.hash()
            );
        });

        let final_tip = node0.get_tip_block().number();

        // Wait for rich-indexer to catch up
        // sleep(Duration::from_secs(5));

        info!("=== Phase 5: Verify rich-indexer follows chain reorganization ===");
        let mut retry_count = 0;
        let max_retries = 10;

        loop {
            let indexer_tip_after = node0.rpc_client().get_indexer_tip().unwrap();
            info!(
                "Rich-indexer tip after reorg: {}-{}",
                indexer_tip_after.block_number, indexer_tip_after.block_hash
            );

            if indexer_tip_after.block_number == final_tip.into() {
                info!("✅ SUCCESS: Rich-indexer tip matches chain tip");
                info!("  Chain tip: {}", final_tip);
                info!("  Rich-indexer tip: {}", indexer_tip_after.block_number);
                break;
            } else {
                warn!(
                    "Rich-indexer tip ({}) != chain tip ({})",
                    indexer_tip_after.block_number, final_tip
                );
            }

            retry_count += 1;
            if retry_count >= max_retries {
                warn!(
                    "❌ FAILED: Rich-indexer did not catch up within {} retries",
                    max_retries
                );
                warn!("This indicates the rich-indexer chain reorganization bug!");
                break;
            }

            info!(
                "Waiting for rich-indexer to catch up... (retry {}/{})",
                retry_count, max_retries
            );
            sleep(Duration::from_secs(2));
        }
    }

    // Disable node discovery for controlled test environment
    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        config.network.connect_outbound_interval_secs = 100_000;
    }
}
