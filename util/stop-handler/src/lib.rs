//! TODO(doc): @keroro520
use ckb_logger::error;
use parking_lot::Mutex;
use std::fmt::Debug;
use std::sync::mpsc;
use std::sync::{Arc, Weak};
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

/// Weak is a version of Arc that holds a non-owning reference to the managed allocation.
/// Since a Weak reference does not count towards ownership,
/// it will not prevent the value stored in the allocation from being dropped,
/// and Weak itself makes no guarantees about the value still being present.
#[derive(Debug)]
enum Ref<T> {
    Arc(Arc<T>),
    Weak(Weak<T>),
}

impl<T> Clone for Ref<T> {
    #[inline]
    fn clone(&self) -> Ref<T> {
        match self {
            Self::Arc(arc) => Self::Arc(Arc::clone(arc)),
            Self::Weak(weak) => Self::Weak(Weak::clone(weak)),
        }
    }
}

impl<T> Ref<T> {
    fn downgrade(&self) -> Ref<T> {
        match self {
            Self::Arc(arc) => Self::Weak(Arc::downgrade(arc)),
            Self::Weak(weak) => Self::Weak(Weak::clone(weak)),
        }
    }
}

/// TODO(doc): @keroro520
//the outer Option take ownership for `Arc::try_unwrap`
//the inner Option take ownership for `JoinHandle` or `oneshot::Sender`
#[derive(Clone, Debug)]
pub struct StopHandler<T> {
    inner: Option<Ref<Mutex<Option<Handler<T>>>>>,
    name: String,
}

impl<T: Debug> StopHandler<T> {
    /// TODO(doc): @keroro520
    pub fn new(
        signal: SignalSender<T>,
        thread: Option<JoinHandle<T>>,
        name: String,
    ) -> StopHandler<T> {
        let handler = Handler { signal, thread };
        StopHandler {
            inner: Some(Ref::Arc(Arc::new(Mutex::new(Some(handler))))),
            name,
        }
    }

    /// Creates a new Weak pointer.
    pub fn downgrade_clone(&self) -> StopHandler<T> {
        StopHandler {
            inner: self.inner.as_ref().map(|inner| inner.downgrade()),
            name: self.name.clone(),
        }
    }

    /// TODO(doc): @keroro520
    pub fn try_send(&mut self, cmd: T) {
        let inner = self
            .inner
            .take()
            .expect("Stop signal can only be sent once");

        if let Ref::Arc(inner) = inner {
            if let Ok(lock) = Arc::try_unwrap(inner) {
                ckb_logger::info!("StopHandler({}) send signal", self.name);
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
}
