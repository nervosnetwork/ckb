use crate::rpc::RpcClient;
use crate::utils::wait_until;
use ckb_app_config::{BlockAssemblerConfig, CKBAppConfig};
use ckb_chain_spec::consensus::Consensus;
use ckb_chain_spec::ChainSpec;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::script::Script;
use ckb_core::transaction::{CellInput, CellOutput, OutPoint, Transaction, TransactionBuilder};
use ckb_core::{capacity_bytes, BlockNumber, Bytes, Capacity};
use ckb_jsonrpc_types::JsonBytes;
use log::info;
use numext_fixed_hash::H256;
use std::convert::Into;
use std::fs;
use std::io::Error;
use std::path::Path;
use std::process::{self, Child, Command, Stdio};

pub struct Node {
    binary: String,
    dir: String,
    p2p_port: u16,
    rpc_port: u16,
    rpc_client: RpcClient,
    node_id: Option<String>,
    genesis_cellbase_hash: H256,
    always_success_code_hash: H256,
    guard: Option<ProcessGuard>,
    pub consensus: Option<Consensus>,
}

struct ProcessGuard(pub Child);

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        match self.0.kill() {
            Err(e) => info!("Could not kill ckb process: {}", e),
            Ok(_) => info!("Successfully killed ckb process"),
        }
        let _ = self.0.wait();
    }
}

impl Node {
    pub fn new(binary: &str, dir: &str, p2p_port: u16, rpc_port: u16) -> Self {
        let rpc_client = RpcClient::new(&format!("http://127.0.0.1:{}/", rpc_port));
        Self {
            binary: binary.to_string(),
            dir: dir.to_string(),
            p2p_port,
            rpc_port,
            rpc_client,
            node_id: None,
            guard: None,
            genesis_cellbase_hash: Default::default(),
            always_success_code_hash: Default::default(),
            consensus: None,
        }
    }

    pub fn node_id(&self) -> &Option<String> {
        &self.node_id
    }

    pub fn p2p_port(&self) -> u16 {
        self.p2p_port
    }

    pub fn start(
        &mut self,
        modify_chain_spec: Box<dyn Fn(&mut ChainSpec) -> ()>,
        modify_ckb_config: Box<dyn Fn(&mut CKBAppConfig) -> ()>,
    ) {
        self.init_config_file(modify_chain_spec, modify_ckb_config)
            .expect("failed to init config file");

        let child_process = Command::new(self.binary.to_owned())
            .env("RUST_BACKTRACE", "full")
            .args(&["-C", &self.dir, "run"])
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("failed to run binary");
        self.guard = Some(ProcessGuard(child_process));
        info!("Started node with working dir: {}", self.dir);

        loop {
            let result = { self.rpc_client().inner().lock().local_node_info().call() };
            if let Ok(local_node_info) = result {
                self.node_id = Some(local_node_info.node_id);
                let _ = self.rpc_client().tx_pool_info();
                break;
            } else if let Some(ref mut child) = self.guard {
                match child.0.try_wait() {
                    Ok(Some(exit)) => {
                        eprintln!("Error: node crashed, {}", exit);
                        process::exit(exit.code().unwrap());
                    }
                    Ok(None) => {
                        std::thread::sleep(std::time::Duration::from_secs(1));
                    }
                    Err(error) => {
                        eprintln!("Error: node crashed with reason: {}", error);
                        process::exit(255);
                    }
                }
            }
        }
    }

