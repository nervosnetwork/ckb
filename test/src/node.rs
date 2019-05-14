use crate::rpc::RpcClient;
use crate::utils::wait_until;
use ckb_app_config::{CKBAppConfig, MinerAppConfig};
use ckb_chain_spec::ChainSpecConfig;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::header::{HeaderBuilder, Seal};
use ckb_core::script::Script;
use ckb_core::transaction::{CellInput, CellOutput, OutPoint, Transaction, TransactionBuilder};
use ckb_core::{capacity_bytes, BlockNumber, Bytes, Capacity};
use jsonrpc_client_http::{HttpHandle, HttpTransport};
use jsonrpc_types::{BlockTemplate, CellbaseTemplate, Unsigned, Version};
use log::info;
use numext_fixed_hash::{h256, H256};
use rand;
use std::convert::TryInto;
use std::fs;
use std::io::Error;
use std::process::{self, Child, Command, Stdio};

pub struct Node {
    pub binary: String,
    pub dir: String,
    pub p2p_port: u16,
    pub rpc_port: u16,
    pub node_id: Option<String>,
    guard: Option<ProcessGuard>,
}

struct ProcessGuard(pub Child);

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        match self.0.kill() {
            Err(e) => info!("Could not kill ckb process: {}", e),
            Ok(_) => info!("Successfully killed ckb process"),
        }
    }
}

impl Node {
    pub fn new(binary: &str, dir: &str, p2p_port: u16, rpc_port: u16) -> Self {
        Self {
            binary: binary.to_string(),
            dir: dir.to_string(),
            p2p_port,
            rpc_port,
            node_id: None,
            guard: None,
        }
    }

