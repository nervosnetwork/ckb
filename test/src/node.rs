use tempfile::tempdir;
use crate::rpc::RpcClient;
use crate::sleep;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::header::{HeaderBuilder, Seal};
use ckb_core::script::Script;
use ckb_core::transaction::{CellInput, CellOutput, OutPoint, Transaction, TransactionBuilder};
use fs_extra::dir::{copy, CopyOptions};
use jsonrpc_client_http::{HttpHandle, HttpTransport};
use jsonrpc_types::{BlockTemplate, CellbaseTemplate};
use log::info;
use numext_fixed_hash::H256;
use rand;
use std::fs::File;
use std::io::{Error, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use ckb_network::{TimerToken, CKBProtocolHandler, ProtocolId, CKBProtocol, CKBProtocolContext, PeerIndex, NetworkConfig, NetworkService};
use ckb_network::futures::sync::mpsc::channel;
use std::sync::Arc;
use bytes::Bytes;
use ckb_network::multiaddr::ToMultiaddr;

const DEFAULT_CONFIG_FILE: &str = "default.toml";

pub struct Node {
    pub binary: String,
    pub dir: String,
    pub p2p_port: u16,
    pub rpc_port: u16,
    pub node_id: Option<String>,
    guard: Option<ProcessGuard>,
}

struct ProcessGuard(Child);

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

    pub fn start(&mut self) {
        self.init_config_file().expect("failed to init config file");
        let child_process = Command::new(self.binary.to_owned())
            .args(&[
                "run",
                "-c",
                &format!("{}/{}", self.dir, DEFAULT_CONFIG_FILE),
            ])
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
                break;
            }
            sleep(1);
        }
    }

    pub fn connect(&self, node: &Node) {
        let node_info = node
            .rpc_client()
            .local_node_info()
            .call()
            .expect("rpc call local_node_info failed");
        self.rpc_client()
            .add_node(
                node_info.node_id,
                format!("/ip4/127.0.0.1/tcp/{}", node.p2p_port),
            )
            .call()
            .expect("rpc call add_node failed");
    }

    pub fn rpc_client(&self) -> RpcClient<HttpHandle> {
        let transport = HttpTransport::new().standalone().unwrap();
        let transport_handle = transport
            .handle(&format!("http://127.0.0.1:{}/", self.rpc_port))
            .unwrap();
        RpcClient::new(transport_handle)
    }

    // workaround: submit_pow_solution rpc doesn't working since miner is running as a standalone process
    // TODO: remove clicker pow engine and cleanup rpc
    pub fn generate_block(&self) -> H256 {
        let result = self
            .rpc_client()
            .submit_block("".to_owned(), (&self.new_block()).into())
            .call()
            .expect("rpc call submit_block failed");
        result.expect("submit_block result none")
    }

    pub fn generate_transaction(&self) -> H256 {
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
        let block = rpc
            .get_block(block_hash)
            .call()
            .expect("rpc call get_block failed")
            .expect("get_block result none");
        let cellbase: Transaction = block.commit_transactions[0].clone().into();
        rpc.send_transaction((&self.new_transaction(cellbase.hash())).into())
            .call()
            .expect("rpc call send_transaction failed")
    }

    pub fn send_traced_transaction(&self) -> H256 {
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
        let block = rpc
            .get_block(block_hash)
            .call()
            .expect("rpc call get_block failed")
            .expect("get_block result none");
        let cellbase: Transaction = block.commit_transactions[0].clone().into();
        rpc.trace_transaction((&self.new_transaction(cellbase.hash())).into())
            .call()
            .expect("rpc call send_transaction failed")
    }

    pub fn new_block(&self) -> Block {
        let template = self
            .rpc_client()
            .get_block_template(None, None, None)
            .call()
            .expect("rpc call get_block_template failed");

        let BlockTemplate {
            version,
            difficulty,
            current_time,
            number,
            parent_hash,
            uncles,                // Vec<UncleTemplate>
            commit_transactions,   // Vec<TransactionTemplate>
            proposal_transactions, // Vec<ProposalShortId>
            cellbase,              // CellbaseTemplate
            ..
        } = template;

        let (cellbase_id, cellbase) = {
            let CellbaseTemplate { hash, data, .. } = cellbase;
            (hash, data)
        };

        let header_builder = HeaderBuilder::default()
            .version(version)
            .number(number)
            .difficulty(difficulty)
            .timestamp(current_time)
            .parent_hash(parent_hash)
            .cellbase_id(cellbase_id)
            .seal(Seal::new(rand::random(), Vec::new()));

        BlockBuilder::default()
            .uncles(uncles.into_iter().map(Into::into).collect())
            .commit_transaction(cellbase.into())
            .commit_transactions(commit_transactions.into_iter().map(Into::into).collect())
            .proposal_transactions(proposal_transactions.into_iter().map(Into::into).collect())
            .with_header_builder(header_builder)
    }

    pub fn new_transaction(&self, hash: H256) -> Transaction {
        // OutPoint and Script reference hash values are from spec#always_success_type_hash test
        let script = Script::always_success();

        TransactionBuilder::default()
            .output(CellOutput::new(50000, vec![], script.clone(), None))
            .input(CellInput::new(OutPoint::new(hash, 0), vec![]))
            .build()
    }

    fn init_config_file(&self) -> Result<(), Error> {
        let mut options = CopyOptions::new();
        options.copy_inside = true;
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/nodes_template");
        let dest = PathBuf::from(&self.dir);
        copy(source, &dest, &options).expect("failed to copy template");

        let mut data = String::new();
        {
            let mut file = File::open(dest.join(DEFAULT_CONFIG_FILE))?;
            file.read_to_string(&mut data)?;
        }
        let new_data = data
            .replace("P2P_PORT", &self.p2p_port.to_string())
            .replace("RPC_PORT", &self.rpc_port.to_string());
        let mut file = File::create(dest.join(DEFAULT_CONFIG_FILE))?;
        file.write_all(new_data.as_bytes())?;
        Ok(())
    }
}

