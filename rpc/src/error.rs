use ckb_error::{Error as CKBError, ErrorKind, InternalError, InternalErrorKind};
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
#[derive(Debug, PartialEq, Clone, Copy)]
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
}

impl RPCError {
    /// TODO(doc): @doitian
    pub fn invalid_params<T: Display>(message: T) -> Error {
        Error {
            code: ErrorCode::InvalidParams,
            message: format!("InvalidParams: {}", message),
            data: None,
        }
    }

    /// TODO(doc): @doitian
    pub fn custom<T: Display>(error_code: RPCError, message: T) -> Error {
        Error {
            code: ErrorCode::ServerError(error_code as i64),
            message: format!("{:?}: {}", error_code, message),
            data: None,
        }
    }

    /// TODO(doc): @doitian
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

    /// TODO(doc): @doitian
    pub fn custom_with_error<T: Display + Debug>(error_code: RPCError, err: T) -> Error {
        Error {
            code: ErrorCode::ServerError(error_code as i64),
            message: format!("{:?}: {}", error_code, err),
            data: Some(Value::String(format!("{:?}", err))),
        }
    }

    /// TODO(doc): @doitian
    pub fn from_submit_transaction_reject(reject: &Reject) -> Error {
        let code = match reject {
            Reject::LowFeeRate(_, _) => RPCError::PoolRejectedTransactionByMinFeeRate,
            Reject::ExceededMaximumAncestorsCount => {
                RPCError::PoolRejectedTransactionByMaxAncestorsCountLimit
            }
            Reject::Full(_, _) => RPCError::PoolIsFull,
            Reject::Duplicated(_) => RPCError::PoolRejectedDuplicatedTransaction,
            Reject::Malformed(_) => RPCError::PoolRejectedMalformedTransaction,
        };
        RPCError::custom_with_error(code, reject)
    }

    /// TODO(doc): @doitian
    pub fn downcast_submit_transaction_reject(err: &CKBError) -> Option<&Reject> {
        use ckb_error::ErrorKind::SubmitTransaction;
        match err.kind() {
            SubmitTransaction => err.downcast_ref::<Reject>(),
            _ => None,
        }
    }

    /// TODO(doc): @doitian
    pub fn from_ckb_error(err: CKBError) -> Error {
        match err.kind() {
            ErrorKind::Dao => {
                Self::custom_with_error(RPCError::DaoError, err.unwrap_cause_or_self())
            }
            ErrorKind::OutPoint => {
                Self::custom_with_error(RPCError::TransactionFailedToResolve, err)
            }
            ErrorKind::Transaction => Self::custom_with_error(
                RPCError::TransactionFailedToVerify,
                err.unwrap_cause_or_self(),
            ),
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

    /// TODO(doc): @doitian
    pub fn from_failure_error(err: failure::Error) -> Error {
        match err.downcast::<CKBError>() {
            Ok(ckb_error) => Self::from_ckb_error(ckb_error),
            Err(err) => Self::ckb_internal_error(err),
        }
    }

    /// TODO(doc): @doitian
    pub fn ckb_internal_error<T: Display + Debug>(err: T) -> Error {
        Self::custom_with_error(RPCError::CKBInternalError, err)
    }

    /// TODO(doc): @doitian
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

    /// TODO(doc): @doitian
    pub fn rpc_method_is_deprecated() -> Error {
        Self::custom(
            RPCError::Deprecated,
            "This RPC method is deprecated, it will be removed in future release. \
            Please check the related information in the CKB release notes and RPC document. \
            You may enable deprecated methods via adding `enable_deprecated_rpc = true` to the `[rpc]` section in ckb.toml.",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_dao_utils::DaoError;
    use ckb_types::{core::error::OutPointError, packed::Byte32};

    #[test]
    fn test_dao_error_from_ckb_error() {
        let err: CKBError = DaoError::InvalidHeader.into();
        assert_eq!(
            "DaoError: InvalidHeader",
            RPCError::from_ckb_error(err).message
        );
    }

    #[test]
    fn test_submit_transaction_error() {
        let err: CKBError = Reject::LowFeeRate(100, 50).into();
        assert_eq!(
            "PoolRejectedTransactionByMinFeeRate: Transaction fee rate must >= 100 shannons/KB, got: 50",
            RPCError::from_submit_transaction_reject(RPCError::downcast_submit_transaction_reject(&err).unwrap()).message
        );

        let err: CKBError = Reject::ExceededMaximumAncestorsCount.into();
        assert_eq!(
            "PoolRejectedTransactionByMaxAncestorsCountLimit: Transaction exceeded maximum ancestors count limit, try send it later",
            RPCError::from_submit_transaction_reject(RPCError::downcast_submit_transaction_reject(&err).unwrap()).message
        );

        let err: CKBError = Reject::Full("size".to_owned(), 10).into();
        assert_eq!(
            "PoolIsFull: Transaction pool exceeded maximum size limit(10), try send it later",
            RPCError::from_submit_transaction_reject(
                RPCError::downcast_submit_transaction_reject(&err).unwrap()
            )
            .message
        );

        let err: CKBError = Reject::Duplicated(Byte32::new([0; 32])).into();
        assert_eq!(
            "PoolRejectedDuplicatedTransaction: Transaction(Byte32(0x0000000000000000000000000000000000000000000000000000000000000000)) already exist in transaction_pool",
            RPCError::from_submit_transaction_reject(RPCError::downcast_submit_transaction_reject(&err).unwrap()).message
        );

        let err: CKBError = Reject::Malformed("cellbase like".to_owned()).into();
        assert_eq!(
            "PoolRejectedMalformedTransaction: Malformed cellbase like transaction",
            RPCError::from_submit_transaction_reject(
                RPCError::downcast_submit_transaction_reject(&err).unwrap()
            )
            .message
        );
    }

    #[test]
    fn test_out_point_error_from_ckb_error() {
        let err: CKBError = OutPointError::InvalidHeader(Byte32::new([0; 32])).into();
        assert_eq!(
            "TransactionFailedToResolve: OutPoint(InvalidHeader(Byte32(0x0000000000000000000000000000000000000000000000000000000000000000)))",
            RPCError::from_ckb_error(err).message
        );
    }
}
