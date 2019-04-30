use ckb_core::cell::UnresolvableError;
use ckb_db::Error as DBError;
use failure::Fail;

#[derive(Debug, PartialEq, Clone, Eq, Fail)]
pub enum SharedError {
    #[fail(display = "UnresolvableTransaction: {:?}", _0)]
    UnresolvableTransaction(UnresolvableError),
    #[fail(display = "InvalidTransaction: {}", _0)]
    InvalidTransaction(String),
    #[fail(display = "InvalidParentBlock")]
    InvalidParentBlock,
    #[fail(display = "InvalidData error: {}", _0)]
    InvalidData(String),
    #[fail(display = "DB error: {}", _0)]
    DB(DBError),
}
