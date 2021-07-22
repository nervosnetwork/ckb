use ckb_error::{Error as CKBError, ErrorKind, InternalError, InternalErrorKind};
use failure::Error as FailureError;

/// Returns whether the error's kind is `InternalErrorKind::Database`
///
/// ### Panic
///
/// Panic if the error kind is `InternalErrorKind::DataCorrupted`.
/// If the database is corrupted, panic is better than handle it silently.
pub(crate) fn is_ckb_db_error(error: &CKBError) -> bool {
    if *error.kind() == ErrorKind::Internal {
        let error_kind = error
            .downcast_ref::<InternalError>()
            .expect("error kind checked")
            .kind();
        if *error_kind == InternalErrorKind::DataCorrupted {
            panic!("{}", error)
        } else {
            return *error_kind == InternalErrorKind::Database;
        }
    }
    false
}

/// Returns whether the error's kind is `InternalErrorKind::Database`
///
/// ### Panic
///
/// Panic if the error kind is `InternalErrorKind::DataCorrupted`.
/// If the database is corrupted, panic is better than handle it silently.
pub(crate) fn is_failure_db_error(error: &FailureError) -> bool {
    if let Some(ref error) = error.downcast_ref::<CKBError>() {
        return is_ckb_db_error(error);
    }
    false
}
