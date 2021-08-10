//! Reexports `crossbeam_channel` to uniform the dependency version.
pub use crossbeam_channel::{
    bounded, select, unbounded, Receiver, RecvError, RecvTimeoutError, Select, SendError, Sender,
    TrySendError,
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
