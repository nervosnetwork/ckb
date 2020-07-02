use crate::error::{
    BlockError, BlockErrorKind, BlockTransactionsError, CellbaseError, CommitError, EpochError,
    HeaderError, HeaderErrorKind, InvalidParentError, NumberError, PowError, TimestampError,
    TransactionError, UnclesError, UnknownParentError,
};
use ckb_error::{
    impl_error_conversion_with_adaptor, impl_error_conversion_with_kind, Error, ErrorKind,
};
use failure::{Context, Fail};

impl From<HeaderErrorKind> for HeaderError {
    fn from(kind: HeaderErrorKind) -> Self {
        Context::new(kind).into()
    }
}

impl From<BlockErrorKind> for BlockError {
    fn from(kind: BlockErrorKind) -> Self {
        Context::new(kind).into()
    }
}

impl_error_conversion_with_kind!(TransactionError, ErrorKind::Transaction, Error);
impl_error_conversion_with_kind!(HeaderError, ErrorKind::Header, Error);
impl_error_conversion_with_kind!(BlockError, ErrorKind::Block, Error);

impl_error_conversion_with_kind!(
    InvalidParentError,
    HeaderErrorKind::InvalidParent,
    HeaderError
);
impl_error_conversion_with_kind!(PowError, HeaderErrorKind::Pow, HeaderError);
impl_error_conversion_with_kind!(TimestampError, HeaderErrorKind::Timestamp, HeaderError);
impl_error_conversion_with_kind!(NumberError, HeaderErrorKind::Number, HeaderError);
impl_error_conversion_with_kind!(EpochError, HeaderErrorKind::Epoch, HeaderError);

impl_error_conversion_with_kind!(
    BlockTransactionsError,
    BlockErrorKind::BlockTransactions,
    BlockError
);
impl_error_conversion_with_kind!(
    UnknownParentError,
    BlockErrorKind::UnknownParent,
    BlockError
);
impl_error_conversion_with_kind!(CommitError, BlockErrorKind::Commit, BlockError);
impl_error_conversion_with_kind!(CellbaseError, BlockErrorKind::Cellbase, BlockError);
impl_error_conversion_with_kind!(UnclesError, BlockErrorKind::Uncles, BlockError);

impl_error_conversion_with_adaptor!(InvalidParentError, HeaderError, Error);
impl_error_conversion_with_adaptor!(PowError, HeaderError, Error);
impl_error_conversion_with_adaptor!(TimestampError, HeaderError, Error);
impl_error_conversion_with_adaptor!(NumberError, HeaderError, Error);
impl_error_conversion_with_adaptor!(EpochError, HeaderError, Error);

impl_error_conversion_with_adaptor!(BlockErrorKind, BlockError, Error);
impl_error_conversion_with_adaptor!(HeaderErrorKind, HeaderError, Error);
impl_error_conversion_with_adaptor!(BlockTransactionsError, BlockError, Error);
impl_error_conversion_with_adaptor!(UnknownParentError, BlockError, Error);
impl_error_conversion_with_adaptor!(CommitError, BlockError, Error);
impl_error_conversion_with_adaptor!(CellbaseError, BlockError, Error);
impl_error_conversion_with_adaptor!(UnclesError, BlockError, Error);
