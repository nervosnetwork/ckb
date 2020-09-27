use ckb_jsonrpc_types::{ExtraLoggerConfig, MainLoggerConfig};
use ckb_logger_service::Logger;
use jsonrpc_core::{Error, ErrorCode::InternalError, Result};
use jsonrpc_derive::rpc;
use std::time;

/// RPC Module Debug for internal RPC methods.
///
/// **This module is for CKB developers and will not guarantee compatibility.** The methods here
/// will be changed or removed without advanced notification.
#[rpc(server)]
#[doc(hidden)]
pub trait DebugRpc {
    /// Dumps jemalloc memory profiling information into a file.
    ///
    /// The file is stored in the server running the CKB node.
    ///
    /// The RPC returns the path to the dumped file on success or returns an error on failure.
    #[rpc(name = "jemalloc_profiling_dump")]
    fn jemalloc_profiling_dump(&self) -> Result<String>;
    /// Changes main logger config options while CKB is running.
    #[rpc(name = "update_main_logger")]
    fn update_main_logger(&self, config: MainLoggerConfig) -> Result<()>;
    /// Sets logger config options for extra loggers.
    ///
    /// CKB nodes allow setting up extra loggers. These loggers will have their own log files and
    /// they only append logs to their log files.
    ///
    /// ## Params
    ///
    /// * `name` - Extra logger name
    /// * `config_opt` - Adds a new logger or update an existing logger when this is not null.
    /// Removes the logger when this is null.
    #[rpc(name = "set_extra_logger")]
    fn set_extra_logger(&self, name: String, config_opt: Option<ExtraLoggerConfig>) -> Result<()>;
}

pub(crate) struct DebugRpcImpl {}

impl DebugRpc for DebugRpcImpl {
    fn jemalloc_profiling_dump(&self) -> Result<String> {
        let timestamp = time::SystemTime::now()
            .duration_since(time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let filename = format!("ckb-jeprof.{}.heap", timestamp);
        match ckb_memory_tracker::jemalloc_profiling_dump(&filename) {
            Ok(()) => Ok(filename),
            Err(err) => Err(Error {
                code: InternalError,
                message: err,
                data: None,
            }),
        }
    }

    fn update_main_logger(&self, config: MainLoggerConfig) -> Result<()> {
        let MainLoggerConfig {
            filter,
            to_stdout,
            to_file,
            color,
        } = config;
        if filter.is_none() && to_stdout.is_none() && to_file.is_none() && color.is_none() {
            return Ok(());
        }
        Logger::update_main_logger(filter, to_stdout, to_file, color).map_err(|err| Error {
            code: InternalError,
            message: err,
            data: None,
        })
    }

    fn set_extra_logger(&self, name: String, config_opt: Option<ExtraLoggerConfig>) -> Result<()> {
        if let Err(err) = Logger::check_extra_logger_name(&name) {
            return Err(Error {
                code: InternalError,
                message: err,
                data: None,
            });
        }
        if let Some(config) = config_opt {
            Logger::update_extra_logger(name, config.filter)
        } else {
            Logger::remove_extra_logger(name)
        }
        .map_err(|err| Error {
            code: InternalError,
            message: err,
            data: None,
        })
    }
}
