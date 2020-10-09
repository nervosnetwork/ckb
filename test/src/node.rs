use crate::global::binary;
use crate::rpc::RpcClient;
use crate::utils::{find_available_port, temp_path, wait_until};
use crate::{DEFAULT_TX_PROPOSAL_WINDOW, SYSTEM_CELL_ALWAYS_SUCCESS_INDEX};
use ckb_app_config::CKBAppConfig;
use ckb_chain_spec::consensus::Consensus;
use ckb_chain_spec::ChainSpec;
use ckb_jsonrpc_types::TxPoolInfo;
use ckb_types::{
    bytes::Bytes,
    core::{
        self, capacity_bytes, BlockBuilder, BlockNumber, BlockView, Capacity, HeaderView,
        ScriptHashType, TransactionView,
    },
    packed::{Block, Byte32, CellDep, CellInput, CellOutput, CellOutputBuilder, OutPoint, Script},
    prelude::*,
};
use std::collections::HashSet;
use std::convert::Into;
use std::fs;
use std::path::PathBuf;
use std::process::{self, Child, Command, Stdio};
use std::thread::sleep;
use std::time::Duration;
use std::time::Instant;

struct ProcessGuard(pub Child);

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        match self.0.kill() {
            Err(e) => log::error!("Could not kill ckb process: {}", e),
            Ok(_) => log::debug!("Successfully killed ckb process"),
        }
        let _ = self.0.wait();
    }
}

pub struct Node {
    working_dir: PathBuf,
    consensus: Consensus,
    p2p_listen: String,
    rpc_client: RpcClient,

    node_id: Option<String>,     // initialize when starts node
    guard: Option<ProcessGuard>, // initialize when starts node
}

impl Node {
    pub fn new(spec_name: &str, node_name: &str) -> Self {
        let working_dir = temp_path(spec_name, node_name);
        // log::info!("New {}-{} on: {}", spec_name, node_name, working_dir);

        // Copy node template into node's working directory
        let cells_dir = working_dir.join("specs").join("cells");
        fs::create_dir_all(cells_dir).expect("create node's dir");
        for file in &[
            "ckb.toml",
            "specs/integration.toml",
            "specs/cells/always_success",
        ] {
            let src = PathBuf::from("template").join(file);
            let dest = working_dir.join(file);
            fs::copy(&src, &dest)
                .unwrap_or_else(|_| panic!("cp {:?} {}", src.display(), dest.display()));
        }

        // Allocate rpc port and p2p port, and fill into app config
        let mut node = Self::init(working_dir);
        node.modify_app_config(|app_config| {
            let rpc_port = find_available_port();
            let p2p_port = find_available_port();
            app_config.rpc.listen_address = format!("127.0.0.1:{}", rpc_port);
            app_config.network.listen_addresses =
                vec![format!("/ip4/127.0.0.1/tcp/{}", p2p_port).parse().unwrap()];
        });

        node
    }

    pub fn modify_app_config<M>(&mut self, modifier: M)
    where
        M: Fn(&mut CKBAppConfig),
    {
        let app_config_path = self.working_dir().join("ckb.toml");
        let mut app_config: CKBAppConfig = {
            let toml = fs::read(&app_config_path).unwrap();
            toml::from_slice(&toml).unwrap()
        };
        modifier(&mut app_config);
        fs::write(&app_config_path, toml::to_string(&app_config).unwrap()).unwrap();

        *self = Self::init(self.working_dir());
    }

    pub fn modify_chain_spec<M>(&mut self, modifier: M)
    where
        M: Fn(&mut ChainSpec),
    {
        let ckb_spec_path = self.working_dir().join("specs/integration.toml");
        let mut chain_spec = {
            let toml = fs::read(&ckb_spec_path).unwrap();
            toml::from_slice(&toml).unwrap()
        };
        modifier(&mut chain_spec);
        fs::write(&ckb_spec_path, toml::to_string(&chain_spec).unwrap()).unwrap();

        *self = Self::init(self.working_dir());
    }

