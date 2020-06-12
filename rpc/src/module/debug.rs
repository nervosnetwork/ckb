use ckb_logger::configure_logger_filter;
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use std::time;

#[rpc(server)]
pub trait DebugRpc {
    #[rpc(name = "jemalloc_profiling_dump")]
    fn jemalloc_profiling_dump(&self) -> Result<()>;
    #[rpc(name = "set_logger_filter")]
    fn set_logger_filter(&self, filter: String) -> Result<()>;
}

pub(crate) struct DebugRpcImpl {}

impl DebugRpc for DebugRpcImpl {
    fn jemalloc_profiling_dump(&self) -> Result<()> {
        let timestamp = time::SystemTime::now()
            .duration_since(time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let filename = format!("ckb-jeprof.{}.heap\0", timestamp);
        ckb_memory_tracker::jemalloc_profiling_dump(filename);
        Ok(())
    }

    fn set_logger_filter(&self, filter: String) -> Result<()> {
        configure_logger_filter(&filter);
        Ok(())
    }
}
