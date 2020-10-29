use ckb_error::{Error, ErrorKind};
use failure::Fail;
use std::fmt::Display;

/// TODO(doc): @keroro520
#[derive(Fail, Debug, PartialEq, Clone, Eq, Display)]
pub enum DaoError {
    /// TODO(doc): @keroro520
    InvalidHeader,
    /// TODO(doc): @keroro520
    InvalidOutPoint,
    /// TODO(doc): @keroro520
    InvalidDaoFormat,
    /// TODO(doc): @keroro520
    Overflow,
    /// TODO(doc): @keroro520
    ZeroC,
}

impl From<DaoError> for Error {
    fn from(error: DaoError) -> Self {
        error.context(ErrorKind::Dao).into()
    }
}
