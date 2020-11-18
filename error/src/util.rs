//! TODO(doc): @keroro520

/// Compare two errors
///
/// Used for testing only
#[macro_export]
macro_rules! assert_error_eq {
    ($left:expr, $right:expr) => {
        assert_eq!(
            Into::<$crate::Error>::into($left).to_string(),
            Into::<$crate::Error>::into($right).to_string(),
        );
    };
    ($left:expr, $right:expr,) => {
        $crate::assert_error_eq!($left, $right);
    };
    ($left:expr, $right:expr, $($arg:tt)+) => {
        assert_eq!(
            Into::<$crate::Error>::into($left).to_string(),
            Into::<$crate::Error>::into($right).to_string(),
            $($arg)+
        );
    }
}

/// TODO(doc): @keroro520
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

/// TODO(doc): @keroro520
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