    pub fn connect(&self, node: &Node) {
        let node_info = node.rpc_client().local_node_info();

        let node_id = node_info.node_id;
        let rpc_client = self.rpc_client();
        rpc_client.add_node(
            node_id.clone(),
            format!("/ip4/127.0.0.1/tcp/{}", node.p2p_port),
        );

        let result = wait_until(5, || {
            let peers = rpc_client.get_peers();
            peers.iter().any(|peer| peer.node_id == node_id)
        });

        if !result {
            panic!("Connect timeout, node {}", node_id);
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

    pub fn submit_block(&self, block: &Block) -> H256 {
        self.rpc_client()
            .submit_block("".to_owned(), block.into())
            .expect("submit_block result none")
    }

    pub fn process_block_without_verify(&self, block: &Block) -> H256 {
        self.rpc_client()
            .process_block_without_verify(block.into())
            .expect("process_block_without_verify result none")
    }

    pub fn generate_blocks(&self, blocks_num: usize) -> Vec<H256> {
        (0..blocks_num).map(|_| self.generate_block()).collect()
    }

    // generate a new block and submit it through rpc.
    pub fn generate_block(&self) -> H256 {
        self.submit_block(&self.new_block(None, None, None))
    }

    // generate a transaction which spend tip block's cellbase and send it to pool through rpc.
    pub fn generate_transaction(&self) -> H256 {
        self.submit_transaction(&self.new_transaction_spend_tip_cellbase())
    }

    // generate a transaction which spend tip block's cellbase
    pub fn new_transaction_spend_tip_cellbase(&self) -> Transaction {
        let block = self.get_tip_block();
        let cellbase = &block.transactions()[0];
        self.new_transaction(cellbase.hash().to_owned())
    }

    pub fn submit_transaction(&self, transaction: &Transaction) -> H256 {
        self.rpc_client().send_transaction(transaction.into())
    }

    pub fn get_tip_block(&self) -> Block {
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

    pub fn get_block_by_number(&self, number: BlockNumber) -> Block {
        self.rpc_client()
            .get_block_by_number(number)
            .expect("block exists")
            .into()
    }

    pub fn new_block(
        &self,
        bytes_limit: Option<u64>,
        proposals_limit: Option<u64>,
        max_version: Option<u32>,
    ) -> Block {
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

        template.into()
    }

    pub fn new_transaction(&self, hash: H256) -> Transaction {
        self.new_transaction_with_since(hash, 0)
    }

    pub fn new_transaction_with_since(&self, hash: H256, since: u64) -> Transaction {
        let always_success_out_point = OutPoint::new_cell(self.genesis_cellbase_hash.clone(), 1);
        let always_success_script = Script::new(vec![], self.always_success_code_hash.clone());

        TransactionBuilder::default()
            .dep(always_success_out_point)
            .output(CellOutput::new(
                capacity_bytes!(100),
                Bytes::new(),
                always_success_script,
                None,
            ))
            .input(CellInput::new(OutPoint::new_cell(hash, 0), since))
            .build()
    }

    fn prepare_chain_spec(
        &mut self,
        modify_chain_spec: Box<dyn Fn(&mut ChainSpec) -> ()>,
    ) -> Result<(), Error> {
        let integration_spec = include_bytes!("../integration.toml");
        let always_success_cell = include_bytes!("../../script/testdata/always_success");
        let always_success_path = Path::new(&self.dir).join("specs/cells/always_success");
        fs::create_dir_all(format!("{}/specs", self.dir))?;
        fs::create_dir_all(format!("{}/specs/cells", self.dir))?;
        fs::write(&always_success_path, &always_success_cell[..])?;

        let mut spec: ChainSpec =
            toml::from_slice(&integration_spec[..]).expect("chain spec config");
        for r in spec.genesis.system_cells.files.iter_mut() {
            r.absolutize(Path::new(&self.dir).join("specs"));
        }
        modify_chain_spec(&mut spec);

        let consensus = spec.build_consensus().expect("build consensus");
        self.genesis_cellbase_hash
            .clone_from(consensus.genesis_block().transactions()[0].hash());
        self.always_success_code_hash =
            consensus.genesis_block().transactions()[0].outputs()[1].data_hash();

        self.consensus = Some(consensus);

        // write to dir
        fs::write(
            Path::new(&self.dir).join("specs/integration.toml"),
            toml::to_string(&spec).expect("chain spec serialize"),
        )
    }

    fn rewrite_spec(
        &self,
        modify_ckb_config: Box<dyn Fn(&mut CKBAppConfig) -> ()>,
    ) -> Result<(), Error> {
        // rewrite ckb.toml
        let ckb_config_path = format!("{}/ckb.toml", self.dir);
        let mut ckb_config: CKBAppConfig =
            toml::from_slice(&fs::read(&ckb_config_path)?).expect("ckb config");
        ckb_config.block_assembler = Some(BlockAssemblerConfig {
            code_hash: self.always_success_code_hash.clone(),
            args: Default::default(),
            data: JsonBytes::default(),
        });
        modify_ckb_config(&mut ckb_config);
        fs::write(
            &ckb_config_path,
            toml::to_string(&ckb_config).expect("ckb config serialize"),
        )
    }

    fn init_config_file(
        &mut self,
        modify_chain_spec: Box<dyn Fn(&mut ChainSpec) -> ()>,
        modify_ckb_config: Box<dyn Fn(&mut CKBAppConfig) -> ()>,
    ) -> Result<(), Error> {
        let rpc_port = format!("{}", self.rpc_port).to_string();
        let p2p_port = format!("{}", self.p2p_port).to_string();

        Command::new(self.binary.to_owned())
            .args(&[
                "-C",
                &self.dir,
                "init",
                "--chain",
                "integration",
                "--rpc-port",
                &rpc_port,
                "--p2p-port",
                &p2p_port,
            ])
            .output()
            .map(|_| ())?;

        self.prepare_chain_spec(modify_chain_spec)?;
        self.rewrite_spec(modify_ckb_config)?;
        Ok(())
    }

    pub fn assert_tx_pool_size(&self, pending_size: u64, proposed_size: u64) {
        let tx_pool_info = self.rpc_client().tx_pool_info();
        assert_eq!(tx_pool_info.pending.0, pending_size);
        assert_eq!(tx_pool_info.proposed.0, proposed_size);
    }

    pub fn assert_tx_pool_statics(&self, total_tx_size: u64, total_tx_cycles: u64) {
        let tx_pool_info = self.rpc_client().tx_pool_info();
        assert_eq!(tx_pool_info.total_tx_size.0, total_tx_size);
        assert_eq!(tx_pool_info.total_tx_cycles.0, total_tx_cycles);
    }
}