    // Initialize Node instance based on working directory
    fn init(working_dir: PathBuf) -> Self {
        let app_config: CKBAppConfig = {
            let app_config_path = working_dir.join("ckb.toml");
            let toml = fs::read(app_config_path).unwrap();
            toml::from_slice(&toml).unwrap()
        };
        let mut chain_spec: ChainSpec = {
            let chain_spec_path = working_dir.join("specs/integration.toml");
            let toml = fs::read(chain_spec_path).unwrap();
            toml::from_slice(&toml).unwrap()
        };

        let p2p_listen = app_config.network.listen_addresses[0].to_string();
        let rpc_address = app_config.rpc.listen_address;
        let rpc_client = RpcClient::new(&format!("http://{}/", rpc_address));
        let consensus = {
            // Ensure the data path is available because chain_spec.build_consensus() needs to access the
            // system-cell data.
            chain_spec
                .genesis
                .system_cells
                .iter_mut()
                .for_each(|system_cell| {
                    system_cell.file.absolutize(&working_dir.join("specs"));
                });
            chain_spec.build_consensus().unwrap()
        };
        Self {
            working_dir,
            consensus,
            p2p_listen,
            rpc_client,
            node_id: None,
            guard: None,
        }
    }

    pub fn rpc_client(&self) -> &RpcClient {
        &self.rpc_client
    }

    fn working_dir(&self) -> PathBuf {
        self.working_dir.clone()
    }

    pub fn log_path(&self) -> PathBuf {
        self.working_dir().join("data/logs/run.log")
    }

    pub fn node_id(&self) -> &str {
        // peer_id.to_base58()
        self.node_id.as_ref().expect("uninitialized node_id")
    }

    pub fn consensus(&self) -> &Consensus {
        &self.consensus
    }

    pub fn p2p_listen(&self) -> String {
        self.p2p_listen.clone()
    }

    pub fn p2p_address(&self) -> String {
        format!("{}/p2p/{}", self.p2p_listen(), self.node_id())
    }

    pub fn dep_group_tx_hash(&self) -> Byte32 {
        self.consensus().genesis_block().transactions()[1].hash()
    }

    pub fn always_success_script(&self) -> Script {
        let always_success_raw = self.consensus().genesis_block().transactions()[0]
            .outputs_data()
            .get(SYSTEM_CELL_ALWAYS_SUCCESS_INDEX as usize)
            .unwrap()
            .raw_data();
        let always_success_code_hash = CellOutput::calc_data_hash(&always_success_raw);
        Script::new_builder()
            .code_hash(always_success_code_hash)
            .hash_type(ScriptHashType::Data.into())
            .build()
    }

    pub fn always_success_cell_dep(&self) -> CellDep {
        let genesis_cellbase_hash = self.consensus().genesis_block().transactions()[0].hash();
        let always_success_out_point =
            OutPoint::new(genesis_cellbase_hash, SYSTEM_CELL_ALWAYS_SUCCESS_INDEX);
        CellDep::new_builder()
            .out_point(always_success_out_point)
            .build()
    }

    pub fn connect(&self, peer: &Self) {
        self.rpc_client()
            .add_node(peer.node_id().to_string(), peer.p2p_address());
        let connected = wait_until(5, || {
            self.rpc_client()
                .get_peers()
                .iter()
                .any(|p| p.node_id == peer.node_id())
        });
        if !connected {
            panic!("Connect outbound peer timeout, node id: {}", peer.node_id());
        }
    }

    pub fn connect_uncheck(&self, peer: &Self) {
        self.rpc_client()
            .add_node(peer.node_id().to_string(), peer.p2p_address());
    }

    // workaround for banned address checking (because we are using loopback address)
    // 1. checking banned addresses is empty
    // 2. connecting outbound peer and checking banned addresses is not empty
    // 3. clear banned addresses
    pub fn connect_and_wait_ban(&self, peer: &Self) {
        let rpc_client = self.rpc_client();
        assert!(
            rpc_client.get_banned_addresses().is_empty(),
            "banned addresses should be empty"
        );
        rpc_client.add_node(peer.node_id().to_string(), peer.p2p_address());
        let result = wait_until(10, || {
            let banned_addresses = rpc_client.get_banned_addresses();
            let result = !banned_addresses.is_empty();
            banned_addresses.into_iter().for_each(|ban_address| {
                rpc_client.set_ban(ban_address.address, "delete".to_owned(), None, None, None)
            });
            result
        });
        if !result {
            panic!(
                "Connect and wait ban outbound peer timeout, node id: {}",
                peer.node_id()
            );
        }
    }

