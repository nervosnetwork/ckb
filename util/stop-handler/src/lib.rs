use ckb_logger::error;
use futures::sync::oneshot;
use parking_lot::Mutex;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use tokio::sync::oneshot as tokio_oneshot;

#[derive(Debug)]
pub enum SignalSender {
    Future(oneshot::Sender<()>),
    Crossbeam(ckb_channel::Sender<()>),
    Std(mpsc::Sender<()>),
    Tokio(tokio_oneshot::Sender<()>),
}

impl SignalSender {
    pub fn send(self) {
        match self {
            SignalSender::Crossbeam(tx) => {
                if let Err(e) = tx.send(()) {
                    error!("handler signal send error {:?}", e);
                };
            }
            SignalSender::Std(tx) => {
                if let Err(e) = tx.send(()) {
                    error!("handler signal send error {:?}", e);
                };
            }
            SignalSender::Future(tx) => {
                if let Err(e) = tx.send(()) {
                    error!("handler signal send error {:?}", e);
                };
            }
            SignalSender::Tokio(tx) => {
                if let Err(e) = tx.send(()) {
                    error!("handler signal send error {:?}", e);
                };
            }
        }
    }
}

#[derive(Debug)]
struct Handler<T> {
    signal: SignalSender,
    thread: Option<JoinHandle<T>>,
}

//the outer Option take ownership for `Arc::try_unwrap`
//the inner Option take ownership for `JoinHandle` or `oneshot::Sender`
#[derive(Clone, Debug)]
pub struct StopHandler<T> {
    inner: Option<Arc<Mutex<Option<Handler<T>>>>>,
}

impl<T> StopHandler<T> {
    pub fn new(signal: SignalSender, thread: Option<JoinHandle<T>>) -> StopHandler<T> {
        let handler = Handler { signal, thread };
        StopHandler {
            inner: Some(Arc::new(Mutex::new(Some(handler)))),
        }
    }

    pub fn try_send(&mut self) {
        let inner = self
            .inner
            .take()
            .expect("Stop signal can only be sent once");
        if let Ok(lock) = Arc::try_unwrap(inner) {
            let handler = lock.lock().take().expect("Handler can only be taken once");
            let Handler { signal, thread } = handler;
            signal.send();
            if let Some(thread) = thread {
                if let Err(e) = thread.join() {
                    error!("handler thread join error {:?}", e);
                };
            }
        };
    }
}
