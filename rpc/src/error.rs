use ckb_error::{AnyError, Error as CKBError, ErrorKind, InternalError, InternalErrorKind};
use ckb_tx_pool::error::Reject;
use jsonrpc_core::{Error, ErrorCode, Value};
use std::fmt::{Debug, Display};

/// CKB RPC error codes.
///
/// CKB RPC follows the JSON RPC specification about the [error object](https://www.jsonrpc.org/specification#error_object).
///
/// Besides the pre-defined errors, all CKB defined errors are listed here.
///
/// Here is a reference to the pre-defined errors:
///
/// | code             | message          | meaning                                            |
/// | ---------------- | ---------------- | -------------------------------------------------- |
/// | -32700           | Parse error      | Invalid JSON was received by the server.           |
/// | -32600           | Invalid Request  | The JSON sent is not a valid Request object.       |
/// | -32601           | Method not found | The method does not exist / is not available.      |
/// | -32602           | Invalid params   | Invalid method parameter(s).                       |
/// | -32603           | Internal error   | Internal JSON-RPC error.                           |
/// | -32000 to -32099 | Server error     | Reserved for implementation-defined server-errors. |
///
/// CKB application-defined errors follow some patterns to assign the codes:
///
/// * -1 ~ -999 are general errors
/// * -1000 ~ -2999 are module-specific errors. Each module generally gets 100 reserved error
/// codes.
///
/// Unless otherwise noted, all the errors return optional detailed information as `string` in the error
/// object `data` field.
#[derive(Debug, PartialEq, Clone, Copy, Eq)]
pub enum RPCError {
    /// (-1): CKB internal errors are considered to never happen or only happen when the system
    /// resources are exhausted.
    CKBInternalError = -1,
    /// (-2): The CKB method has been deprecated and disabled.
    ///
    /// Set `rpc.enable_deprecated_rpc` to `true` in the config file to enable all deprecated
    /// methods.
    Deprecated = -2,
    /// (-3): Error code -3 is no longer used.
    ///
    /// Before v0.35.0, CKB returns all RPC errors using the error code -3. CKB no longer uses
    /// -3 since v0.35.0.
    Invalid = -3,
    /// (-4): The RPC method is not enabled.
    ///
    /// CKB groups RPC methods into modules, and a method is enabled only when the module is
    /// explicitly enabled in the config file.
    RPCModuleIsDisabled = -4,
    /// (-5): DAO related errors.
    DaoError = -5,
    /// (-6): Integer operation overflow.
    IntegerOverflow = -6,
    /// (-7): The error is caused by a config file option.
    ///
    /// Users have to edit the config file to fix the error.
    ConfigError = -7,
    /// (-101): The CKB local node failed to broadcast a message to its peers.
    P2PFailedToBroadcast = -101,
    /// (-200): Internal database error.
    ///
    /// The CKB node persists data to the database. This is the error from the underlying database
    /// module.
    DatabaseError = -200,
    /// (-201): The chain index is inconsistent.
    ///
    /// An example of an inconsistent index is that the chain index says a block hash is in the chain
    /// but the block cannot be read from the database.
    ///
    /// This is a fatal error usually due to a serious bug. Please back up the data directory and
    /// re-sync the chain from scratch.
    ChainIndexIsInconsistent = -201,
    /// (-202): The underlying database is corrupt.
    ///
    /// This is a fatal error usually caused by the underlying database used by CKB. Please back up
    /// the data directory and re-sync the chain from scratch.
    DatabaseIsCorrupt = -202,
    /// (-301): Failed to resolve the referenced cells and headers used in the transaction, as inputs or
    /// dependencies.
    TransactionFailedToResolve = -301,
    /// (-302): Failed to verify the transaction.
    TransactionFailedToVerify = -302,
    /// (-1000): Some signatures in the submit alert are invalid.
    AlertFailedToVerifySignatures = -1000,
    /// (-1102): The transaction is rejected by the outputs validator specified by the RPC parameter.
    PoolRejectedTransactionByOutputsValidator = -1102,
    /// (-1103): Pool rejects some transactions which seem contain invalid VM instructions. See the issue
    /// link in the error message for details.
    PoolRejectedTransactionByIllTransactionChecker = -1103,
    /// (-1104): The transaction fee rate must be greater than or equal to the config option `tx_pool.min_fee_rate`
    ///
    /// The fee rate is calculated as:
    ///
    /// ```text
    /// fee / (1000 * tx_serialization_size_in_block_in_bytes)
    /// ```
    PoolRejectedTransactionByMinFeeRate = -1104,
    /// (-1105): The in-pool ancestors count must be less than or equal to the config option `tx_pool.max_ancestors_count`
    ///
    /// Pool rejects a large package of chained transactions to avoid certain kinds of DoS attacks.
    PoolRejectedTransactionByMaxAncestorsCountLimit = -1105,
    /// (-1106): The transaction is rejected because the pool has reached its limit.
    PoolIsFull = -1106,
    /// (-1107): The transaction is already in the pool.
    PoolRejectedDuplicatedTransaction = -1107,
    /// (-1108): The transaction is rejected because it does not make sense in the context.
    ///
    /// For example, a cellbase transaction is not allowed in `send_transaction` RPC.
    PoolRejectedMalformedTransaction = -1108,
    /// (-1109): The transaction is expired from tx-pool after `expiry_hours`.
    TransactionExpired = -1109,
    /// (-1200): The indexer error.
    Indexer = -1200,
}

