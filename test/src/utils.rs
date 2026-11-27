use crate::global::{
    PORT_AUTO_WRAP, PORT_COUNTER, PORT_RANGE_END, PORT_RANGE_LEN, PORT_RANGE_START,
};
use crate::util::check::is_transaction_committed;
use crate::{Node, TXOSet};
use ckb_network::bytes::Bytes;
use ckb_types::{
    core::{BlockNumber, BlockView, EpochNumberWithFraction, HeaderView, TransactionView},
    packed::{
        BlockFilterMessage, BlockTransactions, Byte32, CompactBlock, GetBlocks, RelayMessage,
        RelayTransaction, RelayTransactionHashes, RelayTransactions, SendBlock, SendHeaders,
        SyncMessage,
    },
    prelude::*,
};
use core::sync::atomic::Ordering::SeqCst;
use std::cell::RefCell;
use std::convert::Into;
use std::env;
use std::fs::{self, read_to_string};
use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;

pub const FLAG_SINCE_RELATIVE: u64 =
    0b1000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000;
pub const FLAG_SINCE_BLOCK_NUMBER: u64 =
    0b000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000;
pub const FLAG_SINCE_EPOCH_NUMBER: u64 =
    0b010_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000;
pub const FLAG_SINCE_TIMESTAMP: u64 =
    0b100_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000;

// Build compact block based on core block, and specific prefilled indices
pub fn build_compact_block_with_prefilled(block: &BlockView, prefilled: Vec<usize>) -> Bytes {
    let prefilled = prefilled.into_iter().collect();
    let compact_block = CompactBlock::build_from_block(block, &prefilled);

    RelayMessage::new_builder()
        .set(compact_block)
        .build()
        .as_bytes()
}

// Build compact block based on core block
pub fn build_compact_block(block: &BlockView) -> Bytes {
    build_compact_block_with_prefilled(block, Vec::new())
}

pub fn build_block_transactions(block: &BlockView) -> Bytes {
    // compact block has always prefilled cellbase
    let block_txs = BlockTransactions::new_builder()
        .block_hash(block.header().hash())
        .transactions(
            block
                .transactions()
                .into_iter()
                .map(|view| view.data())
                .skip(1)
                .collect::<Vec<_>>(),
        )
        .build();

    RelayMessage::new_builder()
        .set(block_txs)
        .build()
        .as_bytes()
}

pub fn build_header(header: &HeaderView) -> Bytes {
    build_headers(std::slice::from_ref(header))
}

pub fn build_headers(headers: &[HeaderView]) -> Bytes {
    let send_headers = SendHeaders::new_builder()
        .headers(headers.iter().map(|view| view.data()).collect::<Vec<_>>())
        .build();

    SyncMessage::new_builder()
        .set(send_headers)
        .build()
        .as_bytes()
}

pub fn build_block(block: &BlockView) -> Bytes {
    SyncMessage::new_builder()
        .set(SendBlock::new_builder().block(block.data()).build())
        .build()
        .as_bytes()
}

pub fn build_get_blocks(hashes: &[Byte32]) -> Bytes {
    let get_blocks = GetBlocks::new_builder()
        .block_hashes(hashes.iter().map(ToOwned::to_owned).collect::<Vec<_>>())
        .build();

    SyncMessage::new_builder()
        .set(get_blocks)
        .build()
        .as_bytes()
}

pub fn build_relay_txs(transactions: &[(TransactionView, u64)]) -> Bytes {
    let transactions = transactions
        .iter()
        .map(|(tx, cycles)| {
            RelayTransaction::new_builder()
                .cycles(cycles)
                .transaction(tx.data())
                .build()
        })
        .collect::<Vec<_>>();
    let txs = RelayTransactions::new_builder()
        .transactions(transactions)
        .build();

    RelayMessage::new_builder().set(txs).build().as_bytes()
}

