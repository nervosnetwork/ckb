mod id_generator;
#[macro_use]
mod macros;
mod error;

use ckb_error::AnyError;
use ckb_jsonrpc_types::{
    Alert, BannedAddr, Block, BlockEconomicState, BlockNumber, BlockTemplate, BlockView, Capacity,
    CellWithStatus, ChainInfo, DryRunResult, EpochNumber, EpochView, HeaderView, JsonBytes,
    LocalNode, OutPoint, RawTxPool, RemoteNode, Timestamp, Transaction, TransactionProof,
    TransactionWithStatus, TxPoolInfo, Uint32, Uint64, Version,
};
use ckb_types::core::{
    BlockNumber as CoreBlockNumber, Capacity as CoreCapacity, EpochNumber as CoreEpochNumber,
    Version as CoreVersion,
};
use ckb_types::{packed::Byte32, prelude::*, H256};
use lazy_static::lazy_static;

lazy_static! {
    pub static ref HTTP_CLIENT: reqwest::blocking::Client = reqwest::blocking::Client::builder()
        .timeout(::std::time::Duration::from_secs(30))
        .build()
        .expect("reqwest Client build");
}

pub struct RpcClient {
    inner: Inner,
}

impl RpcClient {
    pub fn new(uri: &str) -> Self {
        Self {
            inner: Inner::new(uri),
        }
    }

    pub fn inner(&self) -> &Inner {
        &self.inner
    }

    pub fn get_block(&self, hash: Byte32) -> Option<BlockView> {
        self.inner
            .get_block(hash.unpack())
            .expect("rpc call get_block")
    }

    pub fn get_fork_block(&self, hash: Byte32) -> Option<BlockView> {
        self.inner
            .get_fork_block(hash.unpack())
            .expect("rpc call get_fork_block")
    }

    pub fn get_block_by_number(&self, number: CoreBlockNumber) -> Option<BlockView> {
        self.inner
            .get_block_by_number(number.into())
            .expect("rpc call get_block_by_number")
    }

    pub fn get_header(&self, hash: Byte32) -> Option<HeaderView> {
        self.inner
            .get_header(hash.unpack())
            .expect("rpc call get_header")
    }

    pub fn get_header_by_number(&self, number: CoreBlockNumber) -> Option<HeaderView> {
        self.inner
            .get_header_by_number(number.into())
            .expect("rpc call get_header_by_number")
    }

    pub fn get_block_filter(&self, hash: Byte32) -> Option<JsonBytes> {
        self.inner
            .get_block_filter(hash.unpack())
            .expect("rpc call get_block_filter")
    }

    pub fn get_transaction(&self, hash: Byte32) -> Option<TransactionWithStatus> {
        // keep legacy mode
        self.inner
            .get_transaction(hash.unpack(), Some(0u32.into()))
            .expect("rpc call get_transaction")
    }

    pub fn get_transaction_with_verbosity(
        &self,
        hash: Byte32,
        verbosity: u32,
    ) -> Option<TransactionWithStatus> {
        self.inner
            .get_transaction(hash.unpack(), Some(verbosity.into()))
            .expect("rpc call get_transaction")
    }

    pub fn get_block_hash(&self, number: CoreBlockNumber) -> Option<Byte32> {
        self.inner
            .get_block_hash(number.into())
            .expect("rpc call get_block_hash")
            .map(|x| x.pack())
    }

    pub fn get_tip_header(&self) -> HeaderView {
        self.inner
            .get_tip_header()
            .expect("rpc call get_tip_header")
    }

    pub fn get_live_cell(&self, out_point: OutPoint, with_data: bool) -> CellWithStatus {
        self.inner
            .get_live_cell(out_point, with_data)
            .expect("rpc call get_live_cell")
    }

    pub fn get_tip_block_number(&self) -> CoreBlockNumber {
        self.inner
            .get_tip_block_number()
            .expect("rpc call get_tip_block_number")
            .into()
    }

    pub fn get_current_epoch(&self) -> EpochView {
        self.inner
            .get_current_epoch()
            .expect("rpc call get_current_epoch")
    }

    pub fn get_epoch_by_number(&self, number: CoreEpochNumber) -> Option<EpochView> {
        self.inner
            .get_epoch_by_number(number.into())
            .expect("rpc call get_epoch_by_number")
    }

