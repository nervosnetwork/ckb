use crate::main_node::MainNode;
use crate::mock_node::MockNode;
use ckb_async_runtime::Handle;
use ckb_error::AnyError;
use ckb_jsonrpc_types::{BlockNumber, LocalNode};
use ckb_launcher::SharedPackage;
use ckb_logger::{error, info};
use ckb_network::{DefaultExitHandler, ExitHandler};
use ckb_shared::Shared;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::JoinHandle;
use std::{fs, path::PathBuf, process, sync::Arc, thread, time::Duration};
use tokio::runtime::Builder;
pub use tokio::runtime::Runtime;

/// Create new threaded_scheduler tokio Runtime, return `Runtime`
pub fn new_bench_sync_runtime() -> (Handle, Runtime) {
    let runtime = Builder::new_multi_thread()
        .enable_all()
        .max_blocking_threads(1)
        .worker_threads(1)
        .thread_name("GlobalRt")
        .thread_name_fn(|| {
            static ATOMIC_ID: AtomicU32 = AtomicU32::new(0);
            let id = ATOMIC_ID
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                    // A long thread name will cut to 15 characters in debug tools.
                    // Such as "top", "htop", "gdb" and so on.
                    // It's a kernel limit.
                    //
                    // So if we want to see the whole name in debug tools,
                    // this number should have 6 digits at most,
                    // since the prefix uses 9 characters in below code.
                    //
                    // There still has a issue:
                    // When id wraps around, we couldn't know whether the old id
                    // is released or not.
                    // But we can ignore this, because it's almost impossible.
                    if n >= 999_999 {
                        Some(0)
                    } else {
                        Some(n + 1)
                    }
                })
                .expect("impossible since the above closure must return Some(number)");
            format!("GlobalRt-{}", id)
        })
        .build()
        .expect("ckb runtime initialized");

    let handle = runtime.handle().clone();

    (Handle { inner: handle }, runtime)
}

pub fn ckb_init(binary_path: PathBuf, work_dir: PathBuf, rpc_port: u64, p2p_port: u64) {
    let output = process::Command::new(binary_path.clone())
        .arg("init")
        .arg("-C")
        .arg(work_dir)
        .arg("--rpc-port")
        .arg(rpc_port.to_string())
        .arg("--p2p-port")
        .arg(p2p_port.to_string())
        .arg("--force")
        .output()
        .unwrap();
    if !output.status.success() {
        panic!("failed to execute ckb init: {:?}", output);
    }
}

pub fn start_mock_nodes(
    mock_nodes_count: u64,
    mock_work_dir: PathBuf,
    boot_node_rpc_port: u64,
    main_node_binary_path: PathBuf,
    handle: Handle,
    shared: Arc<once_cell::sync::OnceCell<(Shared, SharedPackage)>>,
    shared_ckb_db_path: PathBuf,
    exit_handler: DefaultExitHandler,
) -> (String, Vec<JoinHandle<()>>) {
    info!("mock work dir is {:?}", mock_work_dir);
    let mut bootnode: Option<String> = None;
    let mut mock_nodes = Vec::new();

    (0..mock_nodes_count).for_each(|node_id| {
        let mock_node_workdir = mock_work_dir.join(format!("mock_{}", node_id));
        let mock_node = MockNode {
            binary_path: main_node_binary_path.clone(),
            rpc_port: boot_node_rpc_port + node_id * 2,
            p2p_port: boot_node_rpc_port + node_id * 2 + 1,
            bootnode: bootnode.clone(),
            work_dir: mock_node_workdir,
            handle: handle.clone(),
            shared: shared.clone(),
            shared_db_path: shared_ckb_db_path.clone(),
            exit_handler: exit_handler.clone(),
        };

        mock_nodes.push(thread::spawn(move || mock_node.start()));

        if bootnode.is_none() {
            'loop_try: loop {
                match get_node_id(boot_node_rpc_port) {
                    Ok(node_id) => {
                        bootnode = Some(format!(
                            "/ip4/127.0.0.1/tcp/{}/p2p/{}",
                            boot_node_rpc_port + 1,
                            node_id
                        ));
                        break 'loop_try;
                    }
                    Err(err) => {
                        error!("failed to get bootnode id, sleep 1s {}", err);
                        thread::sleep(Duration::from_secs(1));
                    }
                }
            }
        }
    });
    (bootnode.unwrap(), mock_nodes)
}

