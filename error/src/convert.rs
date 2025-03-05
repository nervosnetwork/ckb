use crate::{
    Error, InternalError, InternalErrorKind, impl_error_conversion_with_adaptor,
    impl_error_conversion_with_kind,
};

impl_error_conversion_with_kind!(
    ckb_occupied_capacity::Error,
    InternalErrorKind::CapacityOverflow,
    InternalError
);
impl_error_conversion_with_adaptor!(ckb_occupied_capacity::Error, InternalError, Error);