    // TODO it will be removed out later, in another PR
    pub fn disconnect(&self, peer: &Self) {
        self.rpc_client().remove_node(peer.node_id().to_string());
        let disconnected = wait_until(5, || {
            self.rpc_client()
                .get_peers()
                .iter()
                .all(|p| p.node_id != peer.node_id())
                && peer
                    .rpc_client()
                    .get_peers()
                    .iter()
                    .all(|p| p.node_id != self.node_id())
        });
        if !disconnected {
            panic!("Disconnect timeout, node {}", peer.node_id());
        }
    }

    // TODO it will be removed out later, in another PR
    pub fn waiting_for_sync(&self, peer: &Self, target: BlockNumber) {
        let (mut self_tip_number, mut peer_tip_number) = (0, 0);
        // 60 seconds is a reasonable timeout to sync, even for poor CI server
        let synced = wait_until(60, || {
            self_tip_number = self.get_tip_block_number();
            peer_tip_number = peer.get_tip_block_number();
            self_tip_number == peer_tip_number && target == self_tip_number
        });
        if !synced {
            panic!(
                "Waiting for sync timeout, self_tip_number: {}, node_tip_number: {}",
                self_tip_number, peer_tip_number
            );
        }
    }

    pub fn submit_block(&self, block: &BlockView) -> Byte32 {
        self.rpc_client()
            .submit_block("".to_owned(), block.data().into())
            .unwrap()
    }

    pub fn process_block_without_verify(&self, block: &BlockView, broadcast: bool) -> Byte32 {
        self.rpc_client()
            .process_block_without_verify(block.data().into(), broadcast)
            .unwrap()
    }

    pub fn generate_blocks(&self, blocks_num: usize) -> Vec<Byte32> {
        (0..blocks_num).map(|_| self.generate_block()).collect()
    }

