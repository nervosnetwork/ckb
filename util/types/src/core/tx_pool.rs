//! Tx-pool shared type define.
use crate::core::{
    error::{OutPointError, TransactionError},
    Capacity, Cycle, FeeRate,
};
use crate::packed::Byte32;
use ckb_error::{
    impl_error_conversion_with_kind, prelude::*, Error, ErrorKind, InternalError, InternalErrorKind,
};
use std::collections::HashMap;

/// TX reject message
#[derive(Error, Debug, Clone)]
pub enum Reject {
    /// Transaction fee lower than config
    #[error("The min fee rate is {0} shannons/KB, so the transaction fee should be {1} shannons at least, but only got {2}")]
    LowFeeRate(FeeRate, u64, u64),

    /// Transaction exceeded maximum ancestors count limit
    #[error("Transaction exceeded maximum ancestors count limit, try send it later")]
    ExceededMaximumAncestorsCount,

    /// Transaction pool exceeded maximum size or cycles limit,
    #[error("Transaction pool exceeded maximum {0} limit({1}), try send it later")]
    Full(String, u64),

    /// Transaction already exist in transaction_pool
    #[error("Transaction({0}) already exist in transaction_pool")]
    Duplicated(Byte32),

    /// Malformed transaction
    #[error("Malformed {0} transaction")]
    Malformed(String),

    /// Resolve failed
    #[error("Resolve failed {0}")]
    Resolve(OutPointError),

    /// Verification failed
    #[error("Verification failed {0}")]
    Verification(Error),
}

fn is_malformed_from_verification(error: &Error) -> bool {
    match error.kind() {
        ErrorKind::Transaction => error
            .downcast_ref::<TransactionError>()
            .expect("error kind checked")
            .is_malformed_tx(),
        ErrorKind::Script => true,
        ErrorKind::Internal => {
            error
                .downcast_ref::<InternalError>()
                .expect("error kind checked")
                .kind()
                == InternalErrorKind::CapacityOverflow
        }
        _ => false,
    }
}

impl Reject {
    /// Returns true if the reject reason is malformed tx.
    pub fn is_malformed_tx(&self) -> bool {
        match self {
            Reject::Malformed(_) => true,
            Reject::Verification(err) => is_malformed_from_verification(err),
            _ => false,
        }
    }
}

impl_error_conversion_with_kind!(Reject, ErrorKind::SubmitTransaction, Error);

/// Tx-pool transaction status
#[derive(Debug, Clone)]
pub enum TxStatus {
    /// Status "pending". The transaction is in the pool, and not proposed yet.
    Pending,
    /// Status "proposed". The transaction is in the pool and has been proposed.
    Proposed,
    /// Status "reject". The transaction is reject.
    Reject(Reject),
}

/// Tx-pool entry info
#[derive(Debug, PartialEq, Eq)]
pub struct TxEntryInfo {
    /// Consumed cycles.
    pub cycles: Cycle,
    /// The transaction serialized size in block.
    pub size: u64,
    /// The transaction fee.
    pub fee: Capacity,
    /// Size of in-tx-pool ancestor transactions
    pub ancestors_size: u64,
    /// Cycles of in-tx-pool ancestor transactions
    pub ancestors_cycles: u64,
    /// Number of in-tx-pool ancestor transactions
    pub ancestors_count: u64,
}

/// Array of transaction ids
#[derive(Debug, PartialEq, Eq)]
pub struct TxPoolIds {
    /// Pending transaction ids
    pub pending: Vec<Byte32>,
    /// Proposed transaction ids
    pub proposed: Vec<Byte32>,
}

/// All in-pool transaction entry info
#[derive(Debug, PartialEq, Eq)]
pub struct TxPoolEntryInfo {
    /// Pending transaction entry info
    pub pending: HashMap<Byte32, TxEntryInfo>,
    /// Proposed transaction entry info
    pub proposed: HashMap<Byte32, TxEntryInfo>,
}
