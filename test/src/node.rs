use crate::global::binary;
use crate::rpc::RpcClient;
use crate::utils::{find_available_port, temp_path, wait_until};
use crate::{SYSTEM_CELL_ALWAYS_FAILURE_INDEX, SYSTEM_CELL_ALWAYS_SUCCESS_INDEX};
use ckb_app_config::{AppConfig, CKBAppConfig, ExitCode};
use ckb_chain_spec::ChainSpec;
use ckb_chain_spec::consensus::Consensus;
use ckb_error::AnyError;
use ckb_jsonrpc_types::{BlockFilter, BlockTemplate, TxPoolInfo};
use ckb_jsonrpc_types::{PoolTxDetailInfo, TxStatus};
use ckb_logger::{debug, error, info};
use ckb_network::multiaddr::Multiaddr;
use ckb_resource::Resource;
use ckb_shared::shared_builder::open_or_create_db;
use ckb_store::ChainDB;
use ckb_types::{
    bytes,
    core::{
        self, BlockBuilder, BlockNumber, BlockView, Capacity, HeaderView, ScriptHashType,
        TransactionView, capacity_bytes,
    },
    packed::{
        self, Block, Byte32, CellDep, CellInput, CellOutput, CellOutputBuilder, OutPoint, Script,
    },
    prelude::*,
};
use std::borrow::{Borrow, BorrowMut};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, RwLock};
use std::thread::sleep;
use std::time::{Duration, Instant};

pub(crate) struct ProcessGuard {
    pub name: String,
    pub child: Child,
    pub killed: bool,
}

impl ProcessGuard {
    pub(crate) fn is_alive(&mut self) -> bool {
        let try_wait = self.child.try_wait();
        match try_wait {
            Ok(status_op) => status_op.is_none(),
            Err(_err) => false,
        }
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        if !self.killed {
            match self.child.kill() {
                Err(e) => error!("Could not kill ckb process ({}): {}", self.name, e),
                Ok(_) => debug!("Successfully killed ckb process ({})", self.name),
            }
            let _ = self.child.wait();
        }
    }
}

#[derive(Clone)]
pub struct Node {
    inner: Arc<InnerNode>,
}

pub struct InnerNode {
    spec_node_name: String,
    working_dir: PathBuf,
    consensus: Consensus,
    p2p_listen: String,
    rpc_client: RpcClient,
    rpc_listen: String,

    node_id: RwLock<Option<String>>, // initialize when starts node
    guard: RwLock<Option<ProcessGuard>>, // initialize when starts node
}

