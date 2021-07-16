use jsonrpc_core::error::Error as JsonRpcError;
use std::fmt;

#[derive(Debug, PartialEq, Clone)]
pub struct Error {
    pub(in crate::rpc) inner: JsonRpcError,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_string(&self.inner).expect("JsonRpcError to_string")
        )
    }
}

impl ::std::error::Error for Error {}
