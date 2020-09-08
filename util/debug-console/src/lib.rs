use std::{collections::HashMap, sync::Arc};

use lazy_static::lazy_static;
use tokio::sync::{mpsc, oneshot};

use ckb_async_runtime::{new_runtime, Builder, Handle};
use ckb_debug_console_common::Request;
use ckb_debug_console_config::Config;
use ckb_logger::debug;
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_util::RwLock;

pub use ckb_debug_console_common as common;

mod client;
mod commands;
mod console;

lazy_static! {
    static ref SERVICES: Arc<RwLock<HashMap<&'static str, mpsc::Sender<Request>>>> =
        Arc::new(RwLock::new(HashMap::new()));
}

#[must_use]
pub enum Guard {
    Off,
    On {
        handle: Handle,
        stop: StopHandler<()>,
    },
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Self::On { ref mut stop, .. } = self {
            stop.try_send();
        }
    }
}

pub fn init(config_opt: Option<Config>) -> Result<Guard, String> {
    if let Some(config) = config_opt {
        if let Some(socket_addr) = config.listen_address {
            debug!("enabled");
            let mut runtime_builder = Builder::new();
            runtime_builder
                .threaded_scheduler()
                .enable_io()
                .enable_time();
            if config.threads != 0 {
                runtime_builder.core_threads(config.threads);
            } else {
                runtime_builder.core_threads(2);
            };
            let (signal_sender, mut signal_receiver) = oneshot::channel();
            let service = move |_: Handle| async move {
                loop {
                    tokio::select! { _ = &mut signal_receiver => break }
                }
            };
            let (handle, thread) = new_runtime("DebugConsole", Some(runtime_builder), service);

            handle.spawn(async move {
                tokio::spawn(console::async_run(socket_addr));
            });

            let stop = StopHandler::new(SignalSender::Tokio(signal_sender), thread);

            Ok(Guard::On { handle, stop })
        } else {
            debug!("disabled");
            Ok(Guard::Off)
        }
    } else {
        debug!("disabled");
        Ok(Guard::Off)
    }
}

pub fn register(cmd: &'static str, sender: mpsc::Sender<Request>) {
    let _item = SERVICES.write().insert(cmd, sender);
}
