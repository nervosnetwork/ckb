use jsonrpc_core::{Error, ErrorCode, Value};
use std::fmt::{Debug, Display};

// * -1 ~ -999 General errors
// * -1000 ~ -2999 Module specific errors
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum RPCError {
    // ,-- General application errors
    Invalid = -3,
    RPCModuleIsDisabled = -4,
    // ,-- P2P errors
    P2PFailedToBroadcast = -101,
    // ,-- Alert module
    AlertFailedToVerifySignatures = -1000,
}

impl RPCError {
    pub fn invalid_params<T: Display>(message: T) -> Error {
        Error {
            code: ErrorCode::InvalidParams,
            message: format!("InvalidParams: {}", message),
            data: None,
        }
    }

    pub fn custom<T: Display>(error_code: RPCError, message: T) -> Error {
        Error {
            code: ErrorCode::ServerError(error_code as i64),
            message: format!("{:?}: {}", error_code, message),
            data: None,
        }
    }

    pub fn custom_with_error<T: Display + Debug>(error_code: RPCError, err: T) -> Error {
        Error {
            code: ErrorCode::ServerError(error_code as i64),
            message: format!("{:?}: {}", error_code, err),
            data: Some(Value::String(format!("{:?}", err))),
        }
    }

    pub fn rpc_module_is_disabled(module: &str) -> Error {
        Self::custom(
            RPCError::RPCModuleIsDisabled,
            format!(
                "This RPC method is in the module `{module}`. \
                 Please modify `rpc.modules`{miner_info} in ckb.toml and restart the ckb node to enable it.",
                 module = module, miner_info = if module == "Miner" {" and `block_assembler`"} else {""}
            )
        )
    }
}
