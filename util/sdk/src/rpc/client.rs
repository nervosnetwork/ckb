use ckb_jsonrpc_types::{
    BlockNumber, BlockView, CellOutputWithOutPoint, CellWithStatus, ChainInfo, EpochNumber,
    EpochView, HeaderView, Node, OutPoint, Transaction, TransactionWithStatus, TxPoolInfo,
};
use jsonrpc_client_core::{expand_params, jsonrpc_client};
use jsonrpc_client_http::{HttpHandle, HttpTransport};
use serde_derive::{Deserialize, Serialize};

use numext_fixed_hash::H256;

#[derive(Serialize, Deserialize)]
pub struct Nodes(pub Vec<Node>);

#[derive(Serialize, Deserialize)]
pub struct OptionTransactionWithStatus(pub Option<TransactionWithStatus>);

#[derive(Serialize, Deserialize)]
pub struct CellOutputWithOutPoints(pub Vec<CellOutputWithOutPoint>);

#[derive(Serialize, Deserialize)]
pub struct OptionBlockView(pub Option<BlockView>);

#[derive(Serialize, Deserialize)]
pub struct OptionH256(pub Option<H256>);

#[derive(Serialize, Deserialize)]
pub struct OptionEpochView(pub Option<EpochView>);

jsonrpc_client!(pub struct RpcClient {
    pub fn get_blockchain_info(&mut self) -> RpcRequest<ChainInfo>;
    pub fn local_node_info(&mut self) -> RpcRequest<Node>;
    pub fn get_peers(&mut self) -> RpcRequest<Nodes>;
    pub fn add_node(&mut self, peer_id: String, address: String) -> RpcRequest<()>;

    pub fn tx_pool_info(&mut self) -> RpcRequest<TxPoolInfo>;

    pub fn send_transaction(&mut self, tx: Transaction) -> RpcRequest<H256>;
    pub fn get_transaction(&mut self, hash: H256) -> RpcRequest<OptionTransactionWithStatus>;
    pub fn get_cells_by_lock_hash(&mut self, lock_hash: H256, from: BlockNumber, to: BlockNumber) -> RpcRequest<CellOutputWithOutPoints>;
    pub fn get_live_cell(&mut self, out_point: OutPoint) -> RpcRequest<CellWithStatus>;

    pub fn get_tip_header(&mut self) -> RpcRequest<HeaderView>;
    pub fn get_current_epoch(&mut self) -> RpcRequest<EpochView>;
    pub fn get_epoch_by_number(&mut self, number: EpochNumber) -> RpcRequest<OptionEpochView>;
    pub fn get_block(&mut self, hash: H256) -> RpcRequest<OptionBlockView>;
    pub fn get_block_hash(&mut self, number: BlockNumber) -> RpcRequest<OptionH256>;
    pub fn get_block_by_number(&mut self, number: BlockNumber) -> RpcRequest<OptionBlockView>;
    pub fn get_tip_block_number(&mut self) -> RpcRequest<BlockNumber>;
});

impl RpcClient<HttpHandle> {
    pub fn from_uri(server: &str) -> RpcClient<HttpHandle> {
        let transport = HttpTransport::new().standalone().unwrap();
        let transport_handle = transport.handle(server).unwrap();
        RpcClient::new(transport_handle)
    }
}

pub type HttpRpcClient = RpcClient<HttpHandle>;
