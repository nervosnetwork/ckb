use jsonrpc_core::{Error, ErrorCode};

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum RPCError {
    Staging = -2,
}

impl RPCError {
    pub fn custom(err: RPCError, message: String) -> Error {
        Error {
            code: ErrorCode::ServerError(err as i64),
            message,
            data: None,
        }
    }
}
