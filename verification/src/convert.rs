use crate::error::{
    BlockError, BlockErrorKind, BlockTransactionsError, CellbaseError, CommitError, EpochError,
    HeaderError, HeaderErrorKind, InvalidParentError, NumberError, PowError, TimestampError,
    TransactionError, UnclesError, UnknownParentError,
};
use ckb_error::{Error, ErrorKind};
use failure::{format_err, Context, Error as FailureError, Fail};
use std::convert::TryFrom;

impl From<BlockErrorKind> for BlockError {
    fn from(kind: BlockErrorKind) -> Self {
        Context::new(kind).into()
    }
}

impl From<BlockErrorKind> for Error {
    fn from(kind: BlockErrorKind) -> Self {
        Into::<BlockError>::into(kind).into()
    }
}

impl<'a> TryFrom<&'a Error> for &'a TransactionError {
    type Error = FailureError;
    fn try_from(value: &'a Error) -> Result<Self, Self::Error> {
        value
            .downcast_ref::<TransactionError>()
            .ok_or_else(|| format_err!("failed to downcast ckb_error::Error to TransactionError"))
    }
}

impl<'a> TryFrom<&'a Error> for &'a HeaderError {
    type Error = FailureError;
    fn try_from(value: &'a Error) -> Result<Self, Self::Error> {
        value
            .downcast_ref::<HeaderError>()
            .ok_or_else(|| format_err!("failed to downcast ckb_error::Error to HeaderError"))
    }
}

impl<'a> TryFrom<&'a Error> for &'a BlockError {
    type Error = FailureError;
    fn try_from(value: &'a Error) -> Result<Self, Self::Error> {
        value
            .downcast_ref::<BlockError>()
            .ok_or_else(|| format_err!("failed to downcast ckb_error::Error to BlockError"))
    }
}

impl From<TransactionError> for Error {
    fn from(error: TransactionError) -> Self {
        error.context(ErrorKind::Transaction).into()
    }
}

impl From<HeaderError> for Error {
    fn from(error: HeaderError) -> Self {
        error.context(ErrorKind::Header).into()
    }
}

impl From<BlockError> for Error {
    fn from(error: BlockError) -> Self {
        error.context(ErrorKind::Block).into()
    }
}

impl From<InvalidParentError> for Error {
    fn from(error: InvalidParentError) -> Self {
        Into::<HeaderError>::into(error).into()
    }
}

impl From<PowError> for Error {
    fn from(error: PowError) -> Self {
        Into::<HeaderError>::into(error).into()
    }
}

impl From<TimestampError> for Error {
    fn from(error: TimestampError) -> Self {
        Into::<HeaderError>::into(error).into()
    }
}

impl From<NumberError> for Error {
    fn from(error: NumberError) -> Self {
        Into::<HeaderError>::into(error).into()
    }
}

impl From<EpochError> for Error {
    fn from(error: EpochError) -> Self {
        Into::<HeaderError>::into(error).into()
    }
}

impl From<BlockTransactionsError> for Error {
    fn from(error: BlockTransactionsError) -> Self {
        Into::<BlockError>::into(error).into()
    }
}

impl From<UnknownParentError> for Error {
    fn from(error: UnknownParentError) -> Self {
        Into::<BlockError>::into(error).into()
    }
}

impl From<CommitError> for Error {
    fn from(error: CommitError) -> Self {
        Into::<BlockError>::into(error).into()
    }
}

impl From<CellbaseError> for Error {
    fn from(error: CellbaseError) -> Self {
        Into::<BlockError>::into(error).into()
    }
}

impl From<UnclesError> for Error {
    fn from(error: UnclesError) -> Self {
        Into::<BlockError>::into(error).into()
    }
}

impl From<InvalidParentError> for HeaderError {
    fn from(error: InvalidParentError) -> Self {
        error.context(HeaderErrorKind::InvalidParent).into()
    }
}

impl From<PowError> for HeaderError {
    fn from(error: PowError) -> Self {
        error.context(HeaderErrorKind::Pow).into()
    }
}

impl From<TimestampError> for HeaderError {
    fn from(error: TimestampError) -> Self {
        error.context(HeaderErrorKind::Timestamp).into()
    }
}

impl From<NumberError> for HeaderError {
    fn from(error: NumberError) -> Self {
        error.context(HeaderErrorKind::Number).into()
    }
}

impl From<EpochError> for HeaderError {
    fn from(error: EpochError) -> Self {
        error.context(HeaderErrorKind::Epoch).into()
    }
}

impl From<BlockTransactionsError> for BlockError {
    fn from(error: BlockTransactionsError) -> Self {
        error.context(BlockErrorKind::BlockTransactions).into()
    }
}

impl From<UnknownParentError> for BlockError {
    fn from(error: UnknownParentError) -> Self {
        error.context(BlockErrorKind::UnknownParent).into()
    }
}

impl From<CommitError> for BlockError {
    fn from(error: CommitError) -> Self {
        error.context(BlockErrorKind::Commit).into()
    }
}

impl From<CellbaseError> for BlockError {
    fn from(error: CellbaseError) -> Self {
        error.context(BlockErrorKind::Cellbase).into()
    }
}

impl From<UnclesError> for BlockError {
    fn from(error: UnclesError) -> Self {
        error.context(BlockErrorKind::Uncles).into()
    }
}
