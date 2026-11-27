use crate::global::binary;
use crate::rpc::RpcClient;
use crate::utils::{
    TempPathBuf, adaptive_poll_interval, find_available_port, tweaked_duration, wait_until,
};
use crate::{SYSTEM_CELL_ALWAYS_FAILURE_INDEX, SYSTEM_CELL_ALWAYS_SUCCESS_INDEX};
use ckb_app_config::{AppConfig, CKBAppConfig, ExitCode};
use ckb_chain_spec::ChainSpec;
use ckb_chain_spec::consensus::Consensus;
use ckb_error::AnyError;
use ckb_jsonrpc_types::{BlockFilter, BlockTemplate, TxPoolInfo};
use ckb_jsonrpc_types::{PoolTxDetailInfo, TxStatus};
use ckb_logger::{debug, error, info};
use ckb_network::SupportProtocols;
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
use std::io::{self, BufRead, BufReader, ErrorKind, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};
use std::thread::sleep;
use std::time::{Duration, Instant};

pub(crate) type ActiveChildSlot = Arc<Mutex<Option<Child>>>;
type ActiveChildEntry = (String, Weak<Mutex<Option<Child>>>);

struct ActiveChildRegistry {
    shutting_down: bool,
    children: Vec<ActiveChildEntry>,
}

impl ActiveChildRegistry {
    const fn new() -> Self {
        Self {
            shutting_down: false,
            children: Vec::new(),
        }
    }

    fn spawn_child(
        &mut self,
        name: impl Into<String>,
        cmd: &mut Command,
    ) -> io::Result<ActiveChildSlot> {
        if self.shutting_down {
            return Err(io::Error::new(
                ErrorKind::Interrupted,
                "runner is shutting down; refusing to spawn child process",
            ));
        }
        let child = cmd.spawn()?;
        let slot = Arc::new(Mutex::new(Some(child)));
        self.children.retain(|(_, weak)| weak.strong_count() > 0);
        self.children.push((name.into(), Arc::downgrade(&slot)));
        Ok(slot)
    }

    fn begin_shutdown(&mut self) -> Vec<(String, ActiveChildSlot)> {
        self.shutting_down = true;
        let mut live = Vec::new();
        self.children.retain(|(name, weak)| match weak.upgrade() {
            Some(slot) => {
                live.push((name.clone(), slot));
                true
            }
            None => false,
        });
        live
    }
}

fn active_children() -> &'static Mutex<ActiveChildRegistry> {
    static ACTIVE: OnceLock<Mutex<ActiveChildRegistry>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(ActiveChildRegistry::new()))
}

/// Atomically checks shutdown state, spawns `cmd`, and registers the child handle
/// under the same registry lock so watchdog shutdown can never miss a racing spawn.
pub(crate) fn spawn_registered_child(
    name: impl Into<String>,
    cmd: &mut Command,
) -> io::Result<ActiveChildSlot> {
    let mut reg = active_children().lock().unwrap_or_else(|e| e.into_inner());
    reg.spawn_child(name, cmd)
}

/// Force-kills and reaps the child in `slot` while holding the slot lock so
/// concurrent watchdog cleanup waits until the process is actually terminated.
pub(crate) fn kill_and_reap_slot(name: &str, slot: &ActiveChildSlot) {
    let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(mut child) = guard.take() {
        match child.kill() {
            Err(e) if e.kind() != ErrorKind::InvalidInput => {
                error!("Could not kill child process ({name}): {e}");
            }
            _ => debug!("Force-killed child process ({name})"),
        }
        let _ = child.wait();
    }
}

fn wait_child_slot(slot: &ActiveChildSlot) -> io::Result<Option<std::process::ExitStatus>> {
    let mut attempt = 0u32;
    loop {
        {
            let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
            let Some(child) = guard.as_mut() else {
                // Watchdog took ownership to force-kill and reap the child.
                return Ok(None);
            };
            if let Some(status) = child.try_wait()? {
                guard.take();
                return Ok(Some(status));
            }
        }
        sleep(adaptive_poll_interval(attempt, 5, 50));
        attempt = attempt.saturating_add(1);
    }
}

fn stop_gracefully_slot(name: &str, slot: &ActiveChildSlot) {
    #[cfg(not(target_os = "windows"))]
    {
        {
            let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
            let Some(child) = guard.as_mut() else {
                return;
            };
            if matches!(child.try_wait(), Ok(Some(_))) {
                guard.take();
                return;
            }
            // Send SIGINT while holding the slot lock so the child cannot be reaped
            // (and its PID cannot be reused) before the signal is delivered.
            Node::kill_gracefully(child.id());
        }
        if let Err(err) = wait_child_slot(slot) {
            error!("Could not wait for child process ({name}): {err}");
            kill_and_reap_slot(name, slot);
        }
    }
    #[cfg(target_os = "windows")]
    {
        kill_and_reap_slot(name, slot);
    }
}

