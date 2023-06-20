//! TODO(doc): @keroro520

pub use stop_register::{
    broadcast_exit_signals, new_crossbeam_exit_rx, new_tokio_exit_rx, register_thread,
    wait_all_ckb_services_exit,
};

pub use tokio_util::sync::CancellationToken;

mod stop_register;
#[cfg(test)]
mod tests;
