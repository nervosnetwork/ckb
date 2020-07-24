use ckb_jsonrpc_types::{ExtraLoggerConfig, MainLoggerConfig};
use ckb_logger::Logger;
use jsonrpc_core::{Error, ErrorCode::InternalError, Result};
use jsonrpc_derive::rpc;
use std::time;

#[rpc(server)]
pub trait DebugRpc {
    #[rpc(name = "jemalloc_profiling_dump")]
    fn jemalloc_profiling_dump(&self) -> Result<String>;
    #[rpc(name = "update_main_logger")]
    fn update_main_logger(&self, config: MainLoggerConfig) -> Result<()>;
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