fn kill_all_in_registry(registry: &Mutex<ActiveChildRegistry>) {
    let slots = {
        let mut reg = registry.lock().unwrap_or_else(|e| e.into_inner());
        reg.begin_shutdown()
    };
    for (name, slot) in slots {
        kill_and_reap_slot(&name, &slot);
    }
}

/// Enters shutdown mode (blocking any new child spawns), then force-kills and
/// reaps all currently registered child processes (`ckb`, `tor`, etc.) before
/// abrupt runner exit on `--spec-timeout`, `--max-time`, or Ctrl-C.
pub(crate) fn kill_all_active_children() {
    kill_all_in_registry(active_children());
}

struct TemplateAssets {
    ckb_toml_raw: Vec<u8>,
    integration_toml_raw: Vec<u8>,
    always_success_cell: Vec<u8>,
    always_failure_cell: Vec<u8>,
}

impl TemplateAssets {
    fn get() -> &'static Self {
        static ASSETS: OnceLock<TemplateAssets> = OnceLock::new();
        ASSETS.get_or_init(|| {
            let template_dir = std::env::current_dir()
                .expect("get current_dir")
                .join("template");
            let ckb_toml_raw =
                fs::read(template_dir.join("ckb.toml")).expect("read template/ckb.toml");
            let integration_toml_path = template_dir.join("specs/integration.toml");
            let integration_toml_raw =
                fs::read(&integration_toml_path).expect("read template/specs/integration.toml");
            let always_success_cell = fs::read(template_dir.join("specs/cells/always_success"))
                .expect("read template/specs/cells/always_success");
            let always_failure_cell = fs::read(template_dir.join("specs/cells/always_failure"))
                .expect("read template/specs/cells/always_failure");

            TemplateAssets {
                ckb_toml_raw,
                integration_toml_raw,
                always_success_cell,
                always_failure_cell,
            }
        })
    }
}

pub(crate) struct ProcessGuard {
    pub name: String,
    pub child: ActiveChildSlot,
}

impl ProcessGuard {
    pub(crate) fn spawn(name: String, cmd: &mut Command) -> io::Result<Self> {
        let child = spawn_registered_child(name.clone(), cmd)?;
        Ok(Self { name, child })
    }

    pub(crate) fn is_alive(&mut self) -> bool {
        let Ok(mut guard) = self.child.lock() else {
            return false;
        };
        match guard.as_mut().map(Child::try_wait) {
            Some(Ok(status_op)) => status_op.is_none(),
            _ => false,
        }
    }

    pub(crate) fn try_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        let mut guard = self.child.lock().expect("lock child process");
        match guard.as_mut() {
            Some(child) => child.try_wait(),
            None => Err(io::Error::new(
                ErrorKind::BrokenPipe,
                "child process already terminated",
            )),
        }
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        kill_and_reap_slot(&self.name, &self.child);
    }
}

#[derive(Clone)]
pub struct Node {
    inner: Arc<InnerNode>,
}

pub struct InnerNode {
    spec_node_name: String,
    working_dir: TempPathBuf,
    consensus: Arc<Consensus>,
    p2p_listen: String,
    rpc_client: RpcClient,
    rpc_listen: String,

    node_id: RwLock<Option<String>>, // initialize when starts node
    guard: RwLock<Option<ProcessGuard>>, // initialize when starts node
}

impl Node {
    pub fn new(spec_name: &str, node_name: &str) -> Self {
        Self::new_with_modifiers(spec_name, node_name, |_| {}, |_| {})
    }

    pub fn new_with_modifiers<MA, MC>(
        spec_name: &str,
        node_name: &str,
        app_modifier: MA,
        chain_modifier: MC,
    ) -> Self
    where
        MA: FnOnce(&mut CKBAppConfig),
        MC: FnOnce(&mut ChainSpec),
    {
        let template = TemplateAssets::get();
        let working_dir = TempPathBuf::new(spec_name, node_name);

        // Copy node template into node's working directory
        let cells_dir = working_dir.join("specs").join("cells");
        debug!("working_dir {}", working_dir.display());

        fs::create_dir_all(&cells_dir).expect("create node's dir");
        fs::write(
            cells_dir.join("always_success"),
            &template.always_success_cell,
        )
        .expect("write always_success cell");
        fs::write(
            cells_dir.join("always_failure"),
            &template.always_failure_cell,
        )
        .expect("write always_failure cell");

        let chain_spec_path = working_dir.join("specs/integration.toml");
        fs::write(&chain_spec_path, &template.integration_toml_raw)
            .expect("write specs/integration.toml");

        let mut app_config = CKBAppConfig::load_from_slice(&template.ckb_toml_raw).unwrap();
        let rpc_port = find_available_port();
        let p2p_port = find_available_port();
        app_config.rpc.listen_address = format!("127.0.0.1:{rpc_port}");
        app_config.network.listen_addresses =
            vec![format!("/ip4/127.0.0.1/tcp/{p2p_port}").parse().unwrap()];
        app_modifier(&mut app_config);
        fs::write(
            working_dir.join("ckb.toml"),
            toml::to_string(&app_config).unwrap(),
        )
        .unwrap();

        let mut chain_spec =
            ChainSpec::load_from(&Resource::file_system(chain_spec_path.clone())).unwrap();
        let before_chain_spec = toml::to_string(&chain_spec).unwrap();
        chain_modifier(&mut chain_spec);
        let modified_chain_spec_toml = toml::to_string(&chain_spec).unwrap();
        if modified_chain_spec_toml != before_chain_spec {
            fs::write(&chain_spec_path, modified_chain_spec_toml).unwrap();
        }
        let consensus = Self::build_consensus(&working_dir, chain_spec);

        let spec_node_name = format!("{}_{}", spec_name, node_name);
        Self::from_parts(working_dir, spec_node_name, &app_config, consensus)
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
        let old_chain_spec = toml::to_string(&app_config.chain.spec).unwrap();
        modifier(&mut app_config);
        fs::write(&app_config_path, toml::to_string(&app_config).unwrap()).unwrap();

        let consensus = if toml::to_string(&app_config.chain.spec).unwrap() == old_chain_spec {
            Arc::clone(&self.inner.consensus)
        } else {
            Self::load_consensus(self.inner.working_dir.path())
        };

        *self = Self::from_parts(
            self.inner.working_dir.clone(),
            self.inner.spec_node_name.clone(),
            &app_config,
            consensus,
        );
    }

