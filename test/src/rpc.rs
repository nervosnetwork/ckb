use ckb_core::{
    BlockNumber as CoreBlockNumber, EpochNumber as CoreEpochNumber, Version as CoreVersion,
};
use jsonrpc_client_core::{expand_params, jsonrpc_client};
use jsonrpc_client_http::{HttpHandle, HttpTransport};
use jsonrpc_types::{
    Block, BlockNumber, BlockTemplate, BlockView, CellOutputWithOutPoint, CellWithStatus,
    ChainInfo, DryRunResult, EpochExt, EpochNumber, HeaderView, Node, OutPoint, PeerState,
    Transaction, TransactionWithStatus, TxPoolInfo, Unsigned, Version,
};
use numext_fixed_hash::H256;

pub struct RpcClient {
    inner: Inner<HttpHandle>,
}

impl RpcClient {
    pub fn new(uri: &str) -> Self {
        let transport = HttpTransport::new().standalone().unwrap();
        let transport = transport
            .handle(uri)
            .expect("ckb uri, e.g. \"http://127.0.0.1:8114\"");
        Self {
            inner: Inner::new(transport),
        }
    }

    pub fn inner(&mut self) -> &mut Inner<HttpHandle> {
        &mut self.inner
    }

    pub fn get_block(&mut self, hash: H256) -> Option<BlockView> {
        self.inner
            .get_block(hash)
            .call()
            .expect("rpc call get_block")
    }

    pub fn get_block_by_number(&mut self, number: CoreBlockNumber) -> Option<BlockView> {
        self.inner
            .get_block_by_number(BlockNumber(number))
            .call()
            .expect("rpc call get_block_by_number")
    }

    pub fn get_transaction(&mut self, hash: H256) -> Option<TransactionWithStatus> {
        self.inner
            .get_transaction(hash)
            .call()
            .expect("rpc call get_transaction")
    }

    pub fn get_block_hash(&mut self, number: CoreBlockNumber) -> Option<H256> {
        self.inner
            .get_block_hash(BlockNumber(number))
            .call()
            .expect("rpc call get_block_hash")
    }

    pub fn get_tip_header(&mut self) -> HeaderView {
        self.inner
            .get_tip_header()
            .call()
            .expect("rpc call get_block_hash")
    }

    pub fn get_cells_by_lock_hash(
        &mut self,
        lock_hash: H256,
        from: CoreBlockNumber,
        to: CoreBlockNumber,
    ) -> Vec<CellOutputWithOutPoint> {
        self.inner
            .get_cells_by_lock_hash(lock_hash, BlockNumber(from), BlockNumber(to))
            .call()
            .expect("rpc call get_cells_by_lock_hash")
    }

    pub fn get_live_cell(&mut self, out_point: OutPoint) -> CellWithStatus {
        self.inner
            .get_live_cell(out_point)
            .call()
            .expect("rpc call get_live_cell")
    }

    pub fn get_tip_block_number(&mut self) -> CoreBlockNumber {
        self.inner
            .get_tip_block_number()
            .call()
            .expect("rpc call get_tip_block_number")
            .0
    }

    pub fn get_current_epoch(&mut self) -> EpochExt {
        self.inner
            .get_current_epoch()
            .call()
            .expect("rpc call get_current_epoch")
    }

    pub fn get_epoch_by_number(&mut self, number: CoreEpochNumber) -> Option<EpochExt> {
        self.inner
            .get_epoch_by_number(EpochNumber(number))
            .call()
            .expect("rpc call get_epoch_by_number")
    }

    pub fn local_node_info(&mut self) -> Node {
        self.inner
            .local_node_info()
            .call()
            .expect("rpc call local_node_info")
    }

    pub fn get_peers(&mut self) -> Vec<Node> {
        self.inner.get_peers().call().expect("rpc call get_peers")
    }

    pub fn get_block_template(
        &mut self,
        bytes_limit: Option<u64>,
        proposals_limit: Option<u64>,
        max_version: Option<CoreVersion>,
    ) -> BlockTemplate {
        let bytes_limit = bytes_limit.map(Unsigned);
        let proposals_limit = proposals_limit.map(Unsigned);
        let max_version = max_version.map(Version);
        self.inner
            .get_block_template(bytes_limit, proposals_limit, max_version)
            .call()
            .expect("rpc call get_block_template")
    }