pub fn build_relay_tx_hashes(hashes: &[Byte32]) -> Bytes {
    let content = RelayTransactionHashes::new_builder()
        .tx_hashes(hashes.iter().map(ToOwned::to_owned).collect::<Vec<_>>())
        .build();

    RelayMessage::new_builder().set(content).build().as_bytes()
}

pub(crate) fn adaptive_poll_interval(attempt: u32, initial_ms: u64, max_ms: u64) -> Duration {
    let shift = attempt.min(16);
    let ms = initial_ms.saturating_mul(1u64 << shift).min(max_ms);
    Duration::from_millis(ms)
}

pub fn wait_until<F>(secs: u64, mut f: F) -> bool
where
    F: FnMut() -> bool,
{
    let timeout = tweaked_duration(secs);
    let deadline = Instant::now() + timeout;
    let mut attempt = 0u32;
    loop {
        if f() {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        let sleep_dur = adaptive_poll_interval(attempt, 10, 100).min(deadline - now);
        thread::sleep(sleep_dur);
        attempt = attempt.saturating_add(1);
    }
}

pub fn sleep(secs: u64) {
    thread::sleep(tweaked_duration(secs));
}

pub fn tweaked_duration(secs: u64) -> Duration {
    let sec_coefficient = env::var("CKB_TEST_SEC_COEFFICIENT")
        .unwrap_or_default()
        .parse()
        .unwrap_or(1.0);
    Duration::from_secs((secs as f64 * sec_coefficient) as u64)
}

pub fn since_from_relative_block_number(block_number: BlockNumber) -> u64 {
    FLAG_SINCE_RELATIVE | FLAG_SINCE_BLOCK_NUMBER | block_number
}

pub fn since_from_absolute_block_number(block_number: BlockNumber) -> u64 {
    FLAG_SINCE_BLOCK_NUMBER | block_number
}

pub fn since_from_relative_epoch_number(epoch_number: EpochNumberWithFraction) -> u64 {
    FLAG_SINCE_RELATIVE | FLAG_SINCE_EPOCH_NUMBER | epoch_number.full_value()
}

pub fn since_from_absolute_epoch_number(epoch_number: EpochNumberWithFraction) -> u64 {
    FLAG_SINCE_EPOCH_NUMBER | epoch_number.full_value()
}

pub fn since_from_relative_timestamp(timestamp: u64) -> u64 {
    FLAG_SINCE_RELATIVE | FLAG_SINCE_TIMESTAMP | timestamp
}

pub fn since_from_absolute_timestamp(timestamp: u64) -> u64 {
    FLAG_SINCE_TIMESTAMP | timestamp
}

pub fn assert_send_transaction_fail(node: &Node, transaction: &TransactionView, message: &str) {
    node.wait_for_tx_pool();
    let result = node
        .rpc_client()
        .send_transaction_result(transaction.data().into());
    assert!(
        result.is_err(),
        "expect error \"{message}\" but got \"Ok(())\""
    );
    let error = result.expect_err(&format!("transaction is invalid since {message}"));
    let error_string = error.to_string();
    assert!(
        error_string.contains(message),
        "expect error \"{message}\" but got \"{error_string}\""
    );
}

pub fn assert_send_transaction_ok(node: &Node, transaction: &TransactionView) {
    node.wait_for_tx_pool();
    let result = node
        .rpc_client()
        .send_transaction_result(transaction.data().into());
    assert!(result.is_ok(), "result: {:?}", result.unwrap_err());
}

thread_local! {
    static SPEC_RUN_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// RAII guard that scopes `TempPathBuf::new(...)` calls on the current thread
/// into subdirectories of a single per-spec directory.
pub(crate) struct SpecRunDirGuard {
    previous: Option<PathBuf>,
}

impl SpecRunDirGuard {
    pub(crate) fn enter(dir: PathBuf) -> Self {
        let previous = SPEC_RUN_DIR.with(|slot| slot.replace(Some(dir)));
        Self { previous }
    }

    pub(crate) fn current_dir() -> Option<PathBuf> {
        SPEC_RUN_DIR.with(|slot| slot.borrow().clone())
    }
}

impl Drop for SpecRunDirGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        SPEC_RUN_DIR.with(|slot| {
            *slot.borrow_mut() = previous;
        });
    }
}

