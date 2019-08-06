use failure::Fail;
// use ckb_vm::Error as VMInternalError;

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum InternalError {
    /// An arithmetic overflow occurs during capacity calculation,
    /// e.g. `Capacity::safe_add`
    // NOTE: the original name is {Transaction,Block}::CapacityOverflow
    #[fail(display = "Arithmetic overflow during capacity calculation")]
    ArithmeticOverflowCapacity,

    #[fail(display = "Corrupted data: {}", _0)]
    CorruptedData(String),

    // NOTE: the original name is ckb_db::Error::DBError(String)
    #[fail(display = "Database error: {}", _0)]
    Database(String),

    // NOTE: the original name is LimitReached
    #[fail(display = "Full Transaction Pool")]
    FullTransactionPool,

    // FIXME
    /// VM internal error
    // VM(#[fail(cause)] VMInternalError),
    #[fail(display = "{:?}", _0)]
    VM(String),
}