    pub fn submit_block(&mut self, work_id: String, block: Block) -> Option<H256> {
        self.inner
            .submit_block(work_id, block)
            .call()
            .expect("rpc call submit_block")
    }

    pub fn get_blockchain_info(&mut self) -> ChainInfo {
        self.inner
            .get_blockchain_info()
            .call()
            .expect("rpc call get_blockchain_info")
    }

    pub fn send_transaction(&mut self, tx: Transaction) -> H256 {
        self.inner
            .send_transaction(tx)
            .call()
            .expect("rpc call send_transaction")
    }

    pub fn tx_pool_info(&mut self) -> TxPoolInfo {
        self.inner
            .tx_pool_info()
            .call()
            .expect("rpc call tx_pool_info")
    }

    pub fn add_node(&mut self, peer_id: String, address: String) {
        self.inner
            .add_node(peer_id, address)
            .call()
            .expect("rpc call add_node");
    }

    pub fn remove_node(&mut self, peer_id: String) {
        self.inner
            .remove_node(peer_id)
            .call()
            .expect("rpc call remove_node")
    }

    pub fn process_block_without_verify(&mut self, block: Block) -> Option<H256> {
        self.inner
            .process_block_without_verify(block)
            .call()
            .expect("rpc call process_block_without verify")
    }
}

jsonrpc_client!(pub struct Inner {
    pub fn get_block(&mut self, _hash: H256) -> RpcRequest<Option<BlockView>>;
    pub fn get_block_by_number(&mut self, _number: BlockNumber) -> RpcRequest<Option<BlockView>>;
    pub fn get_transaction(&mut self, _hash: H256) -> RpcRequest<Option<TransactionWithStatus>>;
    pub fn get_block_hash(&mut self, _number: BlockNumber) -> RpcRequest<Option<H256>>;
    pub fn get_tip_header(&mut self) -> RpcRequest<HeaderView>;
    pub fn get_cells_by_lock_hash(
        &mut self,
        _lock_hash: H256,
        _from: BlockNumber,
        _to: BlockNumber
    ) -> RpcRequest<Vec<CellOutputWithOutPoint>>;
    pub fn get_live_cell(&mut self, _out_point: OutPoint) -> RpcRequest<CellWithStatus>;
    pub fn get_tip_block_number(&mut self) -> RpcRequest<BlockNumber>;
    pub fn get_current_epoch(&mut self) -> RpcRequest<EpochExt>;
    pub fn get_epoch_by_number(&mut self, number: EpochNumber) -> RpcRequest<Option<EpochExt>>;
    pub fn local_node_info(&mut self) -> RpcRequest<Node>;
    pub fn get_peers(&mut self) -> RpcRequest<Vec<Node>>;
    pub fn get_block_template(
        &mut self,
        bytes_limit: Option<Unsigned>,
        proposals_limit: Option<Unsigned>,
        max_version: Option<Version>
    ) -> RpcRequest<BlockTemplate>;
    pub fn submit_block(&mut self, _work_id: String, _data: Block) -> RpcRequest<Option<H256>>;
    pub fn get_blockchain_info(&mut self) -> RpcRequest<ChainInfo>;
    pub fn get_peers_state(&mut self) -> RpcRequest<Vec<PeerState>>;
    pub fn compute_transaction_hash(&mut self, tx: Transaction) -> RpcRequest<H256>;
    pub fn dry_run_transaction(&mut self, _tx: Transaction) -> RpcRequest<DryRunResult>;
    pub fn send_transaction(&mut self, tx: Transaction) -> RpcRequest<H256>;
    pub fn tx_pool_info(&mut self) -> RpcRequest<TxPoolInfo>;

    pub fn add_node(&mut self, peer_id: String, address: String) -> RpcRequest<()>;
    pub fn remove_node(&mut self, peer_id: String) -> RpcRequest<()>;
    pub fn process_block_without_verify(&mut self, _data: Block) -> RpcRequest<Option<H256>>;
});
