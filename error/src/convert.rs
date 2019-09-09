use crate::{Error, InternalError, InternalErrorKind};

impl From<ckb_occupied_capacity::Error> for InternalError {
    fn from(_error: ckb_occupied_capacity::Error) -> Self {
        InternalErrorKind::CapacityOverflow.into()
    }
}

impl From<ckb_occupied_capacity::Error> for Error {
    fn from(_error: ckb_occupied_capacity::Error) -> Self {
        InternalErrorKind::CapacityOverflow.into()
    }
}
