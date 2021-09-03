use crate::ReadOnlyDB;

#[test]
fn test_open_read_only_not_exist() {
    let tmp_dir = tempfile::Builder::new()
        .prefix("test_open_read_only_not_exist")
        .tempdir()
        .unwrap();

    let cfs: Vec<&str> = vec![];
    let db = ReadOnlyDB::open_cf(&tmp_dir, cfs);
    assert!(matches!(db, Ok(x) if x.is_none()));

    let cfs: Vec<&str> = vec!["0"];
    let db = ReadOnlyDB::open_cf(&tmp_dir, cfs);
    assert!(matches!(db, Ok(x) if x.is_none()));
}
