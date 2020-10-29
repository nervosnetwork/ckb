//! Reexports `crossbeam_channel` to uniform the dependency version.
pub use crossbeam_channel::{
    bounded, select, unbounded, Receiver, RecvError, RecvTimeoutError, Sender,
};