pub struct TestNode {
    network: Arc<NetworkService>
}


impl TestNode {
    pub fn new() -> Self {
        let mut config = NetworkConfig::default();
        config.listen_addresses = vec!["/ip4/0.0.0.0/tcp/12345".to_multiaddr().unwrap()];
        config.ping_interval_secs = 15;
        config.ping_timeout_secs = 20;
        config.max_peers = 125;
        config.max_outbound_peers = 30;
        config.connect_outbound_interval_secs = 1;
        config.path = tempdir().unwrap().path().to_path_buf();

        let (sender, receiver) = channel(std::u8::MAX as usize);

        let protocols = vec![
        (
            CKBProtocol::new(
                "syn".to_string(),
                100,
                &[1][..],
                sender.clone(),
            ),
            Arc::new(DummyProtocolHandler) as Arc<_>
        )];

        let network_service = NetworkService::run_in_thread(&config, protocols, receiver)
            .expect("Create and start network");

        Self {
            network: Arc::new(network_service)
        }
    }

    pub fn connect(&self, node: &Node) {
        let node_info = node
            .rpc_client()
            .local_node_info()
            .call()
            .expect("rpc call local_node_info failed");
        self.network.add_node(
            &node_info.node_id.parse().expect("invalid peer_id"),
            format!("/ip4/127.0.0.1/tcp/{}", node.p2p_port).parse().expect("invalid address"),
        );
    }

    pub fn send(&self, protocol_id: ProtocolId, peer: PeerIndex, data: Vec<u8>) {
        self.network.with_protocol_context(protocol_id, |nc| {
            info!("{:?}", nc.send(peer, data));
        });
    }
}

pub struct DummyProtocolHandler;

impl CKBProtocolHandler for DummyProtocolHandler {
    fn initialize(&self, _nc: Box<dyn CKBProtocolContext>) {

    }

    fn received(&self, _nc: Box<dyn CKBProtocolContext>, peer: PeerIndex, data: Bytes) {
        info!("received {:?}, {:?}", peer, data);
    }

    fn connected(&self, _nc: Box<dyn CKBProtocolContext>, _peer: PeerIndex) {

    }

    fn disconnected(&self, _nc: Box<dyn CKBProtocolContext>, _peer: PeerIndex) {

    }

    fn timer_triggered(&self, _nc: Box<dyn CKBProtocolContext>, _timer: TimerToken) {

    }
}
