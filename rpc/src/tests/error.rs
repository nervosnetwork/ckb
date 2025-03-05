use ckb_dao_utils::DaoError;
use ckb_error::Error as CKBError;
use ckb_tx_pool::error::Reject;
use ckb_types::{
    core::{FeeRate, error::OutPointError},
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
    let reject = Reject::LowFeeRate(min_fee_rate, 100, 50);
    assert_eq!(
        "PoolRejectedTransactionByMinFeeRate: The min fee rate is 500 shannons/KW, requiring a transaction fee of at least 100 shannons, but the fee provided is only 50",
        RPCError::from_submit_transaction_reject(&reject).message
    );

    let reject = Reject::ExceededMaximumAncestorsCount;
    assert_eq!(
        "PoolRejectedTransactionByMaxAncestorsCountLimit: Transaction exceeded maximum ancestors count limit; try later",
        RPCError::from_submit_transaction_reject(&reject).message
    );

    let reject = Reject::Full(format!(
        "the fee_rate for this transaction is: {}",
        FeeRate::from_u64(500)
    ));
    assert_eq!(
        "PoolIsFull: Transaction is replaced because the pool is full, the fee_rate for this transaction is: 500 shannons/KW",
        RPCError::from_submit_transaction_reject(&reject).message
    );

    let reject = Reject::Duplicated(Byte32::new([0; 32]));
    assert_eq!(
        "PoolRejectedDuplicatedTransaction: Transaction(Byte32(0x0000000000000000000000000000000000000000000000000000000000000000)) already exists in transaction_pool",
        RPCError::from_submit_transaction_reject(&reject).message
    );

    let reject = Reject::Malformed("cellbase like".to_owned(), "".to_owned());
    assert_eq!(
        "PoolRejectedMalformedTransaction: Malformed cellbase like transaction",
        RPCError::from_submit_transaction_reject(&reject).message
    );

    let reject = Reject::ExceededTransactionSizeLimit(10, 9);
    assert_eq!(
        "PoolRejectedTransactionBySizeLimit: Transaction size 10 exceeded maximum limit 9",
        RPCError::from_submit_transaction_reject(&reject).message
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
