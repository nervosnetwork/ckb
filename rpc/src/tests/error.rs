use ckb_dao_utils::DaoError;
use ckb_error::Error as CKBError;
use ckb_tx_pool::error::Reject;
use ckb_types::{
    core::{error::OutPointError, FeeRate},
    packed::Byte32,
};

use crate::error::RPCError;

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
    let min_fee_rate = FeeRate::from_u64(500);
    let err: CKBError = Reject::LowFeeRate(min_fee_rate, 100, 50).into();
    assert_eq!(
            "PoolRejectedTransactionByMinFeeRate: The min fee rate is 500 shannons/KB, so the transaction fee should be 100 shannons at least, but only got 50",
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
