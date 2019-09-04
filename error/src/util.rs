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
