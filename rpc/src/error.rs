use ckb_error::{Error as CKBError, InternalError, InternalErrorKind};
use ckb_tx_pool::error::SubmitTxError;
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

    pub fn from_ckb_error(err: CKBError) -> Error {
        use ckb_error::ErrorKind::*;
        match err.kind() {
            Dao => Self::custom_with_error(RPCError::DaoError, err.as_fail_ref()),
            OutPoint => Self::custom_with_error(RPCError::TransactionFailedToResolve, err),
            Transaction => {
                Self::custom_with_error(RPCError::TransactionFailedToVerify, err.as_fail_ref())
            }
            SubmitTransaction => {
                let submit_tx_err = match err.downcast_ref::<SubmitTxError>() {
                    Some(err) => err,
                    None => return Self::ckb_internal_error(err),
                };

                let kind = match *submit_tx_err {
                    SubmitTxError::LowFeeRate(_, _) => {
                        RPCError::PoolRejectedTransactionByMinFeeRate
                    }
                    SubmitTxError::ExceededMaximumAncestorsCount => {
                        RPCError::PoolRejectedTransactionByMaxAncestorsCountLimit
                    }
                };

                RPCError::custom_with_error(kind, submit_tx_err)
            }
            Internal => {
                let internal_err = match err.downcast_ref::<InternalError>() {
                    Some(err) => err,
                    None => return Self::ckb_internal_error(err),
                };

                let kind = match internal_err.kind() {
                    InternalErrorKind::CapacityOverflow => RPCError::IntegerOverflow,
                    InternalErrorKind::TransactionPoolFull => RPCError::PoolIsFull,
                    InternalErrorKind::PoolTransactionDuplicated => {
                        RPCError::PoolRejectedDuplicatedTransaction
                    }
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
    fn test_submit_tx_error_from_ckb_error() {
        let err: CKBError = SubmitTxError::LowFeeRate(100, 50).into();
        assert_eq!(
            "PoolRejectedTransactionByMinFeeRate: Transaction fee rate must >= 100 shannons/KB, got: 50",
            RPCError::from_ckb_error(err).message
        );

        let err: CKBError = SubmitTxError::ExceededMaximumAncestorsCount.into();
        assert_eq!(
            "PoolRejectedTransactionByMaxAncestorsCountLimit: Transaction exceeded maximum ancestors count limit, try send it later",
            RPCError::from_ckb_error(err).message
        );
    }

    #[test]
    fn test_internal_error_from_ckb_error() {
        let err: CKBError = InternalErrorKind::TransactionPoolFull.into();
        assert_eq!(
            "PoolIsFull: TransactionPoolFull",
            RPCError::from_ckb_error(err).message
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
