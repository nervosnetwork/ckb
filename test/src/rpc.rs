use jsonrpc_client_core::{expand_params, jsonrpc_client};
use jsonrpc_types::{
    Block, BlockTemplate, BlockView, HeaderView, Node, Transaction, TransactionWithStatus,
    TxPoolInfo,
};
use numext_fixed_hash::H256;

jsonrpc_client!(pub struct RpcClient {
    pub fn local_node_info(&mut self) -> RpcRequest<Node>;
    pub fn get_peers(&mut self) -> RpcRequest<Vec<Node>>;

    pub fn add_node(&mut self, peer_id: String, address: String) -> RpcRequest<()>;

    pub fn get_block_template(
        &mut self,
        bytes_limit: Option<String>,
        proposals_limit: Option<String>,
        max_version: Option<u32>
    ) -> RpcRequest<BlockTemplate>;

    pub fn submit_block(&mut self, work_id: String, data: Block) -> RpcRequest<Option<H256>>;

    pub fn send_transaction(&mut self, tx: Transaction) -> RpcRequest<H256>;
    pub fn tx_pool_info(&mut self) -> RpcRequest<TxPoolInfo>;

    pub fn get_block(&mut self, hash: H256) -> RpcRequest<Option<BlockView>>;
    pub fn get_transaction(&mut self, hash: H256) -> RpcRequest<Option<TransactionWithStatus>>;
    pub fn get_block_hash(&mut self, number: String) -> RpcRequest<Option<H256>>;
    pub fn get_tip_header(&mut self) -> RpcRequest<HeaderView>;
    pub fn get_tip_block_number(&mut self) -> RpcRequest<String>;
    pub fn enqueue_test_transaction(&mut self, tx: Transaction) -> RpcRequest<H256>;
});
