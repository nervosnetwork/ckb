use ckb_error::Error;

use crate::error::{BlockVersionError, HeaderError, HeaderErrorKind, TimestampError};

#[test]
fn is_too_new() {
    let too_old = TimestampError::BlockTimeTooOld { min: 0, actual: 0 };
    let too_new = TimestampError::BlockTimeTooNew { max: 0, actual: 0 };

    let errors: Vec<HeaderError> = vec![
        HeaderErrorKind::InvalidParent.into(),
        HeaderErrorKind::Pow.into(),
        HeaderErrorKind::Version.into(),
        HeaderErrorKind::Epoch.into(),
        HeaderErrorKind::Version.into(),
        HeaderErrorKind::Timestamp.into(),
        too_old.into(),
        too_new.into(),
    ];

    let is_too_new: Vec<bool> = errors.iter().map(|e| e.is_too_new()).collect();
    assert_eq!(
        is_too_new,
        vec![false, false, false, false, false, false, false, true]
    );
}

#[test]
fn test_version_error_display() {
    let e: Error = BlockVersionError {
        expected: 0,
        actual: 1,
    }
    .into();

    assert_eq!(
        "Header(Version(BlockVersionError(expected: 0, actual: 1)))",
        format!("{e}")
    );
}
