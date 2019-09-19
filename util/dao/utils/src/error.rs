use ckb_error::{Error, ErrorKind};
use failure::Fail;
use std::fmt::Display;

#[derive(Fail, Debug, PartialEq, Clone, Eq, Display)]
pub enum DaoError {
    InvalidHeader,
    InvalidOutPoint,
    InvalidDaoFormat,
    Overflow,
    ZeroC,
}

impl From<DaoError> for Error {
    fn from(error: DaoError) -> Self {
        error.context(ErrorKind::Dao).into()
    }
}
