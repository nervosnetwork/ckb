use ckb_error::{Error as CKBError, ErrorKind, InternalError, InternalErrorKind};
use ckb_tx_pool::error::Reject;
use jsonrpc_core::{Error, ErrorCode, Value};
use std::fmt::{Debug, Display};

// * -1 ~ -999 General errors
// * -1000 ~ -2999 Module specific errors
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum RPCError {
    // ,-- General application errors
    CKBInternalError = -1,
    Deprecated = -2,
    Invalid = -3,
    RPCModuleIsDisabled = -4,
    DaoError = -5,
    IntegerOverflow = -6,
    ConfigError = -7,
    // ,-- P2P errors
    P2PFailedToBroadcast = -101,
    // ,-- Store errors
    DatabaseError = -200,
    ChainIndexIsInconsistent = -201,
    DatabaseIsCorrupt = -202,
    // ,-- Transaction errors
    TransactionFailedToResolve = -301,
    TransactionFailedToVerify = -302,
    // ,-- Alert module
    AlertFailedToVerifySignatures = -1000,
    // ,-- Pool module
    PoolRejectedTransactionByOutputsValidator = -1102,
    PoolRejectedTransactionByIllTransactionChecker = -1103,
    PoolRejectedTransactionByMinFeeRate = -1104,
    PoolRejectedTransactionByMaxAncestorsCountLimit = -1105,
    PoolIsFull = -1106,
    PoolRejectedDuplicatedTransaction = -1107,
    PoolRejectedMalformedTransaction = -1108,
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

    pub fn custom_with_error<T: Display + Debug>(error_code: RPCError, err: T) -> Error {
        Error {
            code: ErrorCode::ServerError(error_code as i64),
            message: format!("{:?}: {}", error_code, err),
            data: Some(Value::String(format!("{:?}", err))),
        }
    }

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

    pub fn downcast_submit_transaction_reject(err: &CKBError) -> Option<&Reject> {
        use ckb_error::ErrorKind::SubmitTransaction;
        match err.kind() {
            SubmitTransaction => err.downcast_ref::<Reject>(),
            _ => None,
        }
    }

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

    pub fn from_failure_error(err: failure::Error) -> Error {
        match err.downcast::<CKBError>() {
            Ok(ckb_error) => Self::from_ckb_error(ckb_error),
            Err(err) => Self::ckb_internal_error(err),
        }
    }

    pub fn ckb_internal_error<T: Display + Debug>(err: T) -> Error {
        Self::custom_with_error(RPCError::CKBInternalError, err)
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
