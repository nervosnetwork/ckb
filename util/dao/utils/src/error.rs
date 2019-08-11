use ckb_error::{Error, ErrorKind};
use failure::{format_err, Error as FailureError, Fail};
use std::convert::TryFrom;
use std::fmt::Display;

#[derive(Fail, Debug, PartialEq, Clone, Eq, Display)]
pub enum DaoError {
    InvalidHeader,
    InvalidOutPoint,
    InvalidDaoFormat,
}

impl<'a> TryFrom<&'a Error> for &'a DaoError {
    type Error = FailureError;
    fn try_from(value: &'a Error) -> Result<Self, Self::Error> {
        value
            .downcast_ref::<DaoError>()
            .ok_or_else(|| format_err!("failed to downcast ckb_error::Error to DaoError"))
    }
}

impl From<DaoError> for Error {
    fn from(error: DaoError) -> Self {
        error.context(ErrorKind::Dao).into()
    }
}