    pub fn generate_blocks_until_contains_valid_cellbase(&self) -> Vec<Byte32> {
        self.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize)
    }

    // generate a new block and submit it through rpc.
    pub fn generate_block(&self) -> Byte32 {
        self.rpc_client().generate_block()
    }

    // Convenient way to construct an uncle block
    pub fn construct_uncle(&self) -> BlockView {
        let block = self.new_block(None, None, None);
        // Make sure the uncle block timestamp is different from
        // the next block timestamp in main fork.
        // Firstly construct uncle block which timestamp
        // is less than the current time, and then generate
        // the new block in main fork which timestamp is greater than
        // or equal to the current time.
        let timestamp = block.timestamp();
        loop {
            let timestamp_next: u64 = self
                .rpc_client()
                .get_block_template(None, None, None)
                .current_time
                .into();
            if timestamp_next > timestamp {
                break;
            }
        }
        block
            .as_advanced_builder()
            .timestamp(timestamp.pack())
            .build()
    }

    // generate a transaction which spend tip block's cellbase and send it to pool through rpc.
    pub fn generate_transaction(&self) -> Byte32 {
        self.submit_transaction(&self.new_transaction_spend_tip_cellbase())
    }

    // generate a transaction which spend tip block's cellbase
    pub fn new_transaction_spend_tip_cellbase(&self) -> TransactionView {
        let block = self.get_tip_block();
        let cellbase = &block.transactions()[0];
        self.new_transaction(cellbase.hash())
    }

    pub fn submit_transaction(&self, transaction: &TransactionView) -> Byte32 {
        self.rpc_client()
            .send_transaction(transaction.data().into())
    }

    pub fn get_tip_block(&self) -> BlockView {
        let rpc_client = self.rpc_client();
        let tip_number = rpc_client.get_tip_block_number();
        rpc_client
            .get_block_by_number(tip_number)
            .expect("tip block exists")
            .into()
    }

    pub fn get_tip_block_number(&self) -> BlockNumber {
        self.rpc_client().get_tip_block_number()
    }

    pub fn get_block(&self, hash: Byte32) -> BlockView {
        self.rpc_client()
            .get_block(hash)
            .expect("block exists")
            .into()
    }

    pub fn get_block_by_number(&self, number: BlockNumber) -> BlockView {
        self.rpc_client()
            .get_block_by_number(number)
            .expect("block exists")
            .into()
    }

    pub fn get_header_by_number(&self, number: BlockNumber) -> HeaderView {
        self.rpc_client()
            .get_header_by_number(number)
            .expect("header exists")
            .into()
    }

    /// The states of chain and txpool are updated asynchronously. Which means that the chain has
    /// updated to the newest tip but txpool not.
    /// get_tip_tx_pool_info wait to ensure the txpool update to the newest tip as well.
    pub fn get_tip_tx_pool_info(&self) -> TxPoolInfo {
        let tip_header = self.rpc_client().get_tip_header();
        let tip_hash = &tip_header.hash;
        let instant = Instant::now();
        let mut recent = TxPoolInfo::default();
        while instant.elapsed() < Duration::from_secs(10) {
            let tx_pool_info = self.rpc_client().tx_pool_info();
            if &tx_pool_info.tip_hash == tip_hash {
                return tx_pool_info;
            }
            recent = tx_pool_info;
        }
        panic!(
            "timeout to get_tip_tx_pool_info, tip_header={:?}, tx_pool_info: {:?}",
            tip_header, recent
        );
    }

    pub fn new_block(
        &self,
        bytes_limit: Option<u64>,
        proposals_limit: Option<u64>,
        max_version: Option<u32>,
    ) -> BlockView {
        self.new_block_builder(bytes_limit, proposals_limit, max_version)
            .build()
    }

    pub fn new_block_builder(
        &self,
        bytes_limit: Option<u64>,
        proposals_limit: Option<u64>,
        max_version: Option<u32>,
    ) -> BlockBuilder {
        let template =
            self.rpc_client()
                .get_block_template(bytes_limit, proposals_limit, max_version);

        Block::from(template).as_advanced_builder()
    }

    pub fn new_transaction(&self, hash: Byte32) -> TransactionView {
        self.new_transaction_with_since(hash, 0)
    }

    pub fn new_transaction_with_since(&self, hash: Byte32, since: u64) -> TransactionView {
        self.new_transaction_with_since_capacity(hash, since, capacity_bytes!(100))
    }

    pub fn new_transaction_with_since_capacity(
        &self,
        hash: Byte32,
        since: u64,
        capacity: Capacity,
    ) -> TransactionView {
        let always_success_cell_dep = self.always_success_cell_dep();
        let always_success_script = self.always_success_script();

        core::TransactionBuilder::default()
            .cell_dep(always_success_cell_dep)
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity.pack())
                    .lock(always_success_script)
                    .build(),
            )
            .output_data(Default::default())
            .input(CellInput::new(OutPoint::new(hash, 0), since))
            .build()
    }

    pub fn new_transaction_with_fee_and_size(
        &self,
        parent_tx: &TransactionView,
        fee: Capacity,
        tx_size: usize,
    ) -> TransactionView {
        let input_capacity: Capacity = parent_tx
            .outputs()
            .get(0)
            .expect("parent output")
            .capacity()
            .unpack();
        let capacity = input_capacity.safe_sub(fee).unwrap();
        let tx = self.new_transaction_with_since_capacity(parent_tx.hash(), 0, capacity);
        let original_tx_size = tx.data().serialized_size_in_block();
        tx.as_advanced_builder()
            .set_outputs_data(vec![
                Bytes::from(vec![0u8; tx_size - original_tx_size]).pack()
            ])
            .build()
    }

    pub fn assert_tx_pool_size(&self, pending_size: u64, proposed_size: u64) {
        let tx_pool_info = self.get_tip_tx_pool_info();
        assert_eq!(tx_pool_info.pending.value(), pending_size);
        assert_eq!(tx_pool_info.proposed.value(), proposed_size);
    }

    pub fn assert_tx_pool_statics(&self, total_tx_size: u64, total_tx_cycles: u64) {
        let tx_pool_info = self.get_tip_tx_pool_info();
        assert_eq!(tx_pool_info.total_tx_size.value(), total_tx_size);
        assert_eq!(tx_pool_info.total_tx_cycles.value(), total_tx_cycles);
    }

    pub fn assert_tx_pool_cycles(&self, total_tx_cycles: u64) {
        let tx_pool_info = self.get_tip_tx_pool_info();
        assert_eq!(tx_pool_info.total_tx_cycles.value(), total_tx_cycles);
    }

    pub fn assert_tx_pool_serialized_size(&self, total_tx_size: u64) {
        let tx_pool_info = self.get_tip_tx_pool_info();
        assert_eq!(tx_pool_info.total_tx_size.value(), total_tx_size);
    }

    pub fn start(&mut self) {
        let mut child_process = Command::new(binary())
            .env("RUST_BACKTRACE", "full")
            .args(&[
                "-C",
                &self.working_dir().to_string_lossy().to_string(),
                "run",
                "--ba-advanced",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("failed to run binary");

        // Wait to ensure the node threads up
        let node_info = loop {
            if let Ok(local_node_info) = self.rpc_client().inner().local_node_info() {
                let _ = self.rpc_client().tx_pool_info();
                break local_node_info;
            }

            match child_process.try_wait() {
                Ok(None) => sleep(std::time::Duration::from_secs(1)),
                Ok(Some(status)) => {
                    log::error!(
                        "Error: node crashed: {}, log_path: {}",
                        status,
                        self.log_path().display()
                    );
                    process::exit(status.code().unwrap());
                }
                Err(error) => {
                    log::error!(
                        "Error: node crashed with reason: {}, log_path: {}",
                        error,
                        self.log_path().display()
                    );
                    process::exit(255);
                }
            }
        };

        self.guard = Some(ProcessGuard(child_process));
        self.node_id = Some(node_info.node_id);
    }

    pub fn stop(&mut self) {
        drop(self.guard.take())
    }

    pub fn export(&self, target: String) {
        Command::new(binary())
            .args(&[
                "export",
                "-C",
                &self.working_dir().to_string_lossy().to_string(),
                &target,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .output()
            .expect("failed to execute process");
    }

    pub fn import(&self, target: String) {
        Command::new(binary())
            .args(&[
                "import",
                "-C",
                &self.working_dir().to_string_lossy().to_string(),
                &target,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .output()
            .expect("failed to execute process");
    }
}

// TODO it will be removed out later, in another PR
pub fn connect_all(nodes: &[Node]) {
    for node_a in nodes.iter() {
        for node_b in nodes.iter() {
            if node_a.p2p_address() != node_b.p2p_address() {
                node_a.connect(node_b);
            }
        }
    }
}

// TODO it will be removed out later, in another PR
pub fn disconnect_all(nodes: &[Node]) {
    for node_a in nodes.iter() {
        for node_b in nodes.iter() {
            if node_a.p2p_address() != node_b.p2p_address() {
                node_a.disconnect(node_b);
            }
        }
    }
}

// TODO it will be removed out later, in another PR
// generate a same block on all nodes, exit IBD mode and return the tip block
pub fn exit_ibd_mode(nodes: &[Node]) -> BlockView {
    let block = nodes[0].new_block(None, None, None);
    nodes.iter().for_each(|node| {
        node.submit_block(&block);
    });
    block
}

// TODO it will be removed out later, in another PR
pub fn waiting_for_sync(nodes: &[Node]) {
    let mut tip_headers: HashSet<HeaderView> = HashSet::with_capacity(nodes.len());
    // 60 seconds is a reasonable timeout to sync, even for poor CI server
    let synced = wait_until(60, || {
        tip_headers = nodes
            .iter()
            .map(|node| node.rpc_client().get_tip_header().into())
            .collect();
        tip_headers.len() == 1
    });
    if !synced {
        panic!("timeout to wait for sync, tip_headers: {:?}", tip_headers);
    }
}
