//! Stop handler utilities for graceful shutdown.
//!
//! This crate provides utilities for managing graceful shutdown of CKB components,
//! including signal handling and cancellation token management.

pub use stop_register::{
    broadcast_exit_signals, has_received_stop_signal, new_crossbeam_exit_rx, new_tokio_exit_rx,
    register_thread, wait_all_ckb_services_exit,
};

pub use tokio_util::sync::CancellationToken;

mod stop_register;

#[cfg(all(test, unix))]
mod tests;
