use ckb_db::Error as DBError;
use failure::Fail;

#[derive(Debug, PartialEq, Clone, Eq, Fail)]
pub enum SharedError {
    #[fail(display = "InvalidTransaction: {}", _0)]
    InvalidTransaction(String),
    #[fail(display = "InvalidParentBlock")]
    InvalidParentBlock,
    #[fail(display = "DB error: {}", _0)]
    DB(DBError),
}
