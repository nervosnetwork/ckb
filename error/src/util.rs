//! Error-related macros

/// Compare two errors.
///
/// NOTE: Used for testing only!
#[doc(hidden)]
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

/// A macro to implement conversion from source type to target type with an implicit error kind.
///
/// ## Examples
///
/// ```text
/// impl_error_conversion_with_kind!(SourceType, error_kind, TargetType)
/// ```
///
/// the expanded code:
///
/// ```text
/// impl From<SourceType> for TargetType {
///     fn from(source: SourceType) -> TargetType {
///         TargetType {
///             kind: error_kind,
///             inner: source.into(),
///         }
///     }
/// }
/// ```
#[doc(hidden)]
#[macro_export]
macro_rules! impl_error_conversion_with_kind {
    ($source:ty, $kind:expr, $target:ty) => {
        impl ::std::convert::From<$source> for $target {
            fn from(error: $source) -> Self {
                $kind.because(error)
            }
        }
    };
}

/// A macro to implement conversion from source type to target type based on an implicit middle adaptor.
///
/// ## Examples
///
/// ```text
/// impl_error_conversion_with_adaptor!(SourceType, AdaptorType, TargetType)
/// ```
///
/// the expanded code:
///
/// ```text
/// impl From<SourceType> for TargetType {
///     fn from(source: SourceType) -> TargetType {
///         let adaptor: AdaptorType = source.into();
///         let target: TargetType = adaptor.into();
///         target
///     }
/// }
/// ```
#[doc(hidden)]
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

#[doc(hidden)]
#[macro_export]
macro_rules! def_error_base_on_kind {
    ($error:ident, $error_kind:ty, $comment_error:expr, $comment_because:expr, $comment_simple:expr) => {
        #[doc = $comment_error]
        #[derive(Error, Debug, Clone)]
        pub struct $error {
            kind: $error_kind,
            inner: $crate::AnyError,
        }

        impl ::std::fmt::Display for $error {
            fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                if let Some(err) = self.cause() {
                    if f.alternate() {
                        write!(f, "{}: {}", self.kind(), err)
                    } else {
                        write!(f, "{}({})", self.kind(), err)
                    }
                } else {
                    write!(f, "{}", self.kind())
                }
            }
        }

        impl ::std::convert::From<$error_kind> for $error {
            fn from(kind: $error_kind) -> Self {
                kind.because($crate::SilentError)
            }
        }

        impl $error_kind {
            #[doc = $comment_because]
            pub fn because<E>(self, reason: E) -> $error
            where
                E: ::std::error::Error + Send + Sync + 'static,
            {
                $error {
                    kind: self,
                    inner: reason.into(),
                }
            }

            #[doc = $comment_simple]
            pub fn other<T>(self, reason: T) -> $error
            where
                T: ::std::fmt::Display,
            {
                $error {
                    kind: self,
                    inner: $crate::OtherError::new(reason.to_string()).into(),
                }
            }
        }

        impl $error {
            /// Returns the general category of this error.
            pub fn kind(&self) -> $error_kind {
                self.kind
            }

            /// Downcast this error object by reference.
            pub fn downcast_ref<E>(&self) -> Option<&E>
            where
                E: ::std::fmt::Display + ::std::fmt::Debug + Send + Sync + 'static,
            {
                self.inner.downcast_ref::<E>()
            }

            /// The lowest level cause of this error — this error's cause's cause's cause etc.
            pub fn root_cause(&self) -> &(dyn ::std::error::Error + 'static) {
                self.inner.root_cause()
            }

            /// The lower-level source of this error, if any.
            pub fn cause(&self) -> Option<&(dyn ::std::error::Error + 'static)> {
                self.inner.chain().next()
            }
        }
    };
    ($error:ident, $error_kind:ty, $comment_error:expr) => {
        def_error_base_on_kind!(
            $error,
            $error_kind,
            $comment_error,
            concat!(
                "Creates `",
                stringify!($error),
                "` base on `",
                stringify!($error_kind),
                "` with an error as the reason."
            ),
            concat!(
                "Creates `",
                stringify!($error),
                "` base on `",
                stringify!($error_kind),
                "` with a simple string as the reason."
            )
        );
    };
    ($error:ident, $error_kind:ty) => {
        def_error_base_on_kind!($error, $error_kind, "/// TODO(doc): @keroro520");
    };
}
