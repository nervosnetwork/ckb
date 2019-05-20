use failure::Context;

#[derive(Debug)]
pub struct Error {
    inner: Context<ErrorKind>,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug, Fail)]
pub enum ErrorKind {
    #[fail(display = "The count of sigs should less than pks.")]
    SigCountOverflow,
    #[fail(display = "The count of sigs less than threshold.")]
    SigNotEnough,
    #[fail(display = "Threshold not reach, sigs: {} required sigs: {}.", _0, _1)]
    Threshold(usize, usize),
}

impl Error {
    pub fn kind(&self) -> ErrorKind {
        *self.inner.get_context()
    }
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Error {
        Error {
            inner: Context::new(kind),
        }
    }
}

impl From<Context<ErrorKind>> for Error {
    fn from(inner: Context<ErrorKind>) -> Error {
        Error { inner }
    }
}