impl RPCError {
    /// Invalid method parameter(s).
    pub fn invalid_params<T: Display>(message: T) -> Error {
        Error {
            code: ErrorCode::InvalidParams,
            message: format!("InvalidParams: {}", message),
            data: None,
        }
    }

    /// Creates an RPC error with custom error code and message.
    pub fn custom<T: Display>(error_code: RPCError, message: T) -> Error {
        Error {
            code: ErrorCode::ServerError(error_code as i64),
            message: format!("{:?}: {}", error_code, message),
            data: None,
        }
    }

    /// Creates an RPC error with custom error code, message and data.
    pub fn custom_with_data<T: Display, F: Debug>(
        error_code: RPCError,
        message: T,
        data: F,
    ) -> Error {
        Error {
            code: ErrorCode::ServerError(error_code as i64),
            message: format!("{:?}: {}", error_code, message),
            data: Some(Value::String(format!("{:?}", data))),
        }
    }

    /// Creates an RPC error from std error with the custom error code.
    ///
    /// The parameter `err` is usually an std error. The Display form is used as the error message,
    /// and the Debug form is used as the data.
    pub fn custom_with_error<T: Display + Debug>(error_code: RPCError, err: T) -> Error {
        Error {
            code: ErrorCode::ServerError(error_code as i64),
            message: format!("{:?}: {}", error_code, err),
            data: Some(Value::String(format!("{:?}", err))),
        }
    }

    /// Creates an RPC error from the reason that a transaction is rejected to be submitted.
    pub fn from_submit_transaction_reject(reject: &Reject) -> Error {
        let code = match reject {
            Reject::LowFeeRate(_, _, _) => RPCError::PoolRejectedTransactionByMinFeeRate,
            Reject::ExceededMaximumAncestorsCount => {
                RPCError::PoolRejectedTransactionByMaxAncestorsCountLimit
            }
            Reject::Full(_, _) => RPCError::PoolIsFull,
            Reject::Duplicated(_) => RPCError::PoolRejectedDuplicatedTransaction,
            Reject::Malformed(_) => RPCError::PoolRejectedMalformedTransaction,
            Reject::DeclaredWrongCycles(..) => RPCError::PoolRejectedMalformedTransaction,
            Reject::Resolve(_) => RPCError::TransactionFailedToResolve,
            Reject::Verification(_) => RPCError::TransactionFailedToVerify,
            Reject::Expiry(_) => RPCError::TransactionExpired,
        };
        RPCError::custom_with_error(code, reject)
    }

    /// Creates an CKB error from `CKBError`.
    pub fn from_ckb_error(err: CKBError) -> Error {
        match err.kind() {
            ErrorKind::Dao => Self::custom_with_error(RPCError::DaoError, err.root_cause()),
            ErrorKind::OutPoint => {
                Self::custom_with_error(RPCError::TransactionFailedToResolve, err)
            }
            ErrorKind::Transaction => {
                Self::custom_with_error(RPCError::TransactionFailedToVerify, err.root_cause())
            }
            ErrorKind::Internal => {
                let internal_err = match err.downcast_ref::<InternalError>() {
                    Some(err) => err,
                    None => return Self::ckb_internal_error(err),
                };

                let kind = match internal_err.kind() {
                    InternalErrorKind::CapacityOverflow => RPCError::IntegerOverflow,
                    InternalErrorKind::DataCorrupted => RPCError::DatabaseIsCorrupt,
                    InternalErrorKind::Database => RPCError::DatabaseError,
                    InternalErrorKind::Config => RPCError::ConfigError,
                    _ => RPCError::CKBInternalError,
                };

                RPCError::custom_with_error(kind, internal_err)
            }
            _ => Self::custom_with_error(RPCError::CKBInternalError, err),
        }
    }

    /// Creates an RPC error from `AnyError`.
    pub fn from_any_error(err: AnyError) -> Error {
        match err.downcast_ref::<CKBError>() {
            Some(ckb_error) => Self::from_ckb_error(ckb_error.clone()),
            None => Self::ckb_internal_error(err.clone()),
        }
    }

    /// CKB internal error.
    ///
    /// CKB internal errors are considered to never happen or only happen when the system
    /// resources are exhausted.
    pub fn ckb_internal_error<T: Display + Debug>(err: T) -> Error {
        Self::custom_with_error(RPCError::CKBInternalError, err)
    }

    /// RPC error which indicates that the method is disabled.
    ///
    /// RPC methods belong to modules and they are only enabled when the belonging module is
    /// included in the config.
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

    /// RPC error which indicates that the method is deprecated.
    ///
    /// Deprecated methods are disabled by default unless they are enabled via the config options
    /// `enable_deprecated_rpc`.
    pub fn rpc_method_is_deprecated() -> Error {
        Self::custom(
            RPCError::Deprecated,
            "This RPC method is deprecated, it will be removed in future release. \
            Please check the related information in the CKB release notes and RPC document. \
            You may enable deprecated methods via adding `enable_deprecated_rpc = true` to the `[rpc]` section in ckb.toml.",
        )
    }
}