    pub fn local_node_info(&self) -> LocalNode {
        self.inner
            .local_node_info()
            .expect("rpc call local_node_info")
    }

    pub fn get_peers(&self) -> Vec<RemoteNode> {
        self.inner.get_peers().expect("rpc call get_peers")
    }

    pub fn get_banned_addresses(&self) -> Vec<BannedAddr> {
        self.inner
            .get_banned_addresses()
            .expect("rpc call get_banned_addresses")
    }

    pub fn clear_banned_addresses(&self) {
        self.inner
            .clear_banned_addresses()
            .expect("rpc call clear_banned_addresses")
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
            .set_ban(address, command, ban_time, absolute, reason)
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
            .get_block_template(bytes_limit, proposals_limit, max_version)
            .expect("rpc call get_block_template")
    }

    pub fn submit_block(&self, work_id: String, block: Block) -> Result<Byte32, AnyError> {
        self.inner.submit_block(work_id, block).map(|x| x.pack())
    }

    pub fn get_blockchain_info(&self) -> ChainInfo {
        self.inner
            .get_blockchain_info()
            .expect("rpc call get_blockchain_info")
    }

    pub fn get_block_median_time(&self, block_hash: Byte32) -> Option<Timestamp> {
        self.inner
            .get_block_median_time(block_hash.unpack())
            .expect("rpc call get_block_median_time")
    }

    pub fn send_transaction(&self, tx: Transaction) -> Byte32 {
        self.send_transaction_result(tx)
            .expect("rpc call send_transaction")
            .pack()
    }

    pub fn send_transaction_result(&self, tx: Transaction) -> Result<H256, AnyError> {
        self.inner
            .send_transaction(tx, Some("passthrough".to_string()))
    }

    pub fn remove_transaction(&self, tx_hash: Byte32) -> bool {
        self.inner
            .remove_transaction(tx_hash.unpack())
            .expect("rpc call remove_transaction")
    }

    pub fn get_raw_tx_pool(&self, verbose: Option<bool>) -> RawTxPool {
        self.inner
            .get_raw_tx_pool(verbose)
            .expect("rpc call get_raw_tx_pool")
    }

    pub fn dry_run_transaction(&self, tx: Transaction) -> DryRunResult {
        self.inner
            .dry_run_transaction(tx)
            .expect("rpc call dry_run_transaction")
    }

    pub fn send_alert(&self, alert: Alert) {
        self.inner.send_alert(alert).expect("rpc call send_alert")
    }

    pub fn tx_pool_info(&self) -> TxPoolInfo {
        self.inner.tx_pool_info().expect("rpc call tx_pool_info")
    }

    pub fn add_node(&self, peer_id: String, address: String) {
        self.inner
            .add_node(peer_id, address)
            .expect("rpc call add_node");
    }

    pub fn remove_node(&self, peer_id: String) {
        self.inner
            .remove_node(peer_id)
            .expect("rpc call remove_node")
    }

    pub fn process_block_without_verify(&self, block: Block, broadcast: bool) -> Option<Byte32> {
        self.inner
            .process_block_without_verify(block, broadcast)
            .expect("rpc call process_block_without verify")
            .map(|x| x.pack())
    }

    pub fn truncate(&self, target_tip_hash: Byte32) {
        self.inner()
            .truncate(target_tip_hash.unpack())
            .expect("rpc call truncate")
    }

    pub fn generate_block(&self) -> Byte32 {
        self.inner()
            .generate_block()
            .expect("rpc call generate_block")
            .pack()
    }

    pub fn generate_block_with_template(&self, block_template: BlockTemplate) -> Byte32 {
        self.inner()
            .generate_block_with_template(block_template)
            .expect("rpc call generate_block_with_template")
            .pack()
    }

    pub fn calculate_dao_maximum_withdraw(
        &self,
        out_point: OutPoint,
        hash: Byte32,
    ) -> CoreCapacity {
        self.inner()
            .calculate_dao_maximum_withdraw(out_point, hash.unpack())
            .expect("rpc call calculate_dao_maximum_withdraw")
            .into()
    }

