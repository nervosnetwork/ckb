use crate::rpc::RpcClient;
use crate::utils::{node_log, wait_until};
use crate::{DEFAULT_TX_PROPOSAL_WINDOW, SYSTEM_CELL_ALWAYS_SUCCESS_INDEX};
use ckb_app_config::{BlockAssemblerConfig, CKBAppConfig};
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
use failure::Error;
use failure::_core::time::Duration;
use std::convert::Into;
use std::fs;
use std::path::PathBuf;
use std::process::{self, Child, Command, Stdio};
use std::time::Instant;

pub struct Node {
    binary: String,
    working_dir: PathBuf,
    p2p_port: u16,
    rpc_port: u16,
    rpc_client: RpcClient,
    node_id: Option<String>,
    genesis_cellbase_hash: Byte32,
    dep_group_tx_hash: Byte32,
    always_success_code_hash: Byte32,
    guard: Option<ProcessGuard>,
    consensus: Option<Consensus>,
    spec: Option<ChainSpec>,
}

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

impl Node {
    pub fn new(
        binary: &str,
        p2p_port: u16,
        rpc_port: u16,
        case_working_dir: PathBuf,
        node_index: &str,
    ) -> Self {
        let rpc_client = RpcClient::new(&format!("http://127.0.0.1:{}/", rpc_port));
        let working_dir = case_working_dir.join(node_index);
        Self {
            binary: binary.to_string(),
            working_dir,
            p2p_port,
            rpc_port,
            rpc_client,
            node_id: None,
            guard: None,
            genesis_cellbase_hash: Default::default(),
            dep_group_tx_hash: Default::default(),
            always_success_code_hash: Default::default(),
            consensus: None,
            spec: None,
        }
    }

    pub fn node_id(&self) -> &str {
        self.node_id.as_ref().expect("uninitialized node_id")
    }

    pub fn consensus(&self) -> &Consensus {
        self.consensus.as_ref().expect("uninitialized consensus")
    }

    pub fn spec(&self) -> &ChainSpec {
        self.spec.as_ref().expect("uninitialized spec")
    }

    pub fn p2p_port(&self) -> u16 {
        self.p2p_port
    }

    pub fn working_dir(&self) -> &PathBuf {
        &self.working_dir
    }

    pub fn log_size(&self) -> u64 {
        let filename = node_log(self.working_dir().to_str().unwrap());
        fs::metadata(filename).unwrap().len()
    }

    pub fn dep_group_tx_hash(&self) -> Byte32 {
        self.dep_group_tx_hash.clone()
    }

