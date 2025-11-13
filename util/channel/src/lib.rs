//! Reexports `crossbeam_channel` to uniform the dependency version.
pub use crossbeam_channel::{
    Receiver, RecvError, RecvTimeoutError, Select, SendError, Sender, TrySendError, after, bounded,
    select, tick, unbounded,
};

pub mod oneshot {
    //! A one-shot channel is used for sending a single message between asynchronous tasks.

    use std::sync::mpsc::sync_channel;
    pub use std::sync::mpsc::{Receiver, RecvError, SyncSender as Sender};

    /// Create a new one-shot channel for sending single values across asynchronous tasks.
    pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
        sync_channel(1)
    }
}

/// Default channel size to send control signals.
pub const SIGNAL_CHANNEL_SIZE: usize = 1;
/// Default channel size to send messages.
pub const DEFAULT_CHANNEL_SIZE: usize = 32;

/// Synchronous request sent to the service.
pub struct Request<A, R> {
    /// One shot channel for the service to send back the response.
    pub responder: oneshot::Sender<R>,
    /// Request arguments.
    pub arguments: A,
}

impl<A, R> Request<A, R> {
    /// Call the service with the arguments and wait for the response.
    pub fn call(sender: &Sender<Request<A, R>>, arguments: A) -> Option<R> {
        let (responder, response) = oneshot::channel();
        let _ = sender.send(Request {
            responder,
            arguments,
        });
        response.recv().ok()
    }

    /// Call the service with the arguments and don't wait for the response.
    pub fn call_without_response(sender: &Sender<Request<A, R>>, arguments: A) {
        let (responder, _response) = oneshot::channel();
        let _ = sender.send(Request {
            responder,
            arguments,
        });
    }
}