impl Node {
    pub fn new(spec_name: &str, node_name: &str) -> Self {
        let working_dir = temp_path(spec_name, node_name);

        // Copy node template into node's working directory
        let cells_dir = working_dir.join("specs").join("cells");
        info!("working_dir {:?}", working_dir);

        fs::create_dir_all(cells_dir).expect("create node's dir");
        for file in &[
            "ckb.toml",
            "specs/integration.toml",
            "specs/cells/always_success",
            "specs/cells/always_failure",
        ] {
            let src = PathBuf::from("template").join(file);
            let dest = working_dir.join(file);
            fs::copy(&src, &dest)
                .unwrap_or_else(|_| panic!("cp {:?} {}", src.display(), dest.display()));
        }

        let spec_node_name = format!("{}_{}", spec_name, node_name);
        // Allocate rpc port and p2p port, and fill into app config
        let mut node = Self::init(working_dir, spec_node_name);
        node.modify_app_config(|app_config| {
            let rpc_port = find_available_port();
            let p2p_port = find_available_port();
            app_config.rpc.listen_address = format!("127.0.0.1:{rpc_port}");
            app_config.network.listen_addresses =
                vec![format!("/ip4/127.0.0.1/tcp/{p2p_port}").parse().unwrap()];
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
            CKBAppConfig::load_from_slice(&toml).unwrap()
        };
        modifier(&mut app_config);
        fs::write(&app_config_path, toml::to_string(&app_config).unwrap()).unwrap();

        *self = Self::init(self.working_dir(), self.inner.spec_node_name.clone());
    }

    pub fn modify_chain_spec<M>(&mut self, modifier: M)
    where
        M: Fn(&mut ChainSpec),
    {
        let chain_spec_path = self.working_dir().join("specs/integration.toml");
        let chain_spec_res = Resource::file_system(chain_spec_path.clone());
        let mut chain_spec = ChainSpec::load_from(&chain_spec_res).unwrap();
        modifier(&mut chain_spec);
        fs::write(&chain_spec_path, toml::to_string(&chain_spec).unwrap()).unwrap();

        *self = Self::init(self.working_dir(), self.inner.spec_node_name.clone());
    }

    // Initialize Node instance based on working directory
    fn init(working_dir: PathBuf, spec_node_name: String) -> Self {
        let app_config = {
            let app_config_path = working_dir.join("ckb.toml");
            let toml = fs::read(app_config_path).unwrap();
            CKBAppConfig::load_from_slice(&toml).unwrap()
        };
        let mut chain_spec: ChainSpec = {
            let chain_spec_path = working_dir.join("specs/integration.toml");
            let chain_spec_res = Resource::file_system(chain_spec_path);
            ChainSpec::load_from(&chain_spec_res).unwrap()
        };

        let p2p_listen = app_config.network.listen_addresses[0].to_string();
        let rpc_listen = format!("http://{}/", app_config.rpc.listen_address);
        let rpc_client = RpcClient::new(&rpc_listen);
        let consensus = {
            // Ensure the data path is available because chain_spec.build_consensus() needs to access the
            // system-cell data.
            chain_spec
                .genesis
                .system_cells
                .iter_mut()
                .for_each(|system_cell| {
                    system_cell.file.absolutize(working_dir.join("specs"));
                });
            chain_spec.build_consensus().unwrap()
        };
        Self {
            inner: Arc::new(InnerNode {
                spec_node_name,
                working_dir,
                consensus,
                p2p_listen,
                rpc_client,
                rpc_listen,
                node_id: RwLock::new(None),
                guard: RwLock::new(None),
            }),
        }
    }

    pub fn rpc_client(&self) -> &RpcClient {
        &self.inner.rpc_client
    }

    pub fn working_dir(&self) -> PathBuf {
        self.inner.working_dir.clone()
    }

    pub fn log_path(&self) -> PathBuf {
        self.working_dir().join("data/logs/run.log")
    }

    pub fn node_id(&self) -> String {
        // peer_id.to_base58()
        self.inner
            .node_id
            .read()
            .expect("read locked node_id")
            .clone()
            .expect("uninitialized node_id")
    }

    pub fn consensus(&self) -> &Consensus {
        &self.inner.consensus
    }

    pub fn p2p_listen(&self) -> String {
        self.inner.p2p_listen.clone()
    }

    pub fn rpc_listen(&self) -> String {
        self.inner.rpc_listen.clone()
    }

    pub fn get_onion_public_addr(&self) -> Option<String> {
        let onion_public_addr = self
            .rpc_client()
            .local_node_info()
            .addresses
            .iter()
            .filter(|addr| addr.address.contains("/onion3/"))
            .collect::<Vec<_>>()
            .first()
            .map(|addr| addr.address.clone());
        onion_public_addr
    }

    pub fn p2p_address(&self) -> String {
        format!("{}/p2p/{}", self.p2p_listen(), self.node_id())
    }

    pub fn dep_group_tx_hash(&self) -> Byte32 {
        self.consensus().genesis_block().transactions()[1].hash()
    }

    pub fn always_success_raw_data(&self) -> bytes::Bytes {
        self.consensus().genesis_block().transactions()[0]
            .outputs_data()
            .get(SYSTEM_CELL_ALWAYS_SUCCESS_INDEX as usize)
            .unwrap()
            .raw_data()
    }

    pub fn always_success_script(&self) -> Script {
        let always_success_raw = self.always_success_raw_data();
        let always_success_code_hash = CellOutput::calc_data_hash(&always_success_raw);
        Script::new_builder()
            .code_hash(always_success_code_hash)
            .hash_type(ScriptHashType::Data)
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

    pub fn always_failure_raw_data(&self) -> bytes::Bytes {
        self.consensus().genesis_block().transactions()[0]
            .outputs_data()
            .get(SYSTEM_CELL_ALWAYS_FAILURE_INDEX as usize)
            .unwrap()
            .raw_data()
    }

    pub fn always_failure_script(&self) -> Script {
        let always_failure_raw = self.always_success_raw_data();
        let always_failure_code_hash = CellOutput::calc_data_hash(&always_failure_raw);
        Script::new_builder()
            .code_hash(always_failure_code_hash)
            .hash_type(ScriptHashType::Data)
            .build()
    }

    pub fn always_failure_cell_dep(&self) -> CellDep {
        let genesis_cellbase_hash = self.consensus().genesis_block().transactions()[0].hash();
        let always_failure_out_point =
            OutPoint::new(genesis_cellbase_hash, SYSTEM_CELL_ALWAYS_FAILURE_INDEX);
        CellDep::new_builder()
            .out_point(always_failure_out_point)
            .build()
    }

    pub fn connect(&self, peer: &Self) {
        self.rpc_client()
            .add_node(peer.node_id(), peer.p2p_listen());
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

    pub fn connect_onion(&self, peer: &Self) {
        wait_until(30, || peer.get_onion_public_addr().is_some());

        let onion_pub_address = peer
            .get_onion_public_addr()
            .expect("peer onion address is not found");

        info!(
            "got peer:{}'s onion address: {}",
            peer.node_id(),
            onion_pub_address
        );

        self.rpc_client()
            .add_node(peer.node_id(), onion_pub_address);
        let connected = wait_until(180, || {
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
            .add_node(peer.node_id(), peer.p2p_listen());
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
        rpc_client.add_node(peer.node_id(), peer.p2p_listen());
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
        self.rpc_client().remove_node(peer.node_id());
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

    pub fn submit_block(&self, block: &BlockView) -> Byte32 {
        let hash = self
            .rpc_client()
            .submit_block("".to_owned(), block.data().into())
            .unwrap();
        self.wait_for_tx_pool();
        hash
    }

    pub fn process_block_without_verify(&self, block: &BlockView, broadcast: bool) -> Byte32 {
        self.rpc_client()
            .process_block_without_verify(block.data().into(), broadcast)
            .unwrap()
    }

    // Convenient way to construct an uncle block
    pub fn construct_uncle(&self) -> (BlockView, BlockView) {
        let block = self.new_block_without_uncles(None, None, None);
        // Make sure the uncle block timestamp is different from
        // the next block timestamp in main fork.
        // Firstly construct uncle block which timestamp
        // is less than the current time, and then generate
        // the new block in main fork which timestamp is greater than
        // or equal to the current time.
        let timestamp = block.timestamp();
        let uncle = block.as_advanced_builder().timestamp(timestamp - 1).build();
        (block, uncle)
    }

    // generate a transaction which spend tip block's cellbase and send it to pool through rpc.
    pub fn generate_transaction(&self) -> Byte32 {
        self.submit_transaction(&self.new_transaction_spend_tip_cellbase())
    }

    // generate a transaction which spend tip block's cellbase and capacity
    pub fn new_transaction_with_capacity(&self, capacity: Capacity) -> TransactionView {
        let block = self.get_tip_block();
        let cellbase = &block.transactions()[0];
        self.new_transaction_with_since_capacity(cellbase.hash(), 0, capacity)
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

    pub fn submit_transaction_with_result(
        &self,
        transaction: &TransactionView,
    ) -> Result<Byte32, AnyError> {
        let res = self
            .rpc_client()
            .send_transaction_result(transaction.data().into())?
            .into();
        Ok(res)
    }

    pub fn get_transaction(&self, tx_hash: Byte32) -> TxStatus {
        self.rpc_client().get_transaction(tx_hash).tx_status
    }

    pub fn remove_transaction(&self, tx_hash: Byte32) -> bool {
        self.rpc_client().remove_transaction(tx_hash)
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

    pub fn get_block_filter(&self, hash: Byte32) -> BlockFilter {
        self.rpc_client()
            .get_block_filter(hash)
            .expect("block filter exists")
    }

    pub fn get_pool_tx_detail_info(&self, hash: Byte32) -> PoolTxDetailInfo {
        self.rpc_client().get_pool_tx_detail_info(hash)
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
            "timeout to get_tip_tx_pool_info, tip_header={tip_header:?}, tx_pool_info: {recent:?}"
        );
    }

    pub fn wait_for_tx_pool(&self) {
        let rpc_client = self.rpc_client();
        let mut chain_tip = rpc_client.get_tip_header();
        let mut tx_pool_tip = rpc_client.tx_pool_info();
        if chain_tip.hash == tx_pool_tip.tip_hash {
            return;
        }
        let mut instant = Instant::now();
        while instant.elapsed() < Duration::from_secs(10) {
            sleep(std::time::Duration::from_millis(100));
            chain_tip = rpc_client.get_tip_header();
            let prev_tx_pool_tip = tx_pool_tip;
            tx_pool_tip = rpc_client.tx_pool_info();
            if chain_tip.hash == tx_pool_tip.tip_hash {
                return;
            } else if prev_tx_pool_tip.tip_hash != tx_pool_tip.tip_hash {
                instant = Instant::now();
            }
        }
        panic!(
            "timeout to wait for tx pool,\n\tchain   tip: {:?}, {:#x},\n\ttx-pool tip: {}, {:#x}",
            chain_tip.inner.number.value(),
            chain_tip.hash,
            tx_pool_tip.tip_number.value(),
            tx_pool_tip.tip_hash,
        );
    }

    pub fn wait_tx_pool_ready(&self) {
        let rpc_client = self.rpc_client();
        while !rpc_client.tx_pool_ready() {
            sleep(std::time::Duration::from_millis(200));
        }
    }

    pub fn new_block_without_uncles(
        &self,
        bytes_limit: Option<u64>,
        proposals_limit: Option<u64>,
        max_version: Option<u32>,
    ) -> BlockView {
        self.new_block_builder(bytes_limit, proposals_limit, max_version)
            .set_uncles(vec![])
            .build()
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

    pub fn new_block_with_blocking<B>(&self, blocking: B) -> BlockView
    where
        B: Fn(&BlockTemplate) -> bool,
    {
        self.new_block_builder_with_blocking(blocking).build()
    }

    pub fn new_block_builder_with_blocking<B>(&self, blocking: B) -> BlockBuilder
    where
        B: Fn(&BlockTemplate) -> bool,
    {
        let mut count = 0;
        let mut template = self.rpc_client().get_block_template(None, None, None);
        while blocking(&template) {
            sleep(Duration::from_millis(50));
            template = self.rpc_client().get_block_template(None, None, None);
            count += 1;

            if count > 100 {
                panic!("mine_with_blocking timeout");
            }
        }
        Block::from(template).as_advanced_builder()
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

    pub fn new_transaction_with_capacity_and_index(
        &self,
        hash: Byte32,
        capacity: Capacity,
        index: u32,
        since: u64,
    ) -> TransactionView {
        let always_success_cell_dep = self.always_success_cell_dep();
        let always_success_script = self.always_success_script();

        core::TransactionBuilder::default()
            .cell_dep(always_success_cell_dep)
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity)
                    .lock(always_success_script)
                    .build(),
            )
            .output_data(packed::Bytes::default())
            .input(CellInput::new(OutPoint::new(hash, index), since))
            .build()
    }

    pub fn new_transaction_with_since_capacity(
        &self,
        hash: Byte32,
        since: u64,
        capacity: Capacity,
    ) -> TransactionView {
        self.new_transaction_with_capacity_and_index(hash, capacity, 0, since)
    }

    pub fn new_always_failure_transaction(&self, hash: Byte32) -> TransactionView {
        let always_failure_cell_dep = self.always_failure_cell_dep();
        let always_failure_script = self.always_failure_script();

        core::TransactionBuilder::default()
            .cell_dep(always_failure_cell_dep)
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(100))
                    .lock(always_failure_script)
                    .build(),
            )
            .output_data(packed::Bytes::default())
            .input(CellInput::new(OutPoint::new(hash, 0), 0))
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

    pub fn assert_pool_entry_status(&self, hash: Byte32, expect_status: &str) {
        let response = self.get_pool_tx_detail_info(hash);
        assert_eq!(response.entry_status, expect_status);
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
        let working_dir = self.working_dir();
        let args = ["-C", &working_dir.to_string_lossy(), "run", "--ba-advanced"];
        let binary = binary();

        {
            let full_command = format!("{} {}", binary.display(), args.join(" "),);

            info!("Start Node: {}", full_command);
        }

        let mut child_process = Command::new(binary)
            .env("RUST_BACKTRACE", "full")
            .args(args)
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
                    error!(
                        "Error: node crashed: {}, log_path: {}",
                        status,
                        self.log_path().display()
                    );
                    info!("Last 500 lines of log:");
                    self.print_last_500_lines_log(&self.log_path());
                    info!("End of last 500 lines of log");
                    // parent process will exit
                    return;
                }
                Err(error) => {
                    error!(
                        "Error: node crashed with reason: {}, log_path: {}",
                        error,
                        self.log_path().display()
                    );
                    info!("Last 500 lines of log:");
                    self.print_last_500_lines_log(&self.log_path());
                    info!("End of last 500 lines of log");
                    return;
                }
            }
        };

        self.wait_find_unverified_blocks_finished();
        self.wait_tx_pool_ready();

        self.set_process_guard(ProcessGuard {
            name: self.inner.spec_node_name.clone(),
            child: child_process,
            killed: false,
        });
        self.set_node_id(node_info.node_id.as_str());
    }

    pub(crate) fn print_last_500_lines_log(&self, log_file: &Path) {
        let file = File::open(log_file).expect("open log file");
        let reader = BufReader::new(file);
        let lines: Vec<String> = reader.lines().map(|line| line.unwrap()).collect();
        let start = if lines.len() > 500 {
            lines.len() - 500
        } else {
            0
        };
        for line in lines.iter().skip(start) {
            info!("{}", line);
        }
    }

    pub(crate) fn set_process_guard(&mut self, guard: ProcessGuard) {
        let mut g = self.inner.guard.write().unwrap();
        *g = Some(guard);
    }

    pub(crate) fn set_node_id(&mut self, node_id: &str) {
        let mut n = self.inner.node_id.write().unwrap();
        *n = Some(node_id.to_owned());
    }

    pub(crate) fn take_guard(&mut self) -> Option<ProcessGuard> {
        let mut g = self.inner.guard.write().unwrap();
        g.take()
    }

    pub(crate) fn is_alive(&mut self) -> bool {
        let mut g = self.inner.guard.write().unwrap();
        if let Some(guard) = g.as_mut() {
            guard.is_alive()
        } else {
            false
        }
    }

    pub fn stop(&mut self) {
        drop(self.take_guard());
    }

    fn derive_options(
        &self,
        mut config: CKBAppConfig,
        root_dir: &Path,
        subcommand_name: &str,
    ) -> Result<CKBAppConfig, ExitCode> {
        config.root_dir = root_dir.to_path_buf();

        config.data_dir = root_dir.join(config.data_dir);

        config.db.adjust(root_dir, &config.data_dir, "db");
        config.ancient = config.data_dir.join("ancient");

        config.network.path = config.data_dir.join("network");
        if config.tmp_dir.is_none() {
            config.tmp_dir = Some(config.data_dir.join("tmp"));
        }
        config.logger.log_dir = config.data_dir.join("logs");
        config.logger.file = Path::new(&(subcommand_name.to_string() + ".log")).to_path_buf();

        let tx_pool_path = config.data_dir.join("tx_pool");
        config.tx_pool.adjust(root_dir, tx_pool_path);

        let indexer_path = config.data_dir.join("indexer");
        config.indexer.adjust(root_dir, indexer_path);

        config.chain.spec.absolutize(root_dir);

        Ok(config)
    }

    pub fn wait_find_unverified_blocks_finished(&self) {
        // wait for node[0] to find unverified blocks finished

        let now = std::time::Instant::now();
        while !self
            .access_log(|line: &str| line.contains("find unverified blocks finished"))
            .expect("node[0] must have log")
        {
            std::thread::sleep(std::time::Duration::from_secs(1));
            if now.elapsed() > Duration::from_secs(60) {
                panic!("node[0] should find unverified blocks finished in 60s");
            }
            info!("waiting for node[0] to find unverified blocks finished");
        }
    }

    pub fn access_log<F>(&self, line_checker: F) -> io::Result<bool>
    where
        F: Fn(&str) -> bool,
    {
        let file = File::open(self.log_path())?;
        let reader = BufReader::new(file);

        for line in reader.lines() {
            let line = line?;
            if line_checker(&line) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn access_db<F>(&self, f: F)
    where
        F: Fn(&ChainDB),
    {
        info!("accessing db");
        info!("AppConfig load_for_subcommand {:?}", self.working_dir());

        let resource = Resource::ckb_config(self.working_dir());
        let app_config =
            CKBAppConfig::load_from_slice(&resource.get().expect("resource")).expect("app config");

        let config = AppConfig::CKB(Box::new(
            self.derive_options(app_config, self.working_dir().as_ref(), "run")
                .expect("app config"),
        ));

        let consensus = config
            .chain_spec()
            .expect("spec")
            .build_consensus()
            .expect("consensus");

        let app_config = config.into_ckb().expect("app config");

        let db = open_or_create_db(
            "ckb",
            &app_config.root_dir,
            &app_config.db,
            consensus.hardfork_switch().clone(),
        )
        .expect("open_or_create_db");
        let chain_db = ChainDB::new(db, app_config.store);
        f(&chain_db);

        info!("accessed db done");
    }

    #[allow(unused_mut)]
    pub fn stop_gracefully(&mut self) {
        let guard = self.take_guard();
        if let Some(mut guard) = guard {
            if !guard.killed {
                // on nix: send SIGINT to the child
                // on windows: don't kill gracefully..... fix later
                #[cfg(not(target_os = "windows"))]
                {
                    Self::kill_gracefully(guard.child.id());
                    let _ = guard.child.wait();
                    guard.killed = true;
                }
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn kill_gracefully(pid: u32) {
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid as i32),
            nix::sys::signal::Signal::SIGINT,
        )
        .expect("cannot send ctrl-c");
    }

    pub fn export(&self, target: String) {
        Command::new(binary())
            .args([
                "export",
                "-C",
                &self.working_dir().to_string_lossy(),
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
            .args([
                "import",
                "-C",
                &self.working_dir().to_string_lossy(),
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
pub fn disconnect_all<N: Borrow<Node>>(nodes: &[N]) {
    for node_a in nodes.iter() {
        for node_b in nodes.iter() {
            if node_a.borrow().p2p_address() != node_b.borrow().p2p_address() {
                node_a.borrow().disconnect(node_b.borrow());
            }
        }
    }
}

// TODO it will be removed out later, in another PR
pub fn waiting_for_sync<N: Borrow<Node>>(nodes: &[N]) {
    let mut tip_headers: HashSet<ckb_jsonrpc_types::HeaderView> =
        HashSet::with_capacity(nodes.len());
    // 60 seconds is a reasonable timeout to sync, even for poor CI server
    let synced = wait_until(120, || {
        tip_headers = nodes
            .as_ref()
            .iter()
            .map(|node| node.borrow().rpc_client().get_tip_header())
            .collect();
        tip_headers.len() == 1
    });
    if !synced {
        panic!(
            "timeout to wait for sync, tip_headers: {:?}",
            tip_headers
                .iter()
                .map(|header| header.inner.number.value())
                .collect::<Vec<BlockNumber>>()
        );
    }
    for node in nodes {
        node.borrow().wait_for_tx_pool();
    }
}

pub fn make_bootnodes_for_all<N: BorrowMut<Node>>(nodes: &mut [N]) {
    let node_multiaddrs: HashMap<String, Multiaddr> = nodes
        .iter()
        .map(|n| {
            (
                n.borrow().node_id(),
                n.borrow().p2p_address().try_into().unwrap(),
            )
        })
        .collect();
    let other_node_addrs: Vec<Vec<Multiaddr>> = node_multiaddrs
        .keys()
        .map(|id| {
            let addrs = node_multiaddrs
                .iter()
                .filter(|(other_id, _)| other_id.as_str() != id.as_str())
                .map(|(_, addr)| addr.to_owned())
                .collect::<Vec<_>>();
            addrs
        })
        .collect();
    for (i, node) in nodes.iter_mut().enumerate() {
        node.borrow_mut()
            .modify_app_config(|config: &mut CKBAppConfig| {
                info!("Setting bootnodes to {:?}", other_node_addrs[i]);
                config.network.bootnodes = other_node_addrs[i].clone();
            })
    }
    // Restart nodes to make bootnodes work
    for node in nodes.iter_mut() {
        node.borrow_mut().stop();
        node.borrow_mut().start();
        info!("Restarted node {:?}", node.borrow_mut().node_id());
    }
}