fn sanitize_path_component(raw: &str) -> String {
    let sanitized: String = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('-');
    if trimmed.is_empty() {
        "node".to_string()
    } else {
        trimmed.to_string()
    }
}

#[derive(Debug, Clone)]
pub struct TempPathBuf {
    path: PathBuf,
    // `Some` for owned root tempdirs; `None` for child subdirs inside a spec's
    // root tempdir (whose lifecycle is owned by that root `TempPathBuf`).
    _tempdir: Option<Arc<TempDir>>,
}

impl TempPathBuf {
    /// Create a root temporary directory for a single spec execution.
    pub fn new_spec_dir(spec_label: &str) -> std::io::Result<Self> {
        let mut builder = tempfile::Builder::new();
        let label = sanitize_path_component(spec_label);
        let prefix = ["ckb-it", label.as_str(), ""].join("-");
        builder.prefix(&prefix);
        let tempdir = if let Ok(val) = env::var("CKB_INTEGRATION_TEST_TMP") {
            builder.tempdir_in(val)
        } else {
            builder.tempdir()
        }?;
        let path = tempdir.path().to_owned();
        Ok(Self {
            path,
            _tempdir: Some(Arc::new(tempdir)),
        })
    }

    /// Return a path located under the active spec run directory (if any),
    /// or a standalone random path under temp_dir.
    pub fn new(case_name: &str, suffix: &str) -> Self {
        if let Some(spec_dir) = SPEC_RUN_DIR.with(|slot| slot.borrow().clone()) {
            let base = sanitize_path_component(suffix);
            let mut candidate = spec_dir.join(&base);
            let mut counter = 1usize;
            loop {
                match fs::create_dir(&candidate) {
                    Ok(()) => {
                        return Self {
                            path: candidate,
                            _tempdir: None,
                        };
                    }
                    Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                        candidate = spec_dir.join(format!("{base}-{counter}"));
                        counter += 1;
                    }
                    Err(err) => {
                        panic!(
                            "create node dir {} inside spec dir failed: {}",
                            candidate.display(),
                            err
                        );
                    }
                }
            }
        }