    pub fn export(&self, target: String) {
        Command::new(self.binary.to_owned())
            .args(&[
                "export",
                "-C",
                self.working_dir().to_str().unwrap(),
                &target,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .output()
            .expect("failed to execute process");
    }

    pub fn import(&self, target: String) {
        Command::new(self.binary.to_owned())
            .args(&[
                "import",
                "-C",
                self.working_dir().to_str().unwrap(),
                &target,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .output()
            .expect("failed to execute process");
    }

    pub fn start(&mut self) {
        let child_process = Command::new(self.binary.to_owned())
            .env("RUST_BACKTRACE", "full")
            .args(&[
                "-C",
                self.working_dir().to_str().unwrap(),
                "run",
                "--ba-advanced",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("failed to run binary");
        self.guard = Some(ProcessGuard(child_process));

        loop {
            let result = { self.rpc_client().inner().local_node_info() };
            if let Ok(local_node_info) = result {
                self.node_id = Some(local_node_info.node_id);
                let _ = self.rpc_client().tx_pool_info();
                break;
            } else if let Some(ref mut child) = self.guard {
                match child.0.try_wait() {
                    Ok(Some(exit)) => {
                        log::error!("Error: node crashed, {}", exit);
                        process::exit(exit.code().unwrap());
                    }
                    Ok(None) => {
                        std::thread::sleep(std::time::Duration::from_secs(1));
                    }
                    Err(error) => {
                        log::error!("Error: node crashed with reason: {}", error);
                        process::exit(255);
                    }
                }
            }
        }
    }

    pub fn stop(&mut self) {
        drop(self.guard.take())
    }

    pub fn connect(&self, outbound_peer: &Node) {
        let node_info = outbound_peer.rpc_client().local_node_info();

        let node_id = node_info.node_id;
        let rpc_client = self.rpc_client();
        rpc_client.add_node(
            node_id.clone(),
            format!("/ip4/127.0.0.1/tcp/{}", outbound_peer.p2p_port),
        );

        let result = wait_until(5, || {
            let peers = rpc_client.get_peers();
            peers.iter().any(|peer| peer.node_id == node_id)
        });

        if !result {
            panic!("Connect outbound peer timeout, node id: {}", node_id);
        }
    }

    pub fn connect_uncheck(&self, outbound_peer: &Node) {
        let node_info = outbound_peer.rpc_client().local_node_info();

        let node_id = node_info.node_id;
        let rpc_client = self.rpc_client();
        rpc_client.add_node(
            node_id,
            format!("/ip4/127.0.0.1/tcp/{}", outbound_peer.p2p_port),
        );
    }

    // workaround for banned address checking (because we are using loopback address)
    // 1. checking banned addresses is empty
    // 2. connecting outbound peer and checking banned addresses is not empty
    // 3. clear banned addresses
    pub fn connect_and_wait_ban(&self, outbound_peer: &Node) {
        let node_info = outbound_peer.rpc_client().local_node_info();
        let node_id = node_info.node_id;
        let rpc_client = self.rpc_client();

        assert!(
            rpc_client.get_banned_addresses().is_empty(),
            "banned addresses should be empty"
        );
        rpc_client.add_node(
            node_id.clone(),
            format!("/ip4/127.0.0.1/tcp/{}", outbound_peer.p2p_port),
        );

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
                node_id
            );
        }
    }

    pub fn disconnect(&self, node: &Node) {
        let node_info = node.rpc_client().local_node_info();

        let node_id = node_info.node_id;
        let rpc_client = self.rpc_client();
        rpc_client.remove_node(node_id.clone());

        let result = wait_until(5, || {
            let peers = rpc_client.get_peers();
            peers.iter().all(|peer| peer.node_id != node_id)
        });

        if !result {
            panic!("Disconnect timeout, node {}", node_id);
        }

        let rpc_client = node.rpc_client();
        let node_id = self.node_id();
        let result = wait_until(5, || {
            let peers = rpc_client.get_peers();
            peers.iter().all(|peer| peer.node_id != node_id)
        });
        if !result {
            panic!("Disconnect timeout, node {}", node_id);
        }
    }

    pub fn waiting_for_sync(&self, node: &Node, target: BlockNumber) {
        let self_rpc_client = self.rpc_client();
        let node_rpc_client = node.rpc_client();
        let (mut self_tip_number, mut node_tip_number) = (0, 0);
        // 60 seconds is a reasonable timeout to sync, even for poor CI server
        let result = wait_until(60, || {
            self_tip_number = self_rpc_client.get_tip_block_number();
            node_tip_number = node_rpc_client.get_tip_block_number();
            self_tip_number == node_tip_number && target == self_tip_number
        });

        if !result {
            panic!(
                "Waiting for sync timeout, self_tip_number: {}, node_tip_number: {}",
                self_tip_number, node_tip_number
            );
        }
    }

    pub fn rpc_client(&self) -> &RpcClient {
        &self.rpc_client
    }

    pub fn submit_block(&self, block: &BlockView) -> Byte32 {
        self.rpc_client()
            .submit_block("".to_owned(), block.data().into())
            .expect("submit_block failed")
    }

    pub fn process_block_without_verify(&self, block: &BlockView, broadcast: bool) -> Byte32 {
        self.rpc_client()
            .process_block_without_verify(block.data().into(), broadcast)
            .expect("process_block_without_verify result none")
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

    pub fn always_success_script(&self) -> Script {
        Script::new_builder()
            .code_hash(self.always_success_code_hash.clone())
            .hash_type(ScriptHashType::Data.into())
            .build()
    }

    pub fn always_success_cell_dep(&self) -> CellDep {
        CellDep::new_builder()
            .out_point(OutPoint::new(
                self.genesis_cellbase_hash.clone(),
                SYSTEM_CELL_ALWAYS_SUCCESS_INDEX,
            ))
            .build()
    }

    fn prepare_chain_spec(
        &mut self,
        modify_chain_spec: Box<dyn Fn(&mut ChainSpec)>,
    ) -> Result<(), Error> {
        let integration_spec = include_bytes!("../integration.toml");
        let always_success_cell = include_bytes!("../../script/testdata/always_success");
        let always_success_path = self.working_dir().join("specs/cells/always_success");
        fs::create_dir_all(format!("{}/specs", self.working_dir().display()))?;
        fs::create_dir_all(format!("{}/specs/cells", self.working_dir().display()))?;
        fs::write(&always_success_path, &always_success_cell[..])?;

        let mut spec: ChainSpec =
            toml::from_slice(&integration_spec[..]).expect("chain spec config");
        for r in spec.genesis.system_cells.iter_mut() {
            r.file.absolutize(self.working_dir().join("specs"));
        }
        modify_chain_spec(&mut spec);

        let consensus = spec.build_consensus().expect("build consensus");
        self.genesis_cellbase_hash
            .clone_from(&consensus.genesis_block().transactions()[0].hash());
        self.dep_group_tx_hash
            .clone_from(&consensus.genesis_block().transactions()[1].hash());
        self.always_success_code_hash = CellOutput::calc_data_hash(
            &consensus.genesis_block().transactions()[0]
                .outputs_data()
                .get(SYSTEM_CELL_ALWAYS_SUCCESS_INDEX as usize)
                .unwrap()
                .raw_data(),
        );

        self.consensus = Some(consensus);
        self.spec = Some(spec.clone());

        // write to dir
        fs::write(
            self.working_dir().join("specs/integration.toml"),
            toml::to_string(&spec).expect("chain spec serialize"),
        )
        .map_err(Into::into)
    }

    fn rewrite_spec(&self, modify_ckb_config: Box<dyn Fn(&mut CKBAppConfig)>) -> Result<(), Error> {
        // rewrite ckb.toml
        let ckb_config_path = format!("{}/ckb.toml", self.working_dir().display());
        let mut ckb_config: CKBAppConfig =
            toml::from_slice(&fs::read(&ckb_config_path)?).expect("ckb config");
        ckb_config.block_assembler = Some(BlockAssemblerConfig {
            code_hash: self.always_success_code_hash.unpack(),
            args: Default::default(),
            hash_type: ScriptHashType::Data.into(),
            message: Default::default(),
        });

        modify_ckb_config(&mut ckb_config);
        fs::write(
            &ckb_config_path,
            toml::to_string(&ckb_config).expect("ckb config serialize"),
        )
        .map_err(Into::into)
    }

    pub fn edit_config_file(
        &mut self,
        modify_chain_spec: Box<dyn Fn(&mut ChainSpec)>,
        modify_ckb_config: Box<dyn Fn(&mut CKBAppConfig)>,
    ) {
        let rpc_port = format!("{}", self.rpc_port);
        let p2p_port = format!("{}", self.p2p_port);

        let init_output = Command::new(self.binary.to_owned())
            .args(&[
                "-C",
                self.working_dir().to_str().unwrap(),
                "init",
                "--chain",
                "integration",
                "--rpc-port",
                &rpc_port,
                "--p2p-port",
                &p2p_port,
                "--force",
            ])
            .output()
            .unwrap_or_else(|e| {
                panic!(
                    "init working_dir {} command fail: {}",
                    self.working_dir().display(),
                    e
                );
            });

        if !init_output.status.success() {
            panic!(
                "init working_dir {} output not success: {}",
                self.working_dir().display(),
                String::from_utf8_lossy(init_output.stderr.as_slice())
            );
        }

        self.prepare_chain_spec(modify_chain_spec)
            .unwrap_or_else(|e| {
                panic!(
                    "prepare chain spec working_dir {} fail: {}",
                    self.working_dir().display(),
                    e,
                );
            });
        self.rewrite_spec(modify_ckb_config).unwrap_or_else(|e| {
            panic!(
                "write chain spec working_dir {} fail: {}",
                self.working_dir().display(),
                e,
            );
        });
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
}