    pub fn modify_chain_spec<M>(&mut self, modifier: M)
    where
        M: Fn(&mut ChainSpec),
    {
        let chain_spec_path = self.working_dir().join("specs/integration.toml");
        let chain_spec_res = Resource::file_system(chain_spec_path.clone());
        let mut chain_spec = ChainSpec::load_from(&chain_spec_res).unwrap();
        let before = toml::to_string(&chain_spec).unwrap();
        modifier(&mut chain_spec);
        let after = toml::to_string(&chain_spec).unwrap();
        if after == before {
            return;
        }
        fs::write(&chain_spec_path, after).unwrap();

        let app_config = {
            let app_config_path = self.working_dir().join("ckb.toml");
            let toml = fs::read(app_config_path).unwrap();
            CKBAppConfig::load_from_slice(&toml).unwrap()
        };
        let consensus = Self::build_consensus(self.inner.working_dir.path(), chain_spec);

        *self = Self::from_parts(
            self.inner.working_dir.clone(),
            self.inner.spec_node_name.clone(),
            &app_config,
            consensus,
        );
    }

    fn load_consensus(working_dir: &Path) -> Arc<Consensus> {
        let chain_spec_path = working_dir.join("specs/integration.toml");
        let chain_spec_res = Resource::file_system(chain_spec_path);
        let chain_spec = ChainSpec::load_from(&chain_spec_res).unwrap();
        Self::build_consensus(working_dir, chain_spec)
    }

    fn build_consensus(working_dir: &Path, mut chain_spec: ChainSpec) -> Arc<Consensus> {
        // Ensure the data path is available because chain_spec.build_consensus() needs to access the
        // system-cell data.
        let specs_dir = working_dir.join("specs");
        chain_spec
            .genesis
            .system_cells
            .iter_mut()
            .for_each(|system_cell| {
                system_cell.file.absolutize(&specs_dir);
            });
        Arc::new(chain_spec.build_consensus().unwrap())
    }

