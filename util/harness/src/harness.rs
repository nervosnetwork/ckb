use super::test_node::TestNode;
use std::cmp;
use std::path::PathBuf;
use std::{env, io};
use tempdir::TempDir;
use toml;
use toml::value::Table;

// The maximum number of nodes a single test can spawn
pub const MAX_NODES: usize = 8;
// rpc or p2p ports lowerbond
pub const PORT_MIN: usize = 11000;
// ports gap between rpc and p2p
pub const PORT_GAP: usize = 5000;

/// Test harness for ckb
///
/// contains:
/// - config builder for setup ckb node
/// - test node wrapper
/// - P2P connections
///
/// As such the harness meant to serve as an easily expandable test harness
/// when do black box testing
pub struct TestHarness {
    pub num_nodes: usize,
    pub nodes: Vec<TestNode>,
}

pub struct ConfigBuilder {
    config: Table,
}

impl ConfigBuilder {
    fn new() -> ConfigBuilder {
        let toml = toml! {
            [ckb]
            chain = "dev"

            [logger]
            file = "ckb.log"
            filter = "info"
            color = true

            [network]
            listen_addr = "0.0.0.0:0"
            boot_nodes = []
            reserved_nodes = []
            only_reserved_peers = false
            min_peers = 4
            max_peers = 8
            secret_file = "secret"
            nodes_file = "nodes.json"

            [rpc]
            listen_addr = "0.0.0.0:0"

            [sync]
            verification_level = "Full"
            orphan_block_limit = 1024

            [pool]
            max_pool_size = 65535
            max_proposal_size = 4095
            max_commit_size = 4096

            [miner]
            max_tx              = 1024
            new_transactions_threshold = 8
            redeem_script_hash  = "0x6463e95f5f1f15415962563b0d4227635d8ae2a74137afbe3e052ef1f9470074"
        };

        let config = match toml {
            toml::Value::Table(config) => config,
            _ => unreachable!(),
        };

        ConfigBuilder { config }
    }

    pub fn network_addr(mut self, addr: String) -> Self {
        *self
            .config
            .get_mut("network")
            .unwrap()
            .get_mut("listen_addr")
            .unwrap() = toml::Value::String(addr);
        self
    }

    pub fn rpc_addr(mut self, addr: String) -> Self {
        *self
            .config
            .get_mut("rpc")
            .unwrap()
            .get_mut("listen_addr")
            .unwrap() = toml::Value::String(addr);
        self
    }

    pub fn build(self) -> Table {
        let ConfigBuilder { config } = self;
        config
    }
}

impl TestHarness {
    pub fn new(num_nodes: usize) -> TestHarness {
        let num_nodes = cmp::min(num_nodes, MAX_NODES);
        TestHarness {
            num_nodes,
            nodes: vec![],
        }
    }

    pub fn start(&mut self) {
        self.nodes.clear();
        self.nodes.extend((0..self.num_nodes).map(|i| {
            let config = ConfigBuilder::new()
                .network_addr(format!("0.0.0.0:{}", PORT_MIN + i))
                .rpc_addr(format!("0.0.0.0:{}", PORT_MIN + PORT_GAP + i))
                .build();
            let base = temp_datadir_path("test_node", i).expect("temp datadir");
            TestNode::new(config, i, base, binary_path())
        }));
    }
}

fn temp_datadir_path(prefix: &str, index: usize) -> io::Result<TempDir> {
    TempDir::new(&format!("{}_{}", prefix, index))
}

fn binary_path() -> PathBuf {
    env::var("CKB").map(PathBuf::from).expect("binary path")
}
