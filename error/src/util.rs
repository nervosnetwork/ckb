/// Compare two errors by the debug strings
///
/// Used for testing only
pub fn assert_error_eq<D: std::fmt::Debug>(l: D, r: D) {
    assert_eq!(format!("{:?}", l), format!("{:?}", r),);
}