    fn from_parts(
        working_dir: TempPathBuf,
        spec_node_name: String,
        app_config: &CKBAppConfig,
        consensus: Arc<Consensus>,
    ) -> Self {
        let p2p_listen = app_config.network.listen_addresses[0].to_string();
        let rpc_listen = format!("http://{}/", app_config.rpc.listen_address);
        let rpc_client = RpcClient::new(&rpc_listen);
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

    pub fn spec_node_name(&self) -> &str {
        &self.inner.spec_node_name
    }

    pub fn rpc_client(&self) -> &RpcClient {
        &self.inner.rpc_client
    }

    pub fn working_dir(&self) -> PathBuf {
        self.inner.working_dir.to_path_buf()
    }

    pub fn owned_working_dir(&self) -> TempPathBuf {
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
        self.rpc_client()
            .local_node_info()
            .addresses
            .iter()
            .filter(|addr| addr.address.contains("/onion3/"))
            .collect::<Vec<_>>()
            .first()
            .map(|addr| addr.address.clone())
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
        let always_failure_raw = self.always_failure_raw_data();
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
        let self_id = self.node_id();
        let peer_id = peer.node_id();
        let is_connected = || {
            self.rpc_client()
                .get_peers()
                .iter()
                .any(|p| p.node_id == peer_id && peer_has_core_protocols(p))
                && peer
                    .rpc_client()
                    .get_peers()
                    .iter()
                    .any(|p| p.node_id == self_id && peer_has_core_protocols(p))
        };
        let connected = wait_until(5, is_connected) || {
            // Re-issue idempotent dial once if an initial handshake dropped under heavy load.
            self.rpc_client()
                .add_node(peer.node_id(), peer.p2p_listen());
            wait_until(15, is_connected)
        };
        if !connected {
            let self_peers: Vec<_> = self
                .rpc_client()
                .get_peers()
                .into_iter()
                .map(|p| {
                    let protos: Vec<_> = p.protocols.into_iter().map(|pr| pr.id.value()).collect();
                    (p.node_id, protos)
                })
                .collect();
            let peer_peers: Vec<_> = peer
                .rpc_client()
                .get_peers()
                .into_iter()
                .map(|p| {
                    let protos: Vec<_> = p.protocols.into_iter().map(|pr| pr.id.value()).collect();
                    (p.node_id, protos)
                })
                .collect();
            panic!(
                "Connect outbound peer timeout: {} ({}) -> {} ({}), self_peers={self_peers:?}, peer_peers={peer_peers:?}",
                self.inner.spec_node_name, self_id, peer.inner.spec_node_name, peer_id
            );
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
        let disconnected = wait_until(15, || {
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
            let self_peers: Vec<_> = self
                .rpc_client()
                .get_peers()
                .into_iter()
                .map(|p| p.node_id)
                .collect();
            let peer_peers: Vec<_> = peer
                .rpc_client()
                .get_peers()
                .into_iter()
                .map(|p| p.node_id)
                .collect();
            panic!(
                "Disconnect timeout: {} -> {} ({}), self_peers={self_peers:?}, peer_peers={peer_peers:?}",
                self.inner.spec_node_name,
                peer.inner.spec_node_name,
                peer.node_id()
            );
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
        self.rpc_client()
            .get_transaction_with_verbosity(tx_hash, 1)
            .tx_status
    }

    /// Collect a best-effort diagnostic summary of this node's live RPC state
    /// (with short timeouts) for post-mortem failure reports.
    pub fn diagnostic_snapshot(&self) -> String {
        use std::fmt::Write;

        let alive = self.is_alive();
        let mut out = String::new();
        let _ = writeln!(out, "Node: {}", self.spec_node_name());
        let _ = writeln!(out, "  alive:       {alive}");
        let _ = writeln!(out, "  rpc:         {}", self.rpc_listen());
        let _ = writeln!(out, "  p2p:         {}", self.p2p_listen());
        let _ = writeln!(out, "  working_dir: {}", self.working_dir().display());
        let _ = writeln!(out, "  log:         {}", self.log_path().display());
        if !alive {
            return out;
        }

        let rpc = self.rpc_client();
        match rpc.diagnostic_call("get_tip_header", serde_json::json!([])) {
            Ok(v) => {
                let number = v["number"].as_str().unwrap_or("?");
                let hash = v["hash"].as_str().unwrap_or("?");
                let epoch = v["epoch"].as_str().unwrap_or("?");
                let ts = v["timestamp"].as_str().unwrap_or("?");
                let _ = writeln!(
                    out,
                    "  tip:         number={number} hash={hash} epoch={epoch} timestamp={ts}"
                );
            }
            Err(err) => {
                let _ = writeln!(out, "  tip:         <error: {err}>");
            }
        }
        match rpc.diagnostic_call("sync_state", serde_json::json!([])) {
            Ok(v) => {
                let _ = writeln!(
                    out,
                    "  sync_state:  ibd={} best_known={} in_flight={} orphan={} unverified_tip={}",
                    v["ibd"],
                    v["best_known_block_number"].as_str().unwrap_or("?"),
                    v["inflight_blocks_count"].as_str().unwrap_or("?"),
                    v["orphan_blocks_count"].as_str().unwrap_or("?"),
                    v["unverified_tip_number"].as_str().unwrap_or("?"),
                );
            }
            Err(err) => {
                let _ = writeln!(out, "  sync_state:  <error: {err}>");
            }
        }
        match rpc.diagnostic_call("tx_pool_info", serde_json::json!([])) {
            Ok(v) => {
                let _ = writeln!(
                    out,
                    "  tx_pool:     pending={} proposed={} orphan={} tip_number={} size={} cycles={}",
                    v["pending"].as_str().unwrap_or("?"),
                    v["proposed"].as_str().unwrap_or("?"),
                    v["orphan"].as_str().unwrap_or("?"),
                    v["tip_number"].as_str().unwrap_or("?"),
                    v["total_tx_size"].as_str().unwrap_or("?"),
                    v["total_tx_cycles"].as_str().unwrap_or("?"),
                );
            }
            Err(err) => {
                let _ = writeln!(out, "  tx_pool:     <error: {err}>");
            }
        }
        match rpc.diagnostic_call("get_peers", serde_json::json!([])) {
            Ok(serde_json::Value::Array(peers)) => {
                let _ = writeln!(out, "  peers ({}):", peers.len());
                for peer in &peers {
                    let id = peer["node_id"].as_str().unwrap_or("?");
                    let dir = if peer["is_outbound"].as_bool().unwrap_or(false) {
                        "outbound"
                    } else {
                        "inbound"
                    };
                    let protos: Vec<String> = peer["protocols"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|p| p["id"].as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    let _ = writeln!(out, "    - {id} ({dir}) protocols=[{}]", protos.join(","));
                }
            }
            Ok(other) => {
                let _ = writeln!(out, "  peers:       {other}");
            }
            Err(err) => {
                let _ = writeln!(out, "  peers:       <error: {err}>");
            }
        }
        match rpc.diagnostic_call("get_banned_addresses", serde_json::json!([])) {
            Ok(serde_json::Value::Array(bans)) => {
                if bans.is_empty() {
                    let _ = writeln!(out, "  banned:      0");
                } else {
                    let _ = writeln!(out, "  banned ({}):", bans.len());
                    for ban in &bans {
                        let _ = writeln!(
                            out,
                            "    - {} reason={}",
                            ban["address"].as_str().unwrap_or("?"),
                            ban["ban_reason"].as_str().unwrap_or("?")
                        );
                    }
                }
            }
            Ok(other) => {
                let _ = writeln!(out, "  banned:      {other}");
            }
            Err(err) => {
                let _ = writeln!(out, "  banned:      <error: {err}>");
            }
        }
        out
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
        let timeout = tweaked_duration(20);
        let mut deadline = Instant::now() + timeout;
        let mut attempt = 0u32;
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            sleep(adaptive_poll_interval(attempt, 2, 50).min(remaining));
            attempt = attempt.saturating_add(1);
            chain_tip = rpc_client.get_tip_header();
            let prev_tx_pool_tip = tx_pool_tip;
            tx_pool_tip = rpc_client.tx_pool_info();
            if chain_tip.hash == tx_pool_tip.tip_hash {
                return;
            } else if prev_tx_pool_tip.tip_hash != tx_pool_tip.tip_hash {
                deadline = Instant::now() + timeout;
                attempt = 0;
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
        let deadline = Instant::now() + tweaked_duration(180);
        let mut attempt = 0u32;
        while !rpc_client.tx_pool_ready() {
            let now = Instant::now();
            if now >= deadline {
                panic!(
                    "timeout to wait tx_pool_ready for node {}",
                    self.inner.spec_node_name
                );
            }
            sleep(adaptive_poll_interval(attempt, 10, 50).min(deadline - now));
            attempt = attempt.saturating_add(1);
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

    pub fn wait_block_template_with_blocking<B>(&self, blocking: B) -> BlockTemplate
    where
        B: Fn(&BlockTemplate) -> bool,
    {
        let deadline = Instant::now() + tweaked_duration(30);
        let mut attempt = 0u32;
        let mut template = self.rpc_client().get_block_template(None, None, None);
        while blocking(&template) {
            let now = Instant::now();
            if now >= deadline {
                panic!(
                    "wait_block_template_with_blocking timeout on {} (tip={}, template_number={}, proposals={}, txs={}, uncles={})",
                    self.inner.spec_node_name,
                    self.get_tip_block_number(),
                    template.number.value(),
                    template.proposals.len(),
                    template.transactions.len(),
                    template.uncles.len()
                );
            }
            sleep(adaptive_poll_interval(attempt, 1, 50).min(deadline - now));
            template = self.rpc_client().get_block_template(None, None, None);
            attempt = attempt.saturating_add(1);
        }
        template
    }

    pub fn new_block_builder_with_blocking<B>(&self, blocking: B) -> BlockBuilder
    where
        B: Fn(&BlockTemplate) -> bool,
    {
        let template = self.wait_block_template_with_blocking(blocking);
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

            debug!("Start Node: {}", full_command);
        }

        let log_offset = fs::metadata(self.log_path()).map(|m| m.len()).unwrap_or(0);

        let mut cmd = Command::new(binary);
        cmd.env("RUST_BACKTRACE", "full")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        // Spawn and register atomically under the child-process registry so
        // startup panics or hard runner timeouts can never leave an orphan.
        let mut guard = ProcessGuard::spawn(self.inner.spec_node_name.clone(), &mut cmd)
            .expect("failed to run binary");

        // Wait to ensure the node threads up
        let startup_timeout = tweaked_duration(180);
        let deadline = Instant::now() + startup_timeout;
        let mut attempt = 0u32;
        let node_info = loop {
            match self
                .rpc_client()
                .inner()
                .local_node_info()
                .and_then(|local_node_info| {
                    self.rpc_client()
                        .inner()
                        .tx_pool_info()
                        .map(|_| local_node_info)
                }) {
                Ok(local_node_info) => {
                    break local_node_info;
                }
                Err(last_err) => match guard.try_wait() {
                    Ok(None) => {
                        let now = Instant::now();
                        if now >= deadline {
                            drop(guard);
                            error!(
                                "Error: node {} timed out during startup after {}s (last rpc error: {}), log_path: {}",
                                self.inner.spec_node_name,
                                startup_timeout.as_secs(),
                                last_err,
                                self.log_path().display()
                            );
                            info!("Last 500 lines of log:");
                            self.print_last_500_lines_log(&self.log_path());
                            info!("End of last 500 lines of log");
                            panic!(
                                "Node {} timed out during startup after {}s, last rpc error: {}",
                                self.inner.spec_node_name,
                                startup_timeout.as_secs(),
                                last_err
                            );
                        }
                        sleep(adaptive_poll_interval(attempt, 10, 50).min(deadline - now));
                        attempt = attempt.saturating_add(1);
                    }
                    Ok(Some(status)) => {
                        drop(guard);
                        error!(
                            "Error: node crashed: {}, log_path: {}",
                            status,
                            self.log_path().display()
                        );
                        info!("Last 500 lines of log:");
                        self.print_last_500_lines_log(&self.log_path());
                        info!("End of last 500 lines of log");
                        panic!(
                            "Node {} crashed during startup with status {}",
                            self.inner.spec_node_name, status
                        );
                    }
                    Err(error) => {
                        drop(guard);
                        error!(
                            "Error: node crashed with reason: {}, log_path: {}",
                            error,
                            self.log_path().display()
                        );
                        info!("Last 500 lines of log:");
                        self.print_last_500_lines_log(&self.log_path());
                        info!("End of last 500 lines of log");
                        panic!(
                            "Node {} crashed during startup with error {}",
                            self.inner.spec_node_name, error
                        );
                    }
                },
            }
        };

        self.set_process_guard(guard);
        self.set_node_id(node_info.node_id.as_str());
        self.wait_find_unverified_blocks_finished_from(log_offset);
        self.wait_tx_pool_ready();
    }

    pub(crate) fn print_last_500_lines_log(&self, log_file: &Path) {
        let Ok(file) = File::open(log_file) else {
            info!("Log file {} does not exist yet", log_file.display());
            return;
        };
        let reader = BufReader::new(file);
        let lines: Vec<String> = reader.lines().map_while(Result::ok).collect();
        let start = lines.len().saturating_sub(500);
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

    pub(crate) fn is_alive(&self) -> bool {
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
        self.wait_find_unverified_blocks_finished_from(0);
    }

    fn wait_find_unverified_blocks_finished_from(&self, mut seek_from: u64) {
        // wait for node to find unverified blocks finished
        let timeout = Duration::from_secs(
            std::env::var("CKB_TEST_BLOCK_TIMEOUT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(180),
        );
        let deadline = Instant::now() + timeout;
        let log_path = self.log_path();
        let mut file: Option<File> = None;
        let mut line_buf = String::new();
        let mut attempt = 0u32;
        loop {
            if file.is_none() {
                match File::open(&log_path) {
                    Ok(opened) => file = Some(opened),
                    Err(err) if err.kind() == ErrorKind::NotFound => {}
                    Err(err) => panic!("node must have log ({}): {err}", log_path.display()),
                }
            }

            if let Some(ref mut opened) = file {
                let file_size = fs::metadata(&log_path)
                    .map(|m| m.len())
                    .unwrap_or(seek_from);
                if file_size > seek_from {
                    opened.seek(SeekFrom::Start(seek_from)).unwrap();
                    let mut reader = BufReader::new(&*opened);
                    loop {
                        line_buf.clear();
                        let bytes_read = reader.read_line(&mut line_buf).unwrap();
                        if bytes_read == 0 || !line_buf.ends_with('\n') {
                            break;
                        }
                        seek_from += bytes_read as u64;
                        if line_buf.contains("find unverified blocks finished") {
                            return;
                        }
                    }
                }
            }

            let now = Instant::now();
            if now >= deadline {
                panic!(
                    "node {} should find unverified blocks finished in {}s",
                    self.inner.spec_node_name,
                    timeout.as_secs()
                );
            }
            sleep(adaptive_poll_interval(attempt, 10, 50).min(deadline - now));
            attempt = attempt.saturating_add(1);
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
        debug!("accessing db");
        debug!("AppConfig load_for_subcommand {:?}", self.working_dir());

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

        debug!("accessed db done");
    }

    pub fn stop_gracefully(&mut self) {
        if let Some(guard) = self.take_guard() {
            stop_gracefully_slot(&guard.name, &guard.child);
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn kill_gracefully(pid: u32) {
        match nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid as i32),
            nix::sys::signal::Signal::SIGINT,
        ) {
            Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
            Err(err) => panic!("cannot send ctrl-c to pid {pid}: {err}"),
        }
    }

    fn run_registered_command(name: &str, cmd: &mut Command) {
        let guard = ProcessGuard::spawn(name.to_string(), cmd).expect("failed to execute process");
        let _ = wait_child_slot(&guard.child).expect("failed to wait for process");
    }

    pub fn export(&self, target: String) {
        let mut cmd = Command::new(binary());
        cmd.args([
            "export",
            "-C",
            &self.working_dir().to_string_lossy(),
            &target,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
        Self::run_registered_command(&format!("{}-export", self.inner.spec_node_name), &mut cmd);
    }

    pub fn import(&self, target: String) {
        let mut cmd = Command::new(binary());
        cmd.args([
            "import",
            "-C",
            &self.working_dir().to_string_lossy(),
            &target,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
        Self::run_registered_command(&format!("{}-import", self.inner.spec_node_name), &mut cmd);
    }
}

fn peer_has_core_protocols(remote_node: &ckb_jsonrpc_types::RemoteNode) -> bool {
    let sync_id = SupportProtocols::Sync.protocol_id().value() as u64;
    let relay_id = SupportProtocols::RelayV3.protocol_id().value() as u64;
    let discovery_id = SupportProtocols::Discovery.protocol_id().value() as u64;
    let has = |target| remote_node.protocols.iter().any(|p| p.id.value() == target);
    has(sync_id) && has(relay_id) && has(discovery_id)
}

// TODO it will be removed out later, in another PR
pub fn connect_all(nodes: &[Node]) {
    if nodes.len() <= 1 {
        return;
    }
    let node_ids: Vec<String> = nodes.iter().map(Node::node_id).collect();
    for i in 0..nodes.len() {
        for j in (i + 1)..nodes.len() {
            if nodes[i].p2p_address() != nodes[j].p2p_address() {
                nodes[i].connect_uncheck(&nodes[j]);
            }
        }
    }
    let all_pairs_connected = || {
        nodes.iter().enumerate().all(|(i, node)| {
            let peers = node.rpc_client().get_peers();
            node_ids
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i && nodes[i].p2p_address() != nodes[*j].p2p_address())
                .all(|(_, peer_id)| {
                    peers
                        .iter()
                        .any(|p| p.node_id == *peer_id && peer_has_core_protocols(p))
                })
        })
    };
    let connected = wait_until(5, all_pairs_connected) || {
        // Re-dial only pairs that did not complete core protocol negotiation in the initial burst.
        for i in 0..nodes.len() {
            let peers = nodes[i].rpc_client().get_peers();
            for j in (i + 1)..nodes.len() {
                if nodes[i].p2p_address() != nodes[j].p2p_address()
                    && !peers
                        .iter()
                        .any(|p| p.node_id == node_ids[j] && peer_has_core_protocols(p))
                {
                    nodes[i].connect_uncheck(&nodes[j]);
                }
            }
        }
        wait_until(15, all_pairs_connected)
    };
    if !connected {
        // Retry any remaining pairs individually to produce a precise failure message.
        for i in 0..nodes.len() {
            for j in 0..nodes.len() {
                if nodes[i].p2p_address() != nodes[j].p2p_address() {
                    nodes[i].connect(&nodes[j]);
                }
            }
        }
    }
}

pub fn wait_relay_protocols_connected<N: Borrow<Node>>(nodes: &[N]) {
    if nodes.len() <= 1 {
        return;
    }
    let relay_protocol_id = SupportProtocols::RelayV3.protocol_id().value() as u64;
    let node_ids: Vec<String> = nodes.iter().map(|n| n.borrow().node_id()).collect();
    let connected = wait_until(20, || {
        nodes.iter().enumerate().all(|(i, node)| {
            let peers = node.borrow().rpc_client().get_peers();
            node_ids
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .all(|(_, peer_id)| {
                    peers.iter().any(|remote_node| {
                        remote_node.node_id == *peer_id
                            && remote_node
                                .protocols
                                .iter()
                                .any(|protocol| protocol.id.value() == relay_protocol_id)
                    })
                })
        })
    });
    if !connected {
        for (i, node) in nodes.iter().enumerate() {
            let node = node.borrow();
            let peers = node
                .rpc_client()
                .get_peers()
                .into_iter()
                .map(|remote_node| {
                    let protocol_ids = remote_node
                        .protocols
                        .into_iter()
                        .map(|protocol| protocol.id.value())
                        .collect::<Vec<_>>();
                    (remote_node.node_id, protocol_ids)
                })
                .collect::<Vec<_>>();
            error!("node[{}] ({}) peers: {:?}", i, node.node_id(), peers);
        }
        panic!("RelayV3 protocol should be active between all connected nodes");
    }
}

// TODO it will be removed out later, in another PR
pub fn disconnect_all<N: Borrow<Node>>(nodes: &[N]) {
    if nodes.len() <= 1 {
        return;
    }
    let node_ids: Vec<String> = nodes.iter().map(|n| n.borrow().node_id()).collect();
    for (i, node_a) in nodes.iter().enumerate() {
        let node_a = node_a.borrow();
        for (j, node_b) in nodes.iter().enumerate() {
            if i != j && node_a.p2p_address() != node_b.borrow().p2p_address() {
                node_a.rpc_client().remove_node(node_ids[j].clone());
            }
        }
    }
    let disconnected = wait_until(20, || {
        nodes.iter().enumerate().all(|(i, node)| {
            let peers = node.borrow().rpc_client().get_peers();
            node_ids
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .all(|(_, peer_id)| peers.iter().all(|p| p.node_id != *peer_id))
        })
    });
    if !disconnected {
        panic!("Disconnect all nodes timeout");
    }
}

// TODO it will be removed out later, in another PR
pub fn waiting_for_sync<N: Borrow<Node>>(nodes: &[N]) {
    waiting_for_sync_with_timeout(nodes, 120);
}

// TODO it will be removed out later, in another PR
pub fn waiting_for_sync_with_timeout<N: Borrow<Node>>(nodes: &[N], timeout_secs: u64) {
    let mut tip_headers: HashSet<ckb_jsonrpc_types::HeaderView> =
        HashSet::with_capacity(nodes.len());
    let synced = wait_until(timeout_secs, || {
        tip_headers = nodes
            .as_ref()
            .iter()
            .map(|node| node.borrow().rpc_client().get_tip_header())
            .collect();
        tip_headers.len() == 1
    });
    if !synced {
        let states: Vec<_> = nodes
            .iter()
            .map(|n| {
                let node = n.borrow();
                let header = node.rpc_client().get_tip_header();
                let sync_state = node.rpc_client().sync_state();
                let peers_len = node.rpc_client().get_peers().len();
                format!(
                    "{}(tip=#{}:{:#x}, best_known=#{}, peers={})",
                    node.inner.spec_node_name,
                    header.inner.number.value(),
                    header.hash,
                    sync_state.best_known_block_number.value(),
                    peers_len,
                )
            })
            .collect();
        panic!(
            "timeout to wait for sync after {}s, node states: [{}]",
            timeout_secs,
            states.join(", ")
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
            node_multiaddrs
                .iter()
                .filter(|(other_id, _)| other_id.as_str() != id.as_str())
                .map(|(_, addr)| addr.to_owned())
                .collect::<Vec<_>>()
        })
        .collect();
    for (i, node) in nodes.iter_mut().enumerate() {
        node.borrow_mut()
            .modify_app_config(|config: &mut CKBAppConfig| {
                debug!("Setting bootnodes to {:?}", other_node_addrs[i]);
                config.network.bootnodes = other_node_addrs[i].clone();
            })
    }
    // Restart nodes in parallel to make bootnodes work
    let mut node_refs: Vec<&mut Node> = nodes.iter_mut().map(BorrowMut::borrow_mut).collect();
    for node in node_refs.iter_mut() {
        node.stop();
    }
    crate::utils::run_in_spec_scope(node_refs.iter_mut(), |node| {
        node.start();
        debug!("Restarted node {:?}", node.node_id());
    });
}

#[cfg(all(test, not(target_os = "windows")))]
mod tests {
    use super::*;

    #[test]
    fn registry_shutdown_kills_reaps_and_blocks_subsequent_spawns() {
        let registry = Mutex::new(ActiveChildRegistry::new());
        let mut cmd = Command::new("sleep");
        cmd.arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let slot = registry
            .lock()
            .unwrap()
            .spawn_child("test_sleep_child", &mut cmd)
            .expect("spawn sleep child");
        let pid = slot.lock().unwrap().as_ref().unwrap().id();

        kill_all_in_registry(&registry);

        assert!(
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None).is_err(),
            "killed child pid {pid} should already be terminated and reaped"
        );

        // Once shutdown has begun, any racing worker attempt to spawn a child must fail
        // immediately without creating an untracked process.
        let mut late_cmd = Command::new("sleep");
        late_cmd
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let err = registry
            .lock()
            .unwrap()
            .spawn_child("late_child", &mut late_cmd)
            .expect_err("registry must reject spawns during shutdown");
        assert_eq!(err.kind(), ErrorKind::Interrupted);
    }

    #[test]
    fn watchdog_takes_over_child_during_graceful_stop() {
        let registry = Mutex::new(ActiveChildRegistry::new());
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "trap '' INT; echo ready; exec sleep 60"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let slot = registry
            .lock()
            .unwrap()
            .spawn_child("test_ignore_sigint", &mut cmd)
            .expect("spawn trap child");

        // Wait deterministically until the shell has installed `trap '' INT`.
        let (pid, mut stdout) = {
            let mut guard = slot.lock().unwrap();
            let child = guard.as_mut().unwrap();
            (child.id(), child.stdout.take().unwrap())
        };
        let mut line = String::new();
        BufReader::new(&mut stdout).read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "ready");

        // Start graceful stop on another thread; because SIGINT is trapped, it will
        // stay in the short-lock `try_wait()` polling loop while keeping the handle shared.
        let stop_slot = Arc::clone(&slot);
        let stop_thread = std::thread::spawn(move || {
            stop_gracefully_slot("test_ignore_sigint", &stop_slot);
        });

        sleep(Duration::from_millis(50));
        assert!(
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None).is_ok(),
            "SIGINT-ignoring child should still be alive during graceful stop"
        );

        // Watchdog shutdown must be able to take ownership, SIGKILL, and reap it immediately.
        kill_all_in_registry(&registry);
        assert!(
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None).is_err(),
            "watchdog should have force-killed and reaped pid {pid} during graceful stop"
        );

        stop_thread
            .join()
            .expect("graceful stop thread should exit cleanly once watchdog reaps child");
    }
}