        let mut builder = tempfile::Builder::new();
        let prefix = ["ckb-it", case_name, suffix, ""].join("-");
        builder.prefix(&prefix);
        let tempdir = if let Ok(val) = env::var("CKB_INTEGRATION_TEST_TMP") {
            builder.tempdir_in(val)
        } else {
            builder.tempdir()
        }
        .expect("create tempdir failed");
        let path = tempdir.path().to_owned();
        Self {
            path,
            _tempdir: Some(Arc::new(tempdir)),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns `true` if this `TempPathBuf` owns a standalone root tempdir.
    pub fn is_owned_root(&self) -> bool {
        self._tempdir.is_some()
    }

    /// Consumes the `TempPathBuf` and returns the underlying path without
    /// deleting the temporary directory.
    pub fn leak(self) -> PathBuf {
        let mut this = std::mem::ManuallyDrop::new(self);
        std::mem::take(&mut this.path)
    }
}

impl Deref for TempPathBuf {
    type Target = PathBuf;
    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

impl AsRef<Path> for TempPathBuf {
    fn as_ref(&self) -> &Path {
        self.path.as_ref()
    }
}

/// Generate new blocks and explode these cellbases into `n` live cells
pub fn generate_utxo_set(node: &Node, n: usize) -> TXOSet {
    // Ensure all the cellbases will be used later are already mature.
    let cellbase_maturity = node.consensus().cellbase_maturity();
    node.mine(cellbase_maturity.index());

    // Explode these mature cellbases into multiple cells
    let mut n_outputs = 0;
    let mut txs = Vec::new();
    while n > n_outputs {
        node.mine(1);
        let mature_number = node.get_tip_block_number() - cellbase_maturity.index();
        let mature_block = node.get_block_by_number(mature_number);
        let mature_cellbase = mature_block.transaction(0).unwrap();
        if mature_cellbase.outputs().is_empty() {
            continue;
        }

        let mature_utxos: TXOSet = TXOSet::from(&mature_cellbase);
        let tx = mature_utxos.boom(vec![node.always_success_cell_dep()]);
        n_outputs += tx.outputs().len();
        txs.push(tx);
    }

    // Ensure all the transactions were committed
    txs.iter().for_each(|tx| {
        node.submit_transaction(tx);
    });
    while txs.iter().any(|tx| !is_transaction_committed(node, tx)) {
        node.mine(node.consensus().finalization_delay_length());
    }

    let mut utxos = TXOSet::default();
    txs.iter()
        .for_each(|tx| utxos.extend(Into::<TXOSet>::into(tx)));
    utxos.truncate(n);
    node.wait_for_tx_pool();
    utxos
}

/// Return a blank block with additional committed transactions
pub fn commit(node: &Node, committed: &[&TransactionView]) -> BlockView {
    let committed = committed
        .iter()
        .map(|t| t.to_owned().to_owned())
        .collect::<Vec<_>>();
    blank(node)
        .as_advanced_builder()
        .transactions(committed)
        .build()
}

/// Return a blank block with additional proposed transactions
pub fn propose(node: &Node, proposals: &[&TransactionView]) -> BlockView {
    let proposals = proposals.iter().map(|tx| tx.proposal_short_id());
    blank(node)
        .as_advanced_builder()
        .proposals(proposals)
        .build()
}

/// Return a block with `proposals = [], transactions = [cellbase], uncles = []`
pub fn blank(node: &Node) -> BlockView {
    let example = node.new_block(None, None, None);
    example
        .as_advanced_builder()
        .set_proposals(vec![])
        .set_transactions(vec![example.transaction(0).unwrap()]) // cellbase
        .set_uncles(vec![])
        .build()
}

// grep "panicked at" $node_log_path
pub fn nodes_panicked(node_log_paths: &[PathBuf]) -> bool {
    node_log_paths
        .iter()
        .any(|log_path| match read_to_string(log_path) {
            Ok(content) => content.contains("panicked at"),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
            Err(err) => {
                panic!(
                    "failed to read node's log {}, error: {:?}",
                    log_path.display(),
                    err
                )
            }
        })
}

pub fn now_ms() -> u64 {
    let start = SystemTime::now();
    let since_the_epoch = start
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards");
    since_the_epoch.as_millis() as u64
}

fn next_candidate_port(counter: &std::sync::atomic::AtomicU16, auto_wrap: bool) -> u16 {
    counter
        .fetch_update(SeqCst, SeqCst, |cur| {
            let next = cur.wrapping_add(1);
            let wrapped = if auto_wrap {
                if !(PORT_RANGE_START..PORT_RANGE_END).contains(&next) {
                    PORT_RANGE_START
                } else {
                    next
                }
            } else if next < 1024 {
                1024
            } else {
                next
            };
            Some(wrapped)
        })
        .expect("PORT_COUNTER update always succeeds")
}

pub fn find_available_port() -> u16 {
    let auto_wrap = PORT_AUTO_WRAP.load(SeqCst);
    for _ in 0..PORT_RANGE_LEN {
        let port = next_candidate_port(&PORT_COUNTER, auto_wrap);
        if port < 1024 {
            continue;
        }
        let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
        if TcpListener::bind(address).is_ok() {
            return port;
        }
    }
    panic!("failed to allocate available port")
}

pub fn message_name(data: &Bytes) -> String {
    if let Ok(message) = SyncMessage::from_compatible_slice(data) {
        message.to_enum().item_name().to_string()
    } else if let Ok(message) = RelayMessage::from_compatible_slice(data) {
        message.to_enum().item_name().to_string()
    } else if let Ok(message) = BlockFilterMessage::from_compatible_slice(data) {
        message.to_enum().item_name().to_string()
    } else {
        panic!("unknown message item");
    }
}

/// Captures thread-local per-spec routing state (`SpecRunDirGuard` and `SpecLogGuard`)
/// so helper threads spawned during a spec keep writing into the same spec dir and `runner.log`.
pub(crate) struct SpecThreadContext {
    spec_dir: Option<PathBuf>,
    log_sink: Option<crate::logger::SpecLogSink>,
}

impl Clone for SpecThreadContext {
    fn clone(&self) -> Self {
        Self {
            spec_dir: self.spec_dir.clone(),
            log_sink: self.log_sink.as_ref().map(std::sync::Arc::clone),
        }
    }
}

pub(crate) struct SpecThreadContextGuard {
    _dir_guard: Option<SpecRunDirGuard>,
    _log_guard: crate::logger::SpecLogGuard,
}

impl SpecThreadContext {
    pub(crate) fn capture() -> Self {
        Self {
            spec_dir: SpecRunDirGuard::current_dir(),
            log_sink: crate::logger::SpecLogGuard::current_sink(),
        }
    }