    pub fn get_block_economic_state(&self, hash: Byte32) -> Option<BlockEconomicState> {
        self.inner()
            .get_block_economic_state(hash.unpack())
            .expect("rpc call get_block_economic_state")
    }

    pub fn notify_transaction(&self, tx: Transaction) -> Byte32 {
        self.inner()
            .notify_transaction(tx)
            .expect("rpc call send_transaction")
            .pack()
    }

    pub fn tx_pool_ready(&self) -> bool {
        self.inner()
            .tx_pool_ready()
            .expect("rpc call tx_pool_ready")
    }
}

jsonrpc!(pub struct Inner {
    pub fn get_block(&self, _hash: H256) -> Option<BlockView>;
    pub fn get_fork_block(&self, _hash: H256) -> Option<BlockView>;
    pub fn get_block_by_number(&self, _number: BlockNumber) -> Option<BlockView>;
    pub fn get_header(&self, _hash: H256) -> Option<HeaderView>;
    pub fn get_header_by_number(&self, _number: BlockNumber) -> Option<HeaderView>;
    pub fn get_block_filter(&self, _hash: H256) -> Option<JsonBytes>;
    pub fn get_transaction(&self, _hash: H256, verbosity: Option<Uint32>) -> Option<TransactionWithStatus>;
    pub fn get_block_hash(&self, _number: BlockNumber) -> Option<H256>;
    pub fn get_tip_header(&self) -> HeaderView;
    pub fn get_live_cell(&self, _out_point: OutPoint, _with_data: bool) -> CellWithStatus;
    pub fn get_tip_block_number(&self) -> BlockNumber;
    pub fn get_current_epoch(&self) -> EpochView;
    pub fn get_epoch_by_number(&self, number: EpochNumber) -> Option<EpochView>;

    pub fn local_node_info(&self) -> LocalNode;
    pub fn get_peers(&self) -> Vec<RemoteNode>;
    pub fn get_banned_addresses(&self) -> Vec<BannedAddr>;
    pub fn set_ban(
        &self,
        address: String,
        command: String,
        ban_time: Option<Timestamp>,
        absolute: Option<bool>,
        reason: Option<String>
    ) -> ();
    pub fn clear_banned_addresses(&self) -> ();

    pub fn get_block_template(
        &self,
        bytes_limit: Option<Uint64>,
        proposals_limit: Option<Uint64>,
        max_version: Option<Version>
    ) -> BlockTemplate;
    pub fn submit_block(&self, _work_id: String, _data: Block) -> H256;
    pub fn get_blockchain_info(&self) -> ChainInfo;
    pub fn get_block_median_time(&self, block_hash: H256) -> Option<Timestamp>;
    pub fn dry_run_transaction(&self, _tx: Transaction) -> DryRunResult;
    pub fn send_transaction(&self, tx: Transaction, outputs_validator: Option<String>) -> H256;
    pub fn remove_transaction(&self, tx_hash: H256) -> bool;
    pub fn tx_pool_info(&self) -> TxPoolInfo;
    pub fn get_raw_tx_pool(&self, verbose: Option<bool>) -> RawTxPool;

    pub fn send_alert(&self, alert: Alert) -> ();

    pub fn add_node(&self, peer_id: String, address: String) -> ();
    pub fn remove_node(&self, peer_id: String) -> ();
    pub fn process_block_without_verify(&self, _data: Block, broadcast: bool) -> Option<H256>;
    pub fn truncate(&self, target_tip_hash: H256) -> ();
    pub fn generate_block(&self) -> H256;
    pub fn generate_block_with_template(&self, block_template: BlockTemplate) -> H256;

    pub fn calculate_dao_maximum_withdraw(&self, _out_point: OutPoint, _hash: H256) -> Capacity;
    pub fn get_block_economic_state(&self, _hash: H256) -> Option<BlockEconomicState>;
    pub fn get_transaction_proof(&self, tx_hashes: Vec<H256>, block_hash: Option<H256>) -> TransactionProof;
    pub fn verify_transaction_proof(&self, tx_proof: TransactionProof) -> Vec<H256>;
    pub fn notify_transaction(&self, tx: Transaction) -> H256;
    pub fn tx_pool_ready(&self) -> bool;
});
