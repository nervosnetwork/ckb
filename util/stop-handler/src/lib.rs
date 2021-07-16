//! TODO(doc): @keroro520
use ckb_logger::error;
use parking_lot::Mutex;
use std::fmt::Debug;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use tokio::sync::oneshot as tokio_oneshot;
use tokio::sync::watch as tokio_watch;

/// init flags
pub const WATCH_INIT: u8 = 0;
/// stop flags
pub const WATCH_STOP: u8 = 1;

/// TODO(doc): @keroro520
#[derive(Debug)]
pub enum SignalSender<T> {
    /// TODO(doc): @keroro520
    Crossbeam(ckb_channel::Sender<T>),
    /// TODO(doc): @keroro520
    Std(mpsc::Sender<T>),
    /// TODO(doc): @keroro520
    Tokio(tokio_oneshot::Sender<T>),
    /// A single-producer, multi-consumer channel that only retains the last sent value.
    Watch(tokio_watch::Sender<u8>),
}

impl<T: Debug> SignalSender<T> {
    /// TODO(doc): @keroro520
    pub fn send(self, cmd: T) {
        match self {
            SignalSender::Crossbeam(tx) => {
                if let Err(e) = tx.try_send(cmd) {
                    error!("handler signal send error {:?}", e);
                };
            }
            SignalSender::Std(tx) => {
                if let Err(e) = tx.send(cmd) {
                    error!("handler signal send error {:?}", e);
                };
            }
            SignalSender::Tokio(tx) => {
                if let Err(e) = tx.send(cmd) {
                    error!("handler signal send error {:?}", e);
                };
            }
            SignalSender::Watch(tx) => {
                if let Err(e) = tx.send(WATCH_STOP) {
                    error!("handler signal send error {:?}", e);
                };
            }
        }
    }
}

#[derive(Debug)]
struct Handler<T> {
    signal: SignalSender<T>,
    thread: Option<JoinHandle<T>>,
}

/// TODO(doc): @keroro520
//the outer Option take ownership for `Arc::try_unwrap`
//the inner Option take ownership for `JoinHandle` or `oneshot::Sender`
#[derive(Clone, Debug)]
pub struct StopHandler<T> {
    inner: Option<Arc<Mutex<Option<Handler<T>>>>>,
}

impl<T: Debug> StopHandler<T> {
    /// TODO(doc): @keroro520
    pub fn new(signal: SignalSender<T>, thread: Option<JoinHandle<T>>) -> StopHandler<T> {
        let handler = Handler { signal, thread };
        StopHandler {
            inner: Some(Arc::new(Mutex::new(Some(handler)))),
        }
    }

    /// TODO(doc): @keroro520
    pub fn try_send(&mut self, cmd: T) {
        let inner = self
            .inner
            .take()
            .expect("Stop signal can only be sent once");
        if let Ok(lock) = Arc::try_unwrap(inner) {
            let handler = lock.lock().take().expect("Handler can only be taken once");
            let Handler { signal, thread } = handler;
            signal.send(cmd);
            if let Some(thread) = thread {
                if let Err(e) = thread.join() {
                    error!("handler thread join error {:?}", e);
                };
            }
        };
    }
}
