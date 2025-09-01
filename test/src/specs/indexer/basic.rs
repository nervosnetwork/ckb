use crate::node::{connect_all, disconnect_all, waiting_for_sync};
use crate::util::cell::gen_spendable;
use crate::util::mining::out_ibd_mode;
use crate::util::transaction::always_success_transactions;
use crate::utils::find_available_port;
use crate::{Node, Spec};
use ckb_logger::{info, warn};
use ckb_types::packed;
use postgresql_embedded::{Settings, blocking::PostgreSQL};
use std::cell::RefCell;
use std::thread::sleep;
use std::time::Duration;

/// Test case to reproduce the rich-indexer chain reorganization bug
///
/// This test creates a chain fork scenario with 2 nodes:
/// 1. Node0 and Node1 both mine independently to create competing chains
/// 2. Trigger chain reorganization by connecting nodes
/// 3. Check if rich-indexer's tip updates correctly to follow the main chain
#[derive(Default)]
pub struct RichIndexerChainReorgBug {
    postgresql: RefCell<Option<PostgreSQL>>,
}

impl Spec for RichIndexerChainReorgBug {
    fn before_run(&self) -> Vec<Node> {
        info!("RichIndexerChainReorgBug: before_run");
        {
            tracing::info!(
                "RUST_LOG is {}",
                std::env::var("RUST_LOG").unwrap_or_default()
            );
        }
        tracing::info!("......................................");
        tracing::info!("Tracing::info ...");
        tracing::debug!("Tracing::debug ...");
        tracing::info!("......................................");
        let node0 = Node::new(self.name(), "node0");
        let node1 = Node::new(self.name(), "node1");
        let mut nodes = [node0, node1];

        // Setup embedded PostgreSQL
        info!("Setting up embedded PostgreSQL for rich-indexer");
        let postgres_port = find_available_port();
        let mut settings = Settings::default();
        settings.port = postgres_port;
        settings.temporary = true;
        settings.username = "postgres".to_string();
        settings.password = "postgres,,".to_string();
        // Make Postgres emit statements and durations to stderr
        let configs = [
            ("log_directory", "/tmp/postgres"),
            ("log_filename", "tmp.log"),
            ("logging_collector", "on"),
            ("log_statement", "all"),
            ("auto_explain.log_min_duration", "0"),
        ];

        for (key, value) in configs {
            settings.configuration.insert(key.into(), value.into());
        }

        info!("setitngs; {:?}", settings);
        let mut postgresql = PostgreSQL::new(settings.clone());
        postgresql.setup().expect("Failed to setup PostgreSQL");
        postgresql.start().expect("Failed to start PostgreSQL");
        {
            // Store postgresql instance for cleanup (in a real implementation,
            // we'd store this properly for cleanup in a Drop impl)
            info!("PostgreSQL started on port {}", postgres_port);
            let status = postgresql.status();
            info!("PostgreSQL status: {:?}", status);
        }
        postgresql.create_database("ckb_rich_indexer_test").unwrap();
        info!("postgresql started.....................");

        // Store postgresql instance in the struct to keep it alive
        *self.postgresql.borrow_mut() = Some(postgresql);

        // Enable rich-indexer only on node0
        {
            info!("nodes count: {}", nodes.len());
            let node0 = &mut nodes[0];
            node0.modify_app_config(|config| {
                // Configure rich-indexer to use PostgreSQL
                config
                    .rpc
                    .modules
                    .push(ckb_app_config::RpcModule::RichIndexer);
                info!("rpc.modules:{:?}", config.rpc.modules);
                config.indexer.rich_indexer = ckb_app_config::RichIndexerConfig {
                    db_type: ckb_app_config::DBDriver::Postgres,
                    db_host: "127.0.0.1".to_string(),
                    db_port: postgres_port,
                    db_user: settings.clone().username.clone(),
                    db_password: settings.clone().password.clone(),
                    db_name: "ckb_rich_indexer_test".to_string(),
                    ..Default::default()
                };
                info!("rich_indexer: {:?}", config.indexer.rich_indexer);
                // config.logger.filter =
                //     Some("debug,tentacle=info,sled=info,tokio_yamux=info".to_string());
                config.logger.log_to_stdout = true;

                // Configure faster polling to increase chance of race conditions
                config.indexer.poll_interval = 1;
                config.indexer.index_tx_pool = false;
            });
        }
        {
            let node1 = &mut nodes[1];
            node1.modify_app_config(|config| {
                config.logger.log_to_stdout = false;
            });
        }

        nodes.iter_mut().for_each(|node| {
            node.start();
        });

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
        info!(
            "RichIndexerChainReorgBug: run.........................................................."
        );
        let node0 = &nodes[0];
        let node1 = &nodes[1];

        info!("nodes count: {}", nodes.len());
        // print nodes genesis block number and hash:
        nodes.iter().enumerate().for_each(|(id, node)| {
            let genesis = node.get_block_by_number(0);
            info!(
                "Node{} genesis block: number={}, hash={}",
                id,
                genesis.number(),
                genesis.hash()
            );
        });

        info!("=== Phase 1: Setup independent mining ===");
        out_ibd_mode(nodes);
        node1.connect(node0);
        node0.mine_until_out_bootstrap_period();

        waiting_for_sync(&[node0, node1]);
        info!(
            "Both nodes synced to height {}, {}",
            node0.get_tip_block_number(),
            node1.get_tip_block_number()
        );
        {
            let indexer_tip = node0.rpc_client().get_indexer_tip().unwrap();
            let indexer_tip_number: u64 = indexer_tip.block_number.into();
            info!("node0 rich-indexer tip: {}", indexer_tip_number);
        }

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
                    info!(
                        "Node{} tip: height {}-{}, txs: {}, block_size: {}",
                        id,
                        tip.number(),
                        tip.hash(),
                        tip.transactions().len(),
                        tip.data().total_size()
                    );
                }
            });
        };

        let gen_txs = |node: &Node| {
            let now = std::time::Instant::now();
            let cells = gen_spendable(node, 4000);
            let txs = always_success_transactions(node, &cells);
            txs.iter().for_each(|tx| {
                let tx_hash = tx.hash();
                let result = node.submit_transaction_with_result(&tx);
                // match result {
                //     Ok(tx_hash) => {
                //         info!("Node{} submitted tx {}", node.node_id(), tx_hash);
                //     }
                //     Err(err) => {
                //         warn!(
                //             "Node{} failed to submit tx: {}, {}",
                //             node.node_id(),
                //             tx_hash,
                //             err
                //         );
                //     }
                // }
            });
            info!("gen txs cost {}s", now.elapsed().as_secs());
        };

        gen_txs(node0);
        node0.mine(1);
        waiting_for_sync(nodes);

        let now = std::time::Instant::now();
        let mut iteration = 0;
        while now.elapsed().le(&Duration::from_secs(600)) {
            info!(
                "\n\n    Create forking_________________________    {}",
                iteration
            );
            gen_txs(node0);
            gen_txs(node1);

            std::thread::scope(|s| {
                let jh0 = s.spawn(|| node0.mine(1));
                let jh1 = s.spawn(|| node1.mine(1));

                jh0.join().unwrap();
                jh1.join().unwrap();
            });

            let base_height = node0.get_tip_block_number();
            node_dbg(Some(base_height));

            gen_txs(node1);
            node1.mine(1);
            waiting_for_sync(nodes);
            node_dbg(None);
            iteration += 1;
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