pub fn run(
    main_node_binary_path: PathBuf,
    main_node_log_filter: String,
    shared_ckb_db_path: PathBuf,
    work_dir: PathBuf,
    mock_nodes_count: u64,
    target_height: u64,
    main_node_rpc_port: u64,
    mock_node_rpc_port: u64,
) {
    fs::create_dir_all(&work_dir).unwrap();

    let work_dir = work_dir.as_path().canonicalize().unwrap();

    let (handle, runtime) = new_bench_sync_runtime();

    let boot_node_rpc_port = mock_node_rpc_port;
    let exit_handler = DefaultExitHandler::default();
    let exit_handler_clone = exit_handler.clone();
    ctrlc::set_handler(move || {
        exit_handler_clone.notify_exit();
    })
    .expect("Error setting Ctrl-C handler");

    let _mock_work_dir = tempfile::tempdir().unwrap();
    let mock_work_dir = _mock_work_dir.path().to_path_buf();

    let (bootnode, mock_nodes) = start_mock_nodes(
        mock_nodes_count,
        mock_work_dir,
        boot_node_rpc_port,
        main_node_binary_path.clone(),
        handle,
        Arc::new(once_cell::sync::OnceCell::new()),
        shared_ckb_db_path,
        exit_handler.clone(),
    );

    let mut main_node = MainNode {
        binary_path: main_node_binary_path,
        rpc_port: main_node_rpc_port,
        p2p_port: main_node_rpc_port + 1,
        work_dir,
        bootnode,
        child: None,
    };

    main_node.validate();
    main_node.start(main_node_log_filter);

    thread::spawn(move || {
        // get_tip_header_number every 10 second in new thread
        loop {
            match get_tip_block_number(main_node_rpc_port).map(|n| u64::from(n)) {
                Ok(tip_number) => {
                    if tip_number >= target_height {
                        info!(
                            "main node has reached header_number: {} >= target_height {}",
                            tip_number, target_height
                        );
                        break;
                    }
                }
                Err(err) => {
                    error!("failed to get header number, sleep 1s {}", err);
                }
            }
            thread::sleep(Duration::from_secs(10));
        }

        // the main node has reached target_height, kill and exit
        process::Command::new("kill")
            .arg("-15")
            .arg(process::id().to_string())
            .output()
            .unwrap();
    });

    exit_handler.wait_for_exit();
    for mock_node in mock_nodes {
        let _ = mock_node.join();
        info!("a mock node stopped")
    }

    main_node.stop();

    runtime.shutdown_timeout(Duration::from_secs(1));

    drop(_mock_work_dir)
}

fn get_node_id(rpc_port: u64) -> Result<String, AnyError> {
    match get_local_node_info(rpc_port) {
        Ok(v) => Ok(v.node_id),
        Err(err) => Err(err),
    }
}

fn get_local_node_info(rpc_port: u64) -> Result<LocalNode, AnyError> {
    let url = format!("http://127.0.0.1:{}", rpc_port);

    let mut req_json = serde_json::Map::new();
    req_json.insert("id".to_owned(), serde_json::json!(1_u64));
    req_json.insert("jsonrpc".to_owned(), serde_json::json!("2.0"));
    req_json.insert("method".to_owned(), serde_json::json!("local_node_info"));
    req_json.insert("params".to_owned(), serde_json::json!(Vec::<String>::new()));
    let client = reqwest::blocking::Client::new();

    let resp = client.post(url).json(&req_json).send()?;
    let output = resp.json::<jsonrpc_core::response::Output>()?;
    match output {
        jsonrpc_core::response::Output::Success(success) => {
            serde_json::from_value(success.result).map_err(Into::into)
        }
        jsonrpc_core::response::Output::Failure(failure) => Err(AnyError::from(failure.error)),
    }
}
fn get_tip_block_number(rpc_port: u64) -> Result<BlockNumber, AnyError> {
    let url = format!("http://127.0.0.1:{}", rpc_port);

    let mut req_json = serde_json::Map::new();
    req_json.insert("id".to_owned(), serde_json::json!(1_u64));
    req_json.insert("jsonrpc".to_owned(), serde_json::json!("2.0"));
    req_json.insert(
        "method".to_owned(),
        serde_json::json!("get_tip_block_number"),
    );
    req_json.insert("params".to_owned(), serde_json::json!(Vec::<String>::new()));
    let client = reqwest::blocking::Client::new();

    let resp = client.post(url).json(&req_json).send()?;
    let output = resp.json::<jsonrpc_core::response::Output>()?;
    match output {
        jsonrpc_core::response::Output::Success(success) => {
            serde_json::from_value(success.result).map_err(Into::into)
        }
        jsonrpc_core::response::Output::Failure(failure) => Err(AnyError::from(failure.error)),
    }
}