    pub(crate) fn enter(&self) -> SpecThreadContextGuard {
        let _dir_guard = self.spec_dir.clone().map(SpecRunDirGuard::enter);
        let _log_guard = crate::logger::SpecLogGuard::enter_shared(
            self.log_sink.as_ref().map(std::sync::Arc::clone),
        );
        SpecThreadContextGuard {
            _dir_guard,
            _log_guard,
        }
    }
}

/// Runs `f` across `items` in parallel scoped threads while inheriting the
/// caller's per-spec run directory and `runner.log` sink, explicitly joining
/// every thread and re-throwing the chronologically first child's original panic
/// payload instead of `std::thread::scope`'s generic `"a scoped thread panicked"`.
pub(crate) fn run_in_spec_scope<I, T, F>(items: I, f: F)
where
    I: IntoIterator<Item = T>,
    T: Send,
    F: Fn(T) + Sync,
{
    let ctx = SpecThreadContext::capture();
    let f = &f;
    let first_panic: std::sync::Mutex<Option<Box<dyn std::any::Any + Send>>> =
        std::sync::Mutex::new(None);
    let parent_name = std::thread::current().name().unwrap_or("spec").to_string();
    std::thread::scope(|scope| {
        let handles: Vec<_> = items
            .into_iter()
            .enumerate()
            .map(|(idx, item)| {
                let ctx = ctx.clone();
                let first_panic = &first_panic;
                let thread_name = format!("{parent_name}-{idx}");
                std::thread::Builder::new()
                    .name(thread_name)
                    .spawn_scoped(scope, move || {
                        let _guard = ctx.enter();
                        if let Err(payload) =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(item)))
                        {
                            let mut slot = first_panic.lock().unwrap_or_else(|e| e.into_inner());
                            if slot.is_none() {
                                *slot = Some(payload);
                            }
                        }
                    })
                    .expect("spawn scoped spec thread")
            })
            .collect();

        for handle in handles {
            let _ = handle.join();
        }
    });
    if let Some(payload) = first_panic.into_inner().unwrap_or_else(|e| e.into_inner()) {
        std::panic::resume_unwind(payload);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::remove_dir_all;

    #[test]
    fn temp_path_buf_leak_keeps_directory() {
        let temp = TempPathBuf::new("test", "leak");
        let path = temp.path().to_owned();
        let returned = temp.leak();
        assert_eq!(returned, path);
        assert!(
            path.exists(),
            "leak() should keep the temporary directory alive"
        );
        // Clean up manually since the test intentionally bypassed automatic cleanup.
        remove_dir_all(&path).unwrap();
    }

    #[test]
    fn spec_run_dir_groups_child_nodes_and_disambiguates_duplicates() {
        let spec_dir = TempPathBuf::new_spec_dir("DemoSpec [1/2]").expect("create spec dir");
        let spec_root = spec_dir.path().to_owned();
        assert!(spec_dir.is_owned_root());

        {
            let _guard = SpecRunDirGuard::enter(spec_root.clone());
            let node0 = TempPathBuf::new("DemoSpec", "node0");
            let node0_dup = TempPathBuf::new("DemoSpec", "node0");
            let node1 = TempPathBuf::new("DemoSpec", "node1");

            assert!(!node0.is_owned_root());
            assert_eq!(node0.path(), spec_root.join("node0"));
            assert_eq!(node0_dup.path(), spec_root.join("node0-1"));
            assert_eq!(node1.path(), spec_root.join("node1"));
            assert!(node0.path().is_dir());
            assert!(node0_dup.path().is_dir());
            assert!(node1.path().is_dir());
        }

        // Dropping the spec root removes the grouped directory and all child node dirs.
        drop(spec_dir);
        assert!(!spec_root.exists());
    }

    #[test]
    fn adaptive_poll_interval_caps_at_max() {
        assert_eq!(
            adaptive_poll_interval(0, 10, 100),
            Duration::from_millis(10)
        );
        assert_eq!(
            adaptive_poll_interval(1, 10, 100),
            Duration::from_millis(20)
        );
        assert_eq!(
            adaptive_poll_interval(3, 10, 100),
            Duration::from_millis(80)
        );
        assert_eq!(
            adaptive_poll_interval(4, 10, 100),
            Duration::from_millis(100)
        );
        assert_eq!(
            adaptive_poll_interval(100, 10, 100),
            Duration::from_millis(100)
        );
    }

    #[test]
    fn find_available_port_stays_in_non_ephemeral_band() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..32 {
            let port = find_available_port();
            assert!(
                (PORT_RANGE_START..PORT_RANGE_START + PORT_RANGE_LEN).contains(&port),
                "allocated port {port} outside safe non-ephemeral band"
            );
            assert!(seen.insert(port), "duplicate port allocated: {port}");
        }
    }