    pub fn start(
        &mut self,
        modify_chain_spec: Box<dyn Fn(&mut ChainSpecConfig) -> ()>,
        modify_ckb_config: Box<dyn Fn(&mut CKBAppConfig) -> ()>,
    ) {
        self.init_config_file(modify_chain_spec, modify_ckb_config)
            .expect("failed to init config file");

        let child_process = Command::new(self.binary.to_owned())
            .env("RUST_BACKTRACE", "full")
            .args(&["-C", &self.dir, "run"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("failed to run binary");
        self.guard = Some(ProcessGuard(child_process));
        info!("Started node with working dir: {}", self.dir);

        let mut client = self.rpc_client();
        loop {
            if let Ok(result) = client.local_node_info().call() {
                info!("RPC service ready, {:?}", result);
                self.node_id = Some(result.node_id);
                assert!(client.tx_pool_info().call().is_ok());
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
        let node_info = node
            .rpc_client()
            .local_node_info()
            .call()
            .expect("rpc call local_node_info failed");

        let node_id = node_info.node_id;
        let mut rpc_client = self.rpc_client();
        rpc_client
            .add_node(
                node_id.clone(),
                format!("/ip4/127.0.0.1/tcp/{}", node.p2p_port),
            )
            .call()
            .expect("rpc call add_node failed");

        let result = wait_until(5, || {
            let peers = rpc_client
                .get_peers()
                .call()
                .expect("rpc call get_peers failed");
            peers.iter().any(|peer| peer.node_id == node_id)
        });

        if !result {
            panic!("Connect timeout, node {}", node_id);
        }
    }

    pub fn disconnect(&self, node: &Node) {
        let node_info = node
            .rpc_client()
            .local_node_info()
            .call()
            .expect("rpc call local_node_info failed");

        let node_id = node_info.node_id;
        let mut rpc_client = self.rpc_client();
        rpc_client
            .remove_node(node_id.clone())
            .call()
            .expect("rpc call add_node failed");

        let result = wait_until(5, || {
            let peers = rpc_client
                .get_peers()
                .call()
                .expect("rpc call get_peers failed");
            peers.iter().all(|peer| peer.node_id != node_id)
        });

        if !result {
            panic!("Disconnect timeout, node {}", node_id);
        }
    }

    pub fn waiting_for_sync(&self, node: &Node, target: BlockNumber, timeout: u64) {
        let mut self_rpc_client = self.rpc_client();
        let mut node_rpc_client = node.rpc_client();
        let (mut self_tip_number, mut node_tip_number) = (0, 0);

        let result = wait_until(timeout, || {
            self_tip_number = self_rpc_client
                .get_tip_block_number()
                .call()
                .expect("rpc call get_tip_block_number failed")
                .0;
            node_tip_number = node_rpc_client
                .get_tip_block_number()
                .call()
                .expect("rpc call get_tip_block_number failed")
                .0;
            self_tip_number == node_tip_number && target == self_tip_number
        });

        if !result {
            panic!(
                "Waiting for sync timeout, self_tip_number: {}, node_tip_number: {}",
                self_tip_number, node_tip_number
            );
        }
    }

    pub fn rpc_client(&self) -> RpcClient<HttpHandle> {
        let transport = HttpTransport::new().standalone().unwrap();
        let transport_handle = transport
            .handle(&format!("http://127.0.0.1:{}/", self.rpc_port))
            .unwrap();
        RpcClient::new(transport_handle)
    }

    pub fn submit_block(&self, block: &Block) -> H256 {
        let result = self
            .rpc_client()
            .submit_block("".to_owned(), block.into())
            .call()
            .expect("rpc call submit_block failed");
        result.expect("submit_block result none")
    }

    pub fn process_block_without_verify(&self, block: &Block) -> H256 {
        let result = self
            .rpc_client()
            .process_block_without_verify(block.into())
            .call()
            .expect("rpc call process_block_without_verify failed");
        result.expect("process_block_without_verify result none")
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
        let cellbase: Transaction = block.transactions()[0]
            .clone()
            .try_into()
            .expect("parse cellbase transaction failed");
        self.new_transaction(cellbase.hash().to_owned())
    }

    pub fn submit_transaction(&self, transaction: &Transaction) -> H256 {
        self.rpc_client()
            .send_transaction(transaction.into())
            .call()
            .expect("rpc call send_transaction failed")
    }

    pub fn get_tip_block(&self) -> Block {
        let mut rpc = self.rpc_client();
        let tip_number = rpc
            .get_tip_block_number()
            .call()
            .expect("rpc call get_tip_block_number failed");
        let block_hash = rpc
            .get_block_hash(tip_number)
            .call()
            .expect("rpc call get_block_hash failed")
            .expect("get_block_hash result none");
        rpc.get_block(block_hash)
            .call()
            .expect("rpc call get_block failed")
            .expect("get_block result none")
            .try_into()
            .expect("block")
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
        let bytes_limit = bytes_limit.map(Unsigned);
        let proposals_limit = proposals_limit.map(Unsigned);
        let max_version = max_version.map(Version);
        let template = self
            .rpc_client()
            .get_block_template(bytes_limit, proposals_limit, max_version)
            .call()
            .expect("rpc call get_block_template failed");

        let BlockTemplate {
            version,
            difficulty,
            current_time,
            number,
            parent_hash,
            uncles,       // Vec<UncleTemplate>
            transactions, // Vec<TransactionTemplate>
            proposals,    // Vec<ProposalShortId>
            cellbase,     // CellbaseTemplate
            ..
        } = template;

        let cellbase = {
            let CellbaseTemplate { data, .. } = cellbase;
            data
        };

        let header_builder = HeaderBuilder::default()
            .version(version.0)
            .number(number.0)
            .difficulty(difficulty)
            .timestamp(current_time.0)
            .parent_hash(parent_hash)
            .seal(Seal::new(rand::random(), Bytes::default()));

        BlockBuilder::default()
            .uncles(
                uncles
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()
                    .expect("parse uncles failed"),
            )
            .transaction(cellbase.try_into().expect("parse cellbase failed"))
            .transactions(
                transactions
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()
                    .expect("parse commit transactions failed"),
            )
            .proposals(
                proposals
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()
                    .expect("parse proposal transactions failed"),
            )
            .header_builder(header_builder)
    }

    pub fn new_transaction(&self, hash: H256) -> Transaction {
        self.new_transaction_with_since(hash, 0)
    }

    pub fn new_transaction_with_since(&self, hash: H256, since: u64) -> Transaction {
        // OutPoint and Script reference hash values are from spec#always_success_type_hash test
        let out_point = OutPoint::new_cell(
            h256!("0xf8532f2ed92aad146878dca1d5ad9840e9c803ab85d1361652500eaee09c9038"),
            0,
        );
        let script = Script::new(
            vec![],
            h256!("0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5"),
        );

        TransactionBuilder::default()
            .dep(out_point)
            .output(CellOutput::new(
                capacity_bytes!(50_000),
                Bytes::new(),
                script,
                None,
            ))
            .input(CellInput::new(OutPoint::new_cell(hash, 0), since, vec![]))
            .build()
    }

    fn prepare_chain_spec(
        &self,
        config_path: &str,
        modify_chain_spec: Box<dyn Fn(&mut ChainSpecConfig) -> ()>,
    ) -> Result<(), Error> {
        let integration_spec = include_bytes!("../integration.toml");
        let always_success_cell = include_bytes!("../../resource/specs/cells/always_success");
        fs::create_dir_all(format!("{}/specs", self.dir))?;
        fs::create_dir_all(format!("{}/specs/cells", self.dir))?;
        fs::write(
            format!("{}/specs/cells/always_success", self.dir),
            &always_success_cell[..],
        )?;

        let mut spec_config: ChainSpecConfig =
            toml::from_slice(&integration_spec[..]).expect("chain spec config");
        modify_chain_spec(&mut spec_config);
        // write to dir
        fs::write(
            &config_path,
            toml::to_string(&spec_config).expect("chain spec serialize"),
        )
    }

    fn rewrite_spec(
        &self,
        config_path: &str,
        modify_ckb_config: Box<dyn Fn(&mut CKBAppConfig) -> ()>,
    ) -> Result<(), Error> {
        // rewrite ckb.toml
        let ckb_config_path = format!("{}/ckb.toml", self.dir);
        let mut ckb_config: CKBAppConfig =
            toml::from_slice(&fs::read(&ckb_config_path)?).expect("ckb config");
        ckb_config.chain.spec = config_path.into();
        modify_ckb_config(&mut ckb_config);
        fs::write(
            &ckb_config_path,
            toml::to_string(&ckb_config).expect("ckb config serialize"),
        )?;
        // rewrite ckb-miner.toml
        let miner_config_path = format!("{}/ckb-miner.toml", self.dir);
        let mut miner_config: MinerAppConfig =
            toml::from_slice(&fs::read(&miner_config_path)?).expect("miner config");
        miner_config.chain.spec = config_path.into();
        fs::write(
            &miner_config_path,
            toml::to_string(&miner_config).expect("miner config serialize"),
        )
    }

    fn init_config_file(
        &self,
        modify_chain_spec: Box<dyn Fn(&mut ChainSpecConfig) -> ()>,
        modify_ckb_config: Box<dyn Fn(&mut CKBAppConfig) -> ()>,
    ) -> Result<(), Error> {
        let rpc_port = format!("{}", self.rpc_port).to_string();
        let p2p_port = format!("{}", self.p2p_port).to_string();

        Command::new(self.binary.to_owned())
            .args(&[
                "-C",
                &self.dir,
                "init",
                "--spec",
                "integration",
                "--rpc-port",
                &rpc_port,
                "--p2p-port",
                &p2p_port,
            ])
            .output()
            .map(|_| ())?;

        let spec_config_path = format!("{}/specs/integration.toml", self.dir);
        self.prepare_chain_spec(&spec_config_path, modify_chain_spec)?;
        self.rewrite_spec(&spec_config_path, modify_ckb_config)?;
        Ok(())
    }

    pub fn assert_tx_pool_size(&self, pending_size: u64, proposed_size: u64) {
        let tx_pool_info = self
            .rpc_client()
            .tx_pool_info()
            .call()
            .expect("rpc call tx_pool_info failed");
        assert_eq!(tx_pool_info.pending.0, pending_size);
        assert_eq!(tx_pool_info.proposed.0, proposed_size);
    }
}
