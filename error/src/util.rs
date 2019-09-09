use crate::Error;

/// Compare two errors
///
/// Used for testing only
pub fn assert_error_eq<L, R>(l: L, r: R)
where
    L: Into<Error>,
    R: Into<Error>,
{
    assert_eq!(
        Into::<Error>::into(l).to_string(),
        Into::<Error>::into(r).to_string(),
    );
}

#[macro_export]
macro_rules! impl_error_conversion_with_kind {
    ($source:ty, $kind:expr, $target:ty) => {
        impl ::std::convert::From<$source> for $target {
            fn from(error: $source) -> Self {
                error.context($kind).into()
            }
        }
    };
}

#[macro_export]
macro_rules! impl_error_conversion_with_adaptor {
    ($source:ty, $adaptor:ty, $target:ty) => {
        impl ::std::convert::From<$source> for $target {
            fn from(error: $source) -> Self {
                ::std::convert::Into::<$adaptor>::into(error).into()
            }
        }
    };
}
