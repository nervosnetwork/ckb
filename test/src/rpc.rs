use ckb_core::{
    BlockNumber as CoreBlockNumber, EpochNumber as CoreEpochNumber, Version as CoreVersion,
};
use ckb_jsonrpc_types::{
    Alert, Block, BlockNumber, BlockTemplate, BlockView, CellOutputWithOutPoint, CellTransaction,
    CellWithStatus, ChainInfo, DryRunResult, EpochNumber, EpochView, HeaderView, LiveCell,
    LockHashIndexState, Node, OutPoint, PeerState, Transaction, TransactionWithStatus, TxPoolInfo,
    Unsigned, Version,
};
use ckb_util::Mutex;
use jsonrpc_client_core::{expand_params, jsonrpc_client, Result as JsonRpcResult};
use jsonrpc_client_http::{HttpHandle, HttpTransport};
use numext_fixed_hash::H256;

pub struct RpcClient {
    inner: Mutex<Inner<HttpHandle>>,
}

impl RpcClient {
    pub fn new(uri: &str) -> Self {
        let transport = HttpTransport::new().standalone().unwrap();
        let transport = transport
            .handle(uri)
            .expect("ckb uri, e.g. \"http://127.0.0.1:8114\"");
        Self {
            inner: Mutex::new(Inner::new(transport)),
        }
    }

    pub fn inner(&self) -> &Mutex<Inner<HttpHandle>> {
        &self.inner
    }

    pub fn get_block(&self, hash: H256) -> Option<BlockView> {
        self.inner
            .lock()
            .get_block(hash)
            .call()
            .expect("rpc call get_block")
    }

    pub fn get_block_by_number(&self, number: CoreBlockNumber) -> Option<BlockView> {
        self.inner
            .lock()
            .get_block_by_number(BlockNumber(number))
            .call()
            .expect("rpc call get_block_by_number")
    }

    pub fn get_transaction(&self, hash: H256) -> Option<TransactionWithStatus> {
        self.inner
            .lock()
            .get_transaction(hash)
            .call()
            .expect("rpc call get_transaction")
    }

    pub fn get_block_hash(&self, number: CoreBlockNumber) -> Option<H256> {
        self.inner
            .lock()
            .get_block_hash(BlockNumber(number))
            .call()
            .expect("rpc call get_block_hash")
    }

    pub fn get_tip_header(&self) -> HeaderView {
        self.inner
            .lock()
            .get_tip_header()
            .call()
            .expect("rpc call get_block_hash")
    }

    pub fn get_cells_by_lock_hash(
        &self,
        lock_hash: H256,
        from: CoreBlockNumber,
        to: CoreBlockNumber,
    ) -> Vec<CellOutputWithOutPoint> {
        self.inner
            .lock()
            .get_cells_by_lock_hash(lock_hash, BlockNumber(from), BlockNumber(to))
            .call()
            .expect("rpc call get_cells_by_lock_hash")
    }

    pub fn get_live_cell(&self, out_point: OutPoint) -> CellWithStatus {
        self.inner
            .lock()
            .get_live_cell(out_point)
            .call()
            .expect("rpc call get_live_cell")
    }

    pub fn get_tip_block_number(&self) -> CoreBlockNumber {
        self.inner
            .lock()
            .get_tip_block_number()
            .call()
            .expect("rpc call get_tip_block_number")
            .0
    }

    pub fn get_current_epoch(&self) -> EpochView {
        self.inner
            .lock()
            .get_current_epoch()
            .call()
            .expect("rpc call get_current_epoch")
    }

    pub fn get_epoch_by_number(&self, number: CoreEpochNumber) -> Option<EpochView> {
        self.inner
            .lock()
            .get_epoch_by_number(EpochNumber(number))
            .call()
            .expect("rpc call get_epoch_by_number")
    }

    pub fn local_node_info(&self) -> Node {
        self.inner
            .lock()
            .local_node_info()
            .call()
            .expect("rpc call local_node_info")
    }

    pub fn get_peers(&self) -> Vec<Node> {
        self.inner
            .lock()
            .get_peers()
            .call()
            .expect("rpc call get_peers")
    }

    pub fn get_block_template(
        &self,
        bytes_limit: Option<u64>,
        proposals_limit: Option<u64>,
        max_version: Option<CoreVersion>,
    ) -> BlockTemplate {
        let bytes_limit = bytes_limit.map(Unsigned);
        let proposals_limit = proposals_limit.map(Unsigned);
        let max_version = max_version.map(Version);
        self.inner
            .lock()
            .get_block_template(bytes_limit, proposals_limit, max_version)
            .call()
            .expect("rpc call get_block_template")
    }

