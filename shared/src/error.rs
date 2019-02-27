use ckb_db::kvdb::Error as DBError;
use failure::Fail;

#[derive(Debug, PartialEq, Clone, Eq, Fail)]
pub enum SharedError {
    #[fail(display = "InvalidInput")]
    InvalidInput,
    #[fail(display = "InvalidOutput")]
    InvalidOutput,
    #[fail(display = "InvalidTransaction")]
    InvalidTransaction,
    #[fail(display = "DB error: {}", _0)]
    DB(DBError),
}
