mod id_generator;
#[macro_use]
mod macros;
mod error;

use ckb_error::AnyError;
use ckb_jsonrpc_types::{
    BannedAddr, Block, BlockEconomicState, BlockFilter, BlockNumber, BlockTemplate, BlockView,
    Capacity, CellWithStatus, ChainInfo, EpochNumber, EpochView, EstimateCycles, HeaderView,
    LocalNode, OutPoint, PoolTxDetailInfo, RawTxPool, RemoteNode, SyncState, Timestamp,
    Transaction, TransactionAndWitnessProof, TransactionProof, TransactionWithStatusResponse,
    TxPoolInfo, Uint32, Uint64, Version,
};
use ckb_types::core::{
    BlockNumber as CoreBlockNumber, Capacity as CoreCapacity, EpochNumber as CoreEpochNumber,
    Version as CoreVersion,
};
use ckb_types::{H256, packed::Byte32};

pub static HTTP_CLIENT: std::sync::LazyLock<reqwest::blocking::Client> =
    std::sync::LazyLock::new(|| {
        reqwest::blocking::Client::builder()
            // Integration RPC endpoints are always loopback nodes started by this
            // runner; disable system/environment proxy discovery so localhost
            // traffic is never routed through an external proxy.
            .no_proxy()
            // Evict idle keep-alive sockets promptly so restarted/stopped nodes
            // rarely leave stale connections in the shared client pool.
            .pool_idle_timeout(::std::time::Duration::from_secs(5))
            .timeout(::std::time::Duration::from_secs(30))
            .build()
            .expect("reqwest Client build")
    });

pub(crate) fn is_read_only_rpc_method(method: &str) -> bool {
    method.starts_with("get_")
        || matches!(
            method,
            "local_node_info" | "tx_pool_info" | "tx_pool_ready" | "sync_state" | "estimate_cycles"
        )
}

fn is_transient_stale_connection_error(err: &reqwest::Error) -> bool {
    // Do not retry timeouts or connection-refused errors: startup polling relies
    // on immediate ConnectionRefused feedback to detect crashed child processes.
    if err.is_timeout() || err.is_connect() {
        return false;
    }
    if err.is_request() || err.is_body() {
        return true;
    }
    let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(err);
    while let Some(cause) = source {
        if let Some(io_err) = cause.downcast_ref::<std::io::Error>()
            && matches!(
                io_err.kind(),
                std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::BrokenPipe
            )
        {
            return true;
        }
        source = cause.source();
    }
    false
}

pub(crate) fn send_jsonrpc_request(
    client: &reqwest::blocking::Client,
    url: &reqwest::Url,
    method: &str,
    req_json: &serde_json::Map<String, serde_json::Value>,
) -> Result<jsonrpc_core::response::Output, AnyError> {
    let max_attempts = if is_read_only_rpc_method(method) {
        3
    } else {
        1
    };
    let mut attempt = 0usize;
    loop {
        attempt += 1;
        match client.post(url.clone()).json(req_json).send() {
            Ok(resp) => match resp.bytes() {
                Ok(body) => {
                    // Full HTTP body read succeeded; parse JSON without retrying
                    // malformed payloads.
                    return serde_json::from_slice::<jsonrpc_core::response::Output>(&body)
                        .map_err(Into::into);
                }
                Err(err) if attempt < max_attempts && is_transient_stale_connection_error(&err) => {
                    std::thread::sleep(std::time::Duration::from_millis(25 * attempt as u64));
                    continue;
                }
                Err(err) => return Err(err.into()),
            },
            Err(err) if attempt < max_attempts && is_transient_stale_connection_error(&err) => {
                std::thread::sleep(std::time::Duration::from_millis(25 * attempt as u64));
                continue;
            }
            Err(err) => return Err(err.into()),
        }
    }
}