    #[test]
    fn next_candidate_port_preserves_explicit_start_and_wraps_auto_band() {
        let explicit = std::sync::atomic::AtomicU16::new(40_000);
        assert_eq!(next_candidate_port(&explicit, false), 40_000);
        assert_eq!(next_candidate_port(&explicit, false), 40_001);
        assert_eq!(next_candidate_port(&explicit, false), 40_002);

        let auto = std::sync::atomic::AtomicU16::new(PORT_RANGE_END - 1);
        assert_eq!(next_candidate_port(&auto, true), PORT_RANGE_END - 1);
        assert_eq!(next_candidate_port(&auto, true), PORT_RANGE_START);
        assert_eq!(next_candidate_port(&auto, true), PORT_RANGE_START + 1);
    }

    #[test]
    fn run_in_spec_scope_inherits_context_and_forwards_child_panic() {
        use std::io::Write;

        let spec_dir = TempPathBuf::new_spec_dir("SpecScopeUnitTest").expect("create spec dir");
        let spec_root = spec_dir.path().to_owned();
        let runner_log = spec_root.join("runner.log");

        let panic_res = {
            let _dir_guard = SpecRunDirGuard::enter(spec_root.clone());
            let _log_guard = crate::logger::SpecLogGuard::enter(&runner_log);

            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_in_spec_scope(["node0", "node1"], |node_name| {
                    assert_eq!(
                        SpecRunDirGuard::current_dir().as_deref(),
                        Some(spec_root.as_path())
                    );
                    if let Some(sink) = crate::logger::SpecLogGuard::current_sink() {
                        writeln!(sink.lock(), "starting {node_name}").unwrap();
                    }
                    if node_name == "node1" {
                        panic!("node1 exited during startup");
                    }
                });
            }))
        };

        let payload = panic_res.expect_err("scoped child panic should propagate");
        let msg = crate::extract_panic_message(payload.as_ref());
        assert_eq!(
            msg, "node1 exited during startup",
            "should preserve child panic payload instead of generic scoped thread panic"
        );

        let log_content = std::fs::read_to_string(&runner_log).unwrap();
        assert!(log_content.contains("starting node0"));
        assert!(log_content.contains("starting node1"));
    }
}
