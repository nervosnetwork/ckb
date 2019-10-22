use ckb_jsonrpc_types::{
    Alert, BannedAddr, Block, BlockNumber, BlockReward, BlockTemplate, BlockView, Capacity,
    CellOutputWithOutPoint, CellTransaction, CellWithStatus, ChainInfo, Cycle, DryRunResult,
    EpochNumber, EpochView, HeaderView, LiveCell, LockHashIndexState, Node, OutPoint, PeerState,
    Timestamp, Transaction, TransactionWithStatus, TxPoolInfo, Uint64, Version,
};
use ckb_types::core::{
    BlockNumber as CoreBlockNumber, Capacity as CoreCapacity, EpochNumber as CoreEpochNumber,
    Version as CoreVersion,
};
use ckb_types::{packed::Byte32, prelude::*, H256};
use ckb_util::Mutex;
use jsonrpc_client_core::{expand_params, jsonrpc_client, Result as JsonRpcResult};
use jsonrpc_client_http::{HttpHandle, HttpTransport};

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

    pub fn get_block(&self, hash: Byte32) -> Option<BlockView> {
        self.inner
            .lock()
            .get_block(hash.unpack())
            .call()
            .expect("rpc call get_block")
    }

    pub fn get_fork_block(&self, hash: Byte32) -> Option<BlockView> {
        self.inner
            .lock()
            .get_fork_block(hash.unpack())
            .call()
            .expect("rpc call get_fork_block")
    }

    pub fn get_block_by_number(&self, number: CoreBlockNumber) -> Option<BlockView> {
        self.inner
            .lock()
            .get_block_by_number(number.into())
            .call()
            .expect("rpc call get_block_by_number")
    }

    pub fn get_header(&self, hash: Byte32) -> Option<HeaderView> {
        self.inner
            .lock()
            .get_header(hash.unpack())
            .call()
            .expect("rpc call get_header")
    }

    pub fn get_header_by_number(&self, number: CoreBlockNumber) -> Option<HeaderView> {
        self.inner
            .lock()
            .get_header_by_number(number.into())
            .call()
            .expect("rpc call get_header_by_number")
    }

    pub fn get_transaction(&self, hash: Byte32) -> Option<TransactionWithStatus> {
        self.inner
            .lock()
            .get_transaction(hash.unpack())
            .call()
            .expect("rpc call get_transaction")
    }

    pub fn get_block_hash(&self, number: CoreBlockNumber) -> Option<Byte32> {
        self.inner
            .lock()
            .get_block_hash(number.into())
            .call()
            .expect("rpc call get_block_hash")
            .map(|x| x.pack())
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
        lock_hash: Byte32,
        from: CoreBlockNumber,
        to: CoreBlockNumber,
    ) -> Vec<CellOutputWithOutPoint> {
        self.inner
            .lock()
            .get_cells_by_lock_hash(lock_hash.unpack(), from.into(), to.into())
            .call()
            .expect("rpc call get_cells_by_lock_hash")
    }

    pub fn get_live_cell(&self, out_point: OutPoint, with_data: bool) -> CellWithStatus {
        self.inner
            .lock()
            .get_live_cell(out_point, with_data)
            .call()
            .expect("rpc call get_live_cell")
    }

    pub fn get_tip_block_number(&self) -> CoreBlockNumber {
        self.inner
            .lock()
            .get_tip_block_number()
            .call()
            .expect("rpc call get_tip_block_number")
            .into()
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
            .get_epoch_by_number(number.into())
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

    pub fn get_banned_addresses(&self) -> Vec<BannedAddr> {
        self.inner
            .lock()
            .get_banned_addresses()
            .call()
            .expect("rpc call get_banned_addresses")
    }

    pub fn set_ban(
        &self,
        address: String,
        command: String,
        ban_time: Option<Timestamp>,
        absolute: Option<bool>,
        reason: Option<String>,
    ) {
        self.inner
            .lock()
            .set_ban(address, command, ban_time, absolute, reason)
            .call()
            .expect("rpc call set_ban")
    }

    pub fn get_block_template(
        &self,
        bytes_limit: Option<u64>,
        proposals_limit: Option<u64>,
        max_version: Option<CoreVersion>,
    ) -> BlockTemplate {
        let bytes_limit = bytes_limit.map(Into::into);
        let proposals_limit = proposals_limit.map(Into::into);
        let max_version = max_version.map(Into::into);
        self.inner
            .lock()
            .get_block_template(bytes_limit, proposals_limit, max_version)
            .call()
            .expect("rpc call get_block_template")
    }

    pub fn submit_block(&self, work_id: String, block: Block) -> JsonRpcResult<Byte32> {
        self.inner
            .lock()
            .submit_block(work_id, block)
            .call()
            .map(|x| x.pack())
    }

    pub fn get_blockchain_info(&self) -> ChainInfo {
        self.inner
            .lock()
            .get_blockchain_info()
            .call()
            .expect("rpc call get_blockchain_info")
    }

    pub fn send_transaction(&self, tx: Transaction) -> Byte32 {
        self.inner
            .lock()
            .send_transaction(tx)
            .call()
            .expect("rpc call send_transaction")
            .pack()
    }

    pub fn send_transaction_result(&self, tx: Transaction) -> JsonRpcResult<H256> {
        self.inner.lock().send_transaction(tx).call()
    }

    pub fn dry_run_transaction(&self, tx: Transaction) -> DryRunResult {
        self.inner
            .lock()
            .dry_run_transaction(tx)
            .call()
            .expect("rpc call dry_run_transaction")
    }

    pub fn broadcast_transaction(&self, tx: Transaction, cycles: Cycle) -> JsonRpcResult<H256> {
        self.inner.lock().broadcast_transaction(tx, cycles).call()
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

    pub fn process_block_without_verify(&self, block: Block) -> Option<Byte32> {
        self.inner
            .lock()
            .process_block_without_verify(block)
            .call()
            .expect("rpc call process_block_without verify")
            .map(|x| x.pack())
    }

    pub fn get_live_cells_by_lock_hash(
        &self,
        lock_hash: Byte32,
        page: u64,
        per_page: u64,
        reverse_order: Option<bool>,
    ) -> Vec<LiveCell> {
        self.inner()
            .lock()
            .get_live_cells_by_lock_hash(
                lock_hash.unpack(),
                page.into(),
                per_page.into(),
                reverse_order,
            )
            .call()
            .expect("rpc call get_live_cells_by_lock_hash")
    }

    pub fn get_transactions_by_lock_hash(
        &self,
        lock_hash: Byte32,
        page: u64,
        per_page: u64,
        reverse_order: Option<bool>,
    ) -> Vec<CellTransaction> {
        self.inner()
            .lock()
            .get_transactions_by_lock_hash(
                lock_hash.unpack(),
                page.into(),
                per_page.into(),
                reverse_order,
            )
            .call()
            .expect("rpc call get_transactions_by_lock_hash")
    }

    pub fn index_lock_hash(
        &self,
        lock_hash: Byte32,
        index_from: Option<CoreBlockNumber>,
    ) -> LockHashIndexState {
        self.inner()
            .lock()
            .index_lock_hash(lock_hash.unpack(), index_from.map(Into::into))
            .call()
            .expect("rpc call index_lock_hash")
    }

    pub fn deindex_lock_hash(&self, lock_hash: Byte32) {
        self.inner()
            .lock()
            .deindex_lock_hash(lock_hash.unpack())
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

    pub fn calculate_dao_maximum_withdraw(
        &self,
        out_point: OutPoint,
        hash: Byte32,
    ) -> CoreCapacity {
        self.inner()
            .lock()
            .calculate_dao_maximum_withdraw(out_point, hash.unpack())
            .call()
            .expect("rpc call calculate_dao_maximum_withdraw")
            .into()
    }

    pub fn get_cellbase_output_capacity_details(&self, hash: Byte32) -> BlockReward {
        self.inner()
            .lock()
            .get_cellbase_output_capacity_details(hash.unpack())
            .call()
            .expect("rpc call get_cellbase_output_capacity_details")
            .expect("get_cellbase_output_capacity_details return none")
    }
}

jsonrpc_client!(pub struct Inner {
    pub fn get_block(&mut self, _hash: H256) -> RpcRequest<Option<BlockView>>;
    pub fn get_fork_block(&mut self, _hash: H256) -> RpcRequest<Option<BlockView>>;
    pub fn get_block_by_number(&mut self, _number: BlockNumber) -> RpcRequest<Option<BlockView>>;
    pub fn get_header(&mut self, _hash: H256) -> RpcRequest<Option<HeaderView>>;
    pub fn get_header_by_number(&mut self, _number: BlockNumber) -> RpcRequest<Option<HeaderView>>;
    pub fn get_transaction(&mut self, _hash: H256) -> RpcRequest<Option<TransactionWithStatus>>;
    pub fn get_block_hash(&mut self, _number: BlockNumber) -> RpcRequest<Option<H256>>;
    pub fn get_tip_header(&mut self) -> RpcRequest<HeaderView>;
    pub fn get_cells_by_lock_hash(
        &mut self,
        _lock_hash: H256,
        _from: BlockNumber,
        _to: BlockNumber
    ) -> RpcRequest<Vec<CellOutputWithOutPoint>>;
    pub fn get_live_cell(&mut self, _out_point: OutPoint, _with_data: bool) -> RpcRequest<CellWithStatus>;
    pub fn get_tip_block_number(&mut self) -> RpcRequest<BlockNumber>;
    pub fn get_current_epoch(&mut self) -> RpcRequest<EpochView>;
    pub fn get_epoch_by_number(&mut self, number: EpochNumber) -> RpcRequest<Option<EpochView>>;

    pub fn local_node_info(&mut self) -> RpcRequest<Node>;
    pub fn get_peers(&mut self) -> RpcRequest<Vec<Node>>;
    pub fn get_banned_addresses(&mut self) -> RpcRequest<Vec<BannedAddr>>;
    pub fn set_ban(
        &mut self,
        address: String,
        command: String,
        ban_time: Option<Timestamp>,
        absolute: Option<bool>,
        reason: Option<String>
    ) -> RpcRequest<()>;

    pub fn get_block_template(
        &mut self,
        bytes_limit: Option<Uint64>,
        proposals_limit: Option<Uint64>,
        max_version: Option<Version>
    ) -> RpcRequest<BlockTemplate>;
    pub fn submit_block(&mut self, _work_id: String, _data: Block) -> RpcRequest<H256>;
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

    pub fn get_live_cells_by_lock_hash(&mut self, lock_hash: H256, page: Uint64, per_page: Uint64, reverse_order: Option<bool>) -> RpcRequest<Vec<LiveCell>>;
    pub fn get_transactions_by_lock_hash(&mut self, lock_hash: H256, page: Uint64, per_page: Uint64, reverse_order: Option<bool>) -> RpcRequest<Vec<CellTransaction>>;
    pub fn index_lock_hash(&mut self, lock_hash: H256, index_from: Option<BlockNumber>) -> RpcRequest<LockHashIndexState>;
    pub fn deindex_lock_hash(&mut self, lock_hash: H256) -> RpcRequest<()>;
    pub fn get_lock_hash_index_states(&mut self) -> RpcRequest<Vec<LockHashIndexState>>;
    pub fn calculate_dao_maximum_withdraw(&mut self, _out_point: OutPoint, _hash: H256) -> RpcRequest<Capacity>;
    pub fn get_cellbase_output_capacity_details(&mut self, _hash: H256) -> RpcRequest<Option<BlockReward>>;
    pub fn broadcast_transaction(&mut self, tx: Transaction, cycles: Cycle) -> RpcRequest<H256>;
});