static DIAGNOSTIC_HTTP_CLIENT: std::sync::LazyLock<reqwest::blocking::Client> =
    std::sync::LazyLock::new(|| {
        reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(::std::time::Duration::from_secs(2))
            .build()
            .expect("reqwest diagnostic Client build")
    });

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

    /// Best-effort short-timeout (2s) JSON-RPC call used when capturing failure
    /// diagnostics so a hung or unresponsive node never stalls failure reporting.
    pub fn diagnostic_call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let req = serde_json::json!({
            "id": self.inner.id_generator.next(),
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let resp = DIAGNOSTIC_HTTP_CLIENT
            .post(self.inner.url.clone())
            .json(&req)
            .send()
            .map_err(|err| format!("http error: {err}"))?;
        let output = resp
            .json::<jsonrpc_core::response::Output>()
            .map_err(|err| format!("json decode error: {err}"))?;
        match output {
            jsonrpc_core::response::Output::Success(success) => Ok(success.result),
            jsonrpc_core::response::Output::Failure(failure) => {
                Err(format!("rpc error: {:?}", failure.error))
            }
        }
    }

    pub fn get_block(&self, hash: Byte32) -> Option<BlockView> {
        self.inner
            .get_block(hash.into())
            .expect("rpc call get_block")
    }

    pub fn get_fork_block(&self, hash: Byte32) -> Option<BlockView> {
        self.inner
            .get_fork_block(hash.into())
            .expect("rpc call get_fork_block")
    }

    pub fn get_block_by_number(&self, number: CoreBlockNumber) -> Option<BlockView> {
        self.inner
            .get_block_by_number(number.into())
            .expect("rpc call get_block_by_number")
    }

    pub fn get_header(&self, hash: Byte32) -> Option<HeaderView> {
        self.inner
            .get_header(hash.into())
            .expect("rpc call get_header")
    }

    pub fn get_header_by_number(&self, number: CoreBlockNumber) -> Option<HeaderView> {
        self.inner
            .get_header_by_number(number.into())
            .expect("rpc call get_header_by_number")
    }

    pub fn get_block_filter(&self, hash: Byte32) -> Option<BlockFilter> {
        self.inner
            .get_block_filter(hash.into())
            .expect("rpc call get_block_filter")
    }

    pub fn get_transaction(&self, hash: Byte32) -> TransactionWithStatusResponse {
        self.get_transaction_with_verbosity(hash, 2)
    }

    pub fn get_transaction_with_verbosity(
        &self,
        hash: Byte32,
        verbosity: u32,
    ) -> TransactionWithStatusResponse {
        self.inner
            .get_transaction(hash.into(), Some(verbosity.into()), None)
            .expect("rpc call get_transaction")
    }

    pub fn get_pool_tx_detail_info(&self, hash: Byte32) -> PoolTxDetailInfo {
        self.inner
            .get_pool_tx_detail_info(hash.into())
            .expect("rpc call get_transaction_tx_pool_details")
    }

    pub fn get_block_hash(&self, number: CoreBlockNumber) -> Option<Byte32> {
        self.inner
            .get_block_hash(number.into())
            .expect("rpc call get_block_hash")
            .map(|x| x.into())
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

    pub fn sync_state(&self) -> SyncState {
        self.inner.sync_state().expect("rpc call sync_state")
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

    pub fn wait_rpc_ready(&self) {
        self.wait_rpc_ready_internal(|| {
            panic!("wait rpc ready timeout");
        });
    }

    pub fn wait_rpc_ready_internal<F>(&self, fail: F) -> bool
    where
        F: Fn(),
    {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut attempt = 0u32;
        while self.inner.get_tip_block_number().is_err() {
            let now = std::time::Instant::now();
            if now >= deadline {
                fail();
                return false;
            }
            let sleep_dur =
                crate::utils::adaptive_poll_interval(attempt, 10, 50).min(deadline - now);
            std::thread::sleep(sleep_dur);
            attempt = attempt.saturating_add(1);
        }
        true
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
        self.inner.submit_block(work_id, block).map(|x| x.into())
    }

    pub fn get_blockchain_info(&self) -> ChainInfo {
        self.inner
            .get_blockchain_info()
            .expect("rpc call get_blockchain_info")
    }

    pub fn get_block_median_time(&self, block_hash: Byte32) -> Option<Timestamp> {
        self.inner
            .get_block_median_time(block_hash.into())
            .expect("rpc call get_block_median_time")
    }

    pub fn send_transaction(&self, tx: Transaction) -> Byte32 {
        match self.send_transaction_result(tx.clone()) {
            Ok(hash) => hash.into(),
            Err(err) => {
                ckb_logger::debug!("failed to send transaction {:?}", tx);
                panic!("rpc call send_transaction: {}", err);
            }
        }
    }

    pub fn send_transaction_result(&self, tx: Transaction) -> Result<H256, AnyError> {
        self.inner
            .send_transaction(tx, Some("passthrough".to_string()))
    }

    pub fn remove_transaction(&self, tx_hash: Byte32) -> bool {
        self.inner
            .remove_transaction(tx_hash.into())
            .expect("rpc call remove_transaction")
    }

    pub fn get_raw_tx_pool(&self, verbose: Option<bool>) -> RawTxPool {
        self.inner
            .get_raw_tx_pool(verbose)
            .expect("rpc call get_raw_tx_pool")
    }

    pub fn estimate_cycles(&self, tx: Transaction) -> EstimateCycles {
        self.inner
            .estimate_cycles(tx)
            .expect("rpc call estimate_cycles")
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
            .map(|x| x.into())
    }

    pub fn truncate(&self, target_tip_hash: Byte32) {
        self.inner()
            .truncate(target_tip_hash.into())
            .expect("rpc call truncate")
    }

    pub fn generate_block(&self) -> Byte32 {
        self.inner()
            .generate_block()
            .expect("rpc call generate_block")
            .into()
    }

    pub fn generate_block_with_template(&self, block_template: BlockTemplate) -> Byte32 {
        self.inner()
            .generate_block_with_template(block_template)
            .expect("rpc call generate_block_with_template")
            .into()
    }

    pub fn calculate_dao_maximum_withdraw(
        &self,
        out_point: OutPoint,
        hash: Byte32,
    ) -> CoreCapacity {
        self.inner()
            .calculate_dao_maximum_withdraw(out_point, hash.into())
            .expect("rpc call calculate_dao_maximum_withdraw")
            .into()
    }

    pub fn get_block_economic_state(&self, hash: Byte32) -> Option<BlockEconomicState> {
        self.inner()
            .get_block_economic_state(hash.into())
            .expect("rpc call get_block_economic_state")
    }

    pub fn notify_transaction(&self, tx: Transaction) -> Byte32 {
        self.inner()
            .notify_transaction(tx)
            .expect("rpc call send_transaction")
            .into()
    }

    pub fn tx_pool_ready(&self) -> bool {
        self.inner()
            .tx_pool_ready()
            .expect("rpc call tx_pool_ready")
    }
}

jsonrpc!(
    pub struct Inner {
    pub fn get_block(&self, _hash: H256) -> Option<BlockView>;
    pub fn get_fork_block(&self, _hash: H256) -> Option<BlockView>;
    pub fn get_block_by_number(&self, _number: BlockNumber) -> Option<BlockView>;
    pub fn get_header(&self, _hash: H256) -> Option<HeaderView>;
    pub fn get_header_by_number(&self, _number: BlockNumber) -> Option<HeaderView>;
    pub fn get_block_filter(&self, _hash: H256) -> Option<BlockFilter>;
    pub fn get_transaction(&self, _hash: H256, verbosity: Option<Uint32>, only_commited: Option<bool>) -> TransactionWithStatusResponse;
    pub fn get_block_hash(&self, _number: BlockNumber) -> Option<H256>;
    pub fn get_tip_header(&self) -> HeaderView;
    pub fn get_live_cell(&self, _out_point: OutPoint, _with_data: bool) -> CellWithStatus;
    pub fn get_tip_block_number(&self) -> BlockNumber;
    pub fn get_current_epoch(&self) -> EpochView;
    pub fn get_epoch_by_number(&self, number: EpochNumber) -> Option<EpochView>;

    pub fn sync_state(&self) -> SyncState;
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
    pub fn estimate_cycles(&self, _tx: Transaction) -> EstimateCycles;
    pub fn send_transaction(&self, tx: Transaction, outputs_validator: Option<String>) -> H256;
    pub fn remove_transaction(&self, tx_hash: H256) -> bool;
    pub fn tx_pool_info(&self) -> TxPoolInfo;
    pub fn get_raw_tx_pool(&self, verbose: Option<bool>) -> RawTxPool;

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
    pub fn get_transaction_and_witness_proof(&self, tx_hashes: Vec<H256>, block_hash: Option<H256>) -> TransactionAndWitnessProof;
    pub fn verify_transaction_and_witness_proof(&self, tx_proof: TransactionAndWitnessProof) -> Vec<H256>;
    pub fn notify_transaction(&self, tx: Transaction) -> H256;
    pub fn tx_pool_ready(&self) -> bool;
    pub fn get_pool_tx_detail_info(&self, _hash: H256) -> PoolTxDetailInfo;
});

#[cfg(test)]
mod tests {
    use super::{HTTP_CLIENT, is_read_only_rpc_method, send_jsonrpc_request};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn make_req(method: &str) -> serde_json::Map<String, serde_json::Value> {
        let mut req = serde_json::Map::new();
        req.insert("id".to_owned(), serde_json::json!(1));
        req.insert("jsonrpc".to_owned(), serde_json::json!("2.0"));
        req.insert("method".to_owned(), serde_json::json!(method));
        req.insert("params".to_owned(), serde_json::Value::Null);
        req
    }

    fn spawn_stub_http_server(
        responses: Vec<&'static [u8]>,
    ) -> (reqwest::Url, Arc<AtomicUsize>, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_clone = Arc::clone(&hits);
        let handle = std::thread::spawn(move || {
            for payload in responses {
                let (mut stream, _) = listener.accept().expect("accept");
                hits_clone.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(payload);
                let _ = stream.flush();
            }
        });
        let url = reqwest::Url::parse(&format!("http://{addr}/")).unwrap();
        (url, hits, handle)
    }

    #[test]
    fn read_only_rpc_method_classification() {
        for method in [
            "get_tip_header",
            "get_peers",
            "get_transaction",
            "local_node_info",
            "tx_pool_info",
            "tx_pool_ready",
            "sync_state",
            "estimate_cycles",
        ] {
            assert!(
                is_read_only_rpc_method(method),
                "{method} should be classified as read-only"
            );
        }
        for method in [
            "set_ban",
            "add_node",
            "remove_node",
            "clear_banned_addresses",
            "ping_peers",
            "send_transaction",
            "submit_block",
            "process_block_without_verify",
            "remove_transaction",
            "truncate",
            "generate_block",
            "notify_transaction",
        ] {
            assert!(
                !is_read_only_rpc_method(method),
                "{method} must not be retried automatically"
            );
        }
    }

    #[test]
    fn read_only_rpc_retries_truncated_body_eof_but_not_invalid_json_or_writes() {
        let truncated: &'static [u8] =
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{\"jsonrpc\":\"2.0\",";
        let truncated_chunked: &'static [u8] =
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n20\r\n{\"jsonrpc\":\"2.0\",";
        let valid_ok: &'static [u8] =
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 42\r\nConnection: close\r\n\r\n{\"jsonrpc\":\"2.0\",\"result\":\"0x1\",\"id\":1}   ";
        let invalid_json: &'static [u8] =
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\nnot-a-json!";

        // 1. Read-only RPC retries Content-Length truncated body EOF and succeeds on attempt 2.
        let (url, hits, handle) = spawn_stub_http_server(vec![truncated, valid_ok]);
        let res = send_jsonrpc_request(
            &HTTP_CLIENT,
            &url,
            "get_tip_block_number",
            &make_req("get_tip_block_number"),
        );
        handle.join().unwrap();
        assert!(
            res.is_ok(),
            "read-only RPC should retry truncated EOF: {res:?}"
        );
        assert_eq!(hits.load(Ordering::SeqCst), 2);

        // 2. Read-only RPC retries chunked truncated body EOF and succeeds on attempt 2.
        let (url, hits, handle) = spawn_stub_http_server(vec![truncated_chunked, valid_ok]);
        let res = send_jsonrpc_request(
            &HTTP_CLIENT,
            &url,
            "get_tip_block_number",
            &make_req("get_tip_block_number"),
        );
        handle.join().unwrap();
        assert!(
            res.is_ok(),
            "read-only RPC should retry truncated chunked EOF: {res:?}"
        );
        assert_eq!(hits.load(Ordering::SeqCst), 2);

        // 3. Write RPC must NOT retry truncated body EOF (sent once only).
        let (url, hits, handle) = spawn_stub_http_server(vec![truncated]);
        let res = send_jsonrpc_request(&HTTP_CLIENT, &url, "set_ban", &make_req("set_ban"));
        handle.join().unwrap();
        assert!(
            res.is_err(),
            "write RPC must fail immediately without retry"
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        // 4. Complete but invalid JSON must NOT retry even for read-only RPC.
        let (url, hits, handle) = spawn_stub_http_server(vec![invalid_json]);
        let res = send_jsonrpc_request(
            &HTTP_CLIENT,
            &url,
            "get_tip_block_number",
            &make_req("get_tip_block_number"),
        );
        handle.join().unwrap();
        assert!(
            res.is_err(),
            "malformed complete JSON must fail immediately"
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }
}
