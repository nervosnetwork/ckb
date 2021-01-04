use ckb_error::{prelude::*, Error, ErrorKind};

/// TODO(doc): @keroro520
#[derive(Error, Debug, PartialEq, Clone, Eq)]
pub enum DaoError {
    /// TODO(doc): @keroro520
    #[error("InvalidHeader")]
    InvalidHeader,
    /// TODO(doc): @keroro520
    #[error("InvalidOutPoint")]
    InvalidOutPoint,
    /// TODO(doc): @keroro520
    #[error("InvalidDaoFormat")]
    InvalidDaoFormat,
    /// TODO(doc): @keroro520
    #[error("Overflow")]
    Overflow,
    /// TODO(doc): @keroro520
    #[error("ZeroC")]
    ZeroC,
}

impl From<DaoError> for Error {
    fn from(error: DaoError) -> Self {
        ErrorKind::Dao.because(error)
    }
}