    pub fn submit_block(&self, work_id: String, block: Block) -> Option<H256> {
        self.inner
            .lock()
            .submit_block(work_id, block)
            .call()
            .expect("rpc call submit_block")
    }

    pub fn get_blockchain_info(&self) -> ChainInfo {
        self.inner
            .lock()
            .get_blockchain_info()
            .call()
            .expect("rpc call get_blockchain_info")
    }

    pub fn send_transaction(&self, tx: Transaction) -> H256 {
        self.inner
            .lock()
            .send_transaction(tx)
            .call()
            .expect("rpc call send_transaction")
    }

    pub fn send_transaction_result(&self, tx: Transaction) -> JsonRpcResult<H256> {
        self.inner.lock().send_transaction(tx).call()
    }

    pub fn send_alert(&self, alert: Alert) {
        self.inner
            .lock()
            .send_alert(alert)
            .call()
            .expect("rpc call send_alert")
    }

    pub fn tx_pool_info(&self) -> TxPoolInfo {
        self.inner
            .lock()
            .tx_pool_info()
            .call()
            .expect("rpc call tx_pool_info")
    }

    pub fn add_node(&self, peer_id: String, address: String) {
        self.inner
            .lock()
            .add_node(peer_id, address)
            .call()
            .expect("rpc call add_node");
    }

    pub fn remove_node(&self, peer_id: String) {
        self.inner
            .lock()
            .remove_node(peer_id)
            .call()
            .expect("rpc call remove_node")
    }

    pub fn process_block_without_verify(&self, block: Block) -> Option<H256> {
        self.inner
            .lock()
            .process_block_without_verify(block)
            .call()
            .expect("rpc call process_block_without verify")
    }

    pub fn get_live_cells_by_lock_hash(
        &self,
        lock_hash: H256,
        page: u64,
        per_page: u64,
        reverse_order: Option<bool>,
    ) -> Vec<LiveCell> {
        self.inner()
            .lock()
            .get_live_cells_by_lock_hash(
                lock_hash,
                Unsigned(page),
                Unsigned(per_page),
                reverse_order,
            )
            .call()
            .expect("rpc call get_live_cells_by_lock_hash")
    }

    pub fn get_transactions_by_lock_hash(
        &self,
        lock_hash: H256,
        page: u64,
        per_page: u64,
        reverse_order: Option<bool>,
    ) -> Vec<CellTransaction> {
        self.inner()
            .lock()
            .get_transactions_by_lock_hash(
                lock_hash,
                Unsigned(page),
                Unsigned(per_page),
                reverse_order,
            )
            .call()
            .expect("rpc call get_transactions_by_lock_hash")
    }

    pub fn index_lock_hash(
        &self,
        lock_hash: H256,
        index_from: Option<CoreBlockNumber>,
    ) -> LockHashIndexState {
        self.inner()
            .lock()
            .index_lock_hash(lock_hash, index_from.map(BlockNumber))
            .call()
            .expect("rpc call index_lock_hash")
    }

    pub fn deindex_lock_hash(&self, lock_hash: H256) {
        self.inner()
            .lock()
            .deindex_lock_hash(lock_hash)
            .call()
            .expect("rpc call deindex_lock_hash")
    }

    pub fn get_lock_hash_index_states(&self) -> Vec<LockHashIndexState> {
        self.inner()
            .lock()
            .get_lock_hash_index_states()
            .call()
            .expect("rpc call get_lock_hash_index_states")
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
    pub fn get_current_epoch(&mut self) -> RpcRequest<EpochView>;
    pub fn get_epoch_by_number(&mut self, number: EpochNumber) -> RpcRequest<Option<EpochView>>;
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

    pub fn send_alert(&mut self, alert: Alert) -> RpcRequest<()>;

    pub fn add_node(&mut self, peer_id: String, address: String) -> RpcRequest<()>;
    pub fn remove_node(&mut self, peer_id: String) -> RpcRequest<()>;
    pub fn process_block_without_verify(&mut self, _data: Block) -> RpcRequest<Option<H256>>;

    pub fn get_live_cells_by_lock_hash(&mut self, lock_hash: H256, page: Unsigned, per_page: Unsigned, reverse_order: Option<bool>) -> RpcRequest<Vec<LiveCell>>;
    pub fn get_transactions_by_lock_hash(&mut self, lock_hash: H256, page: Unsigned, per_page: Unsigned, reverse_order: Option<bool>) -> RpcRequest<Vec<CellTransaction>>;
    pub fn index_lock_hash(&mut self, lock_hash: H256, index_from: Option<BlockNumber>) -> RpcRequest<LockHashIndexState>;
    pub fn deindex_lock_hash(&mut self, lock_hash: H256) -> RpcRequest<()>;
    pub fn get_lock_hash_index_states(&mut self) -> RpcRequest<Vec<LockHashIndexState>>;
});
