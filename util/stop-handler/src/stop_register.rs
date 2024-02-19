use ckb_channel::TrySendError;
use ckb_logger::{debug, info, trace, warn};
use ckb_util::Mutex;
use std::sync::atomic::AtomicBool;
use tokio_util::sync::CancellationToken;

struct CkbServiceHandles {
    thread_handles: Vec<(String, std::thread::JoinHandle<()>)>,
}

/// Wait all ckb services exit
pub fn wait_all_ckb_services_exit() {
    info!("Waiting exit signal...");
    let exit_signal = new_crossbeam_exit_rx();
    let _ = exit_signal.recv();
    let mut handles = CKB_HANDLES.lock();
    debug!("wait_all_ckb_services_exit waiting all threads to exit");
    for (name, join_handle) in handles.thread_handles.drain(..) {
        match join_handle.join() {
            Ok(_) => {
                info!("Waiting thread {} done.", name);
            }
            Err(e) => {
                warn!("Waiting thread {}: ERROR: {:?}", name, e)
            }
        }
    }
    debug!("All ckb threads have been stopped.");
}

static CKB_HANDLES: once_cell::sync::Lazy<Mutex<CkbServiceHandles>> =
    once_cell::sync::Lazy::new(|| {
        Mutex::new(CkbServiceHandles {
            thread_handles: vec![],
        })
    });

static RECEIVED_STOP_SIGNAL: once_cell::sync::Lazy<AtomicBool> =
    once_cell::sync::Lazy::new(AtomicBool::default);

static TOKIO_EXIT: once_cell::sync::Lazy<CancellationToken> =
    once_cell::sync::Lazy::new(CancellationToken::new);

static CROSSBEAM_EXIT_SENDERS: once_cell::sync::Lazy<Mutex<Vec<ckb_channel::Sender<()>>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(vec![]));

/// Create a new CancellationToken for exit signal
pub fn new_tokio_exit_rx() -> CancellationToken {
    TOKIO_EXIT.clone()
}

/// Create a new crossbeam Receiver for exit signal
pub fn new_crossbeam_exit_rx() -> ckb_channel::Receiver<()> {
    let (tx, rx) = ckb_channel::bounded(1);
    CROSSBEAM_EXIT_SENDERS.lock().push(tx);
    rx
}

/// Check if the ckb process has received stop signal
pub fn has_received_stop_signal() -> bool {
    RECEIVED_STOP_SIGNAL.load(std::sync::atomic::Ordering::SeqCst)
}

/// Broadcast exit signals to all threads and all tokio tasks
pub fn broadcast_exit_signals() {
    debug!("Received exit signal; broadcasting exit signal to all threads");
    RECEIVED_STOP_SIGNAL.store(true, std::sync::atomic::Ordering::SeqCst);
    TOKIO_EXIT.cancel();
    CROSSBEAM_EXIT_SENDERS
        .lock()
        .iter()
        .for_each(|tx| match tx.try_send(()) {
            Ok(_) => {}
            Err(TrySendError::Full(_)) => info!("Ckb process has received exit signal"),
            Err(TrySendError::Disconnected(_)) => {
                debug!("broadcast thread: channel is disconnected")
            }
        });
}

/// Register a thread `JoinHandle` to `CKB_HANDLES`
pub fn register_thread(name: &str, thread_handle: std::thread::JoinHandle<()>) {
    trace!("Registering thread {}", name);
    CKB_HANDLES
        .lock()
        .thread_handles
        .push((name.into(), thread_handle));
    trace!("Thread registration completed {}", name);
}
